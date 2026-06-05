use ai_tester::assertions::{compute_weighted_score, evaluate_assertions};
use ai_tester::config::{
    load_project_config, mcp_servers_diagnostic, resolve_acp_agent_for_run,
    resolve_mcp_servers_for_run, AcpAgentLaunch, BuiltinAcpAgentProfile, McpServerTransport,
};
use ai_tester::runtime::{
    parse_claude_jsonl, parse_claude_jsonl_with_user_responses, parse_codex_jsonl,
    runtime_status_for_scenario,
};
use ai_tester::sandbox::{create_sandbox, SandboxOptions};
use ai_tester::scenario::{load_scenario_file, AssertionSpec, Fixtures, Scenario, UserResponse};
use ai_tester::skill::allowed_tools::tokenize_allowed_tools;
use ai_tester::skill::parse_skill_md;
use ai_tester::trace::{ToolCallRecord, TraceRecord, Turn};
#[cfg(windows)]
use ai_tester::util::path::strip_windows_verbatim_prefix;
use ai_tester::util::path::{resolve_existing_inside, resolve_write_target_inside};
use ai_tester::util::redaction::Redactor;
use ai_tester::util::regex::compile_pattern;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[test]
fn scenario_parses_minimal_skill_with_defaults() {
    let yaml = "scenario: basic\nskill: aif-commit\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");

    assert_eq!(scenario.scenario, "basic");
    assert_eq!(scenario.skill.as_deref(), Some("aif-commit"));
    assert_eq!(scenario.runner.runtime, "claude");
    assert_eq!(scenario.runner.permission_mode, "bypassPermissions");
    assert!(scenario.assertions.is_empty());
    assert!(scenario.user_responses.is_empty());
    assert!(!scenario.fixtures.git_init);
}

#[test]
fn path_helper_resolves_only_real_paths_inside_sandbox() {
    let tmp = TempDir::new().expect("temp dir");
    fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    fs::write(tmp.path().join("src/lib.rs"), "ok").expect("file written");

    let resolved = resolve_existing_inside(tmp.path(), Path::new("src/../src/lib.rs"))
        .expect("inside path resolves");
    assert_eq!(fs::read_to_string(resolved).expect("read resolved"), "ok");

    let write_target = resolve_write_target_inside(tmp.path(), Path::new("new/child.txt"))
        .expect("new write target resolves");
    assert!(write_target.ends_with("new/child.txt"));

    let err = resolve_existing_inside(tmp.path(), Path::new("../escape.txt"))
        .expect_err("parent escape rejected");
    assert!(err.to_string().contains("escapes sandbox"));
}

#[test]
fn fixtures_env_is_visible_to_setup_commands() {
    let mut env = BTreeMap::new();
    env.insert("AI_TESTER_SETUP_FLAG".to_string(), "fixture".to_string());
    let command = if cfg!(windows) {
        "echo %AI_TESTER_SETUP_FLAG%> flag.txt"
    } else {
        "printf %s \"$AI_TESTER_SETUP_FLAG\" > flag.txt"
    };
    let fixtures = Fixtures {
        setup_commands: vec![command.to_string()],
        env,
        ..Default::default()
    };

    let sandbox =
        create_sandbox("setup-env", &fixtures, SandboxOptions::default()).expect("sandbox creates");
    let flag = fs::read_to_string(sandbox.path.join("flag.txt")).expect("flag written");
    assert_eq!(flag.trim(), "fixture");
    sandbox.cleanup().expect("cleanup succeeds");
}

#[test]
fn fixtures_env_overrides_host_env_for_setup_commands() {
    let key = "AI_TESTER_SETUP_PRECEDENCE_FLAG";
    let old = std::env::var_os(key);
    std::env::set_var(key, "host");
    let mut env = BTreeMap::new();
    env.insert(key.to_string(), "fixture".to_string());
    let command = if cfg!(windows) {
        format!("echo %{key}%> flag.txt")
    } else {
        format!("printf %s \"${key}\" > flag.txt")
    };
    let fixtures = Fixtures {
        setup_commands: vec![command],
        env,
        ..Default::default()
    };

    let sandbox = create_sandbox("setup-env-precedence", &fixtures, SandboxOptions::default())
        .expect("sandbox creates");
    let flag = fs::read_to_string(sandbox.path.join("flag.txt")).expect("flag written");
    assert_eq!(flag.trim(), "fixture");
    sandbox.cleanup().expect("cleanup succeeds");
    if let Some(old) = old {
        std::env::set_var(key, old);
    } else {
        std::env::remove_var(key);
    }
}

#[test]
fn sandbox_drop_removes_directory_when_not_kept() {
    let sandbox = create_sandbox(
        "drop-cleanup",
        &Fixtures::default(),
        SandboxOptions::default(),
    )
    .expect("sandbox creates");
    let path = sandbox.path.clone();

    drop(sandbox);

    assert!(!path.exists(), "sandbox should be removed by Drop");
}

#[test]
fn sandbox_drop_preserves_directory_when_kept() {
    let sandbox = create_sandbox(
        "drop-keep",
        &Fixtures::default(),
        SandboxOptions {
            keep: true,
            ..Default::default()
        },
    )
    .expect("sandbox creates");
    let path = sandbox.path.clone();

    drop(sandbox);

    assert!(path.exists(), "kept sandbox should survive Drop");
    fs::remove_dir_all(path).expect("kept sandbox removed by test cleanup");
}

#[test]
fn fixtures_setup_timeout_rejects_zero() {
    let err = Scenario::from_yaml_str(
        "scenario: timeout-zero\nsystem_prompt: Body\nfixtures:\n  setup_timeout_seconds: 0\n",
    )
    .expect_err("zero timeout rejected");
    assert!(err.to_string().contains("setup_timeout_seconds"));
    assert!(err.to_string().contains("positive"));
}

#[test]
fn fixtures_setup_command_timeout_reports_output_and_cleans_up() {
    for path in temp_ai_tester_dirs("setup-timeout") {
        let _ = fs::remove_dir_all(path);
    }
    let command = if cfg!(windows) {
        "echo setup-out && echo setup-err 1>&2 && ping -n 6 127.0.0.1 > nul"
    } else {
        "echo setup-out; echo setup-err >&2; sleep 5"
    };
    let fixtures = Fixtures {
        setup_commands: vec![command.to_string()],
        ..Default::default()
    };

    let started = Instant::now();
    let err = create_sandbox(
        "setup-timeout",
        &fixtures,
        SandboxOptions {
            setup_timeout: Duration::from_secs(1),
            ..Default::default()
        },
    )
    .expect_err("setup command times out");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "timeout took too long: {:?}",
        started.elapsed()
    );
    let message = err.to_string();
    assert!(message.contains("setup command timed out"));
    assert!(message.contains("timeout 1s"));
    assert!(message.contains(command));
    assert!(message.contains("setup-out"));
    assert!(message.contains("setup-err"));
    assert!(
        !temp_ai_tester_dirs("setup-timeout")
            .into_iter()
            .any(|path| path.exists()),
        "timed out sandbox should be cleaned up"
    );
}

#[test]
fn fixtures_setup_timeout_kills_process_tree() {
    let marker = TempDir::new().expect("marker temp dir");
    let marker_file = marker.path().join("late-marker.txt");
    let marker_text = marker_file.to_string_lossy().replace('\\', "/");
    let command = if cfg!(windows) {
        format!(
            "start /B powershell -NoProfile -Command \"Start-Sleep -Seconds 3; Set-Content -LiteralPath '{marker_text}' -Value late\" & ping -n 6 127.0.0.1 > nul"
        )
    } else {
        format!("(sleep 3; echo late > '{marker_text}') & sleep 5")
    };
    let fixtures = Fixtures {
        setup_commands: vec![command],
        ..Default::default()
    };

    let _ = create_sandbox(
        "setup-tree-timeout",
        &fixtures,
        SandboxOptions {
            setup_timeout: Duration::from_secs(1),
            ..Default::default()
        },
    )
    .expect_err("setup command times out");
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        !marker_file.exists(),
        "process tree child should not create marker after timeout"
    );
}

#[test]
fn fixtures_setup_timeout_seconds_parses() {
    let scenario = Scenario::from_yaml_str(
        "scenario: setup-timeout\nsystem_prompt: Body\nfixtures:\n  setup_timeout_seconds: 7\n",
    )
    .expect("scenario parses");
    assert_eq!(scenario.fixtures.setup_timeout_seconds, Some(7));
}

#[test]
fn runner_acp_turn_timeout_seconds_parses() {
    let scenario = Scenario::from_yaml_str(
        "scenario: acp-turn-timeout\nsystem_prompt: Body\nrunner:\n  runtime: acp\n  acp_turn_timeout_seconds: 45\n",
    )
    .expect("scenario parses");
    assert_eq!(scenario.runner.acp_turn_timeout_seconds, Some(45));
}

#[test]
fn runner_acp_turn_timeout_rejects_zero() {
    let err = Scenario::from_yaml_str(
        "scenario: acp-turn-timeout-zero\nsystem_prompt: Body\nrunner:\n  runtime: acp\n  acp_turn_timeout_seconds: 0\n",
    )
    .expect_err("zero ACP turn timeout rejected");
    assert!(err.to_string().contains("runner.acp_turn_timeout_seconds"));
    assert!(err.to_string().contains("positive"));
}

#[test]
fn scenario_validation_rejects_invalid_assertion_ids_weights_and_limits() {
    let cases = [
        (
            "scenario: duplicate-id\nsystem_prompt: Body\nassertions:\n  - id: same\n    type: output_contains\n    pattern: done\n  - id: same\n    type: no_output_contains\n    pattern: warn\n",
            &["assertions[].id", "same"][..],
        ),
        (
            "scenario: empty-id\nsystem_prompt: Body\nassertions:\n  - id: '   '\n    type: output_contains\n    pattern: done\n",
            &["assertions[0].id", "must not be empty"][..],
        ),
        (
            "scenario: zero-weight\nsystem_prompt: Body\nassertions:\n  - id: zero\n    type: output_contains\n    weight: 0\n    pattern: done\n",
            &["assertions[0].weight", "positive"][..],
        ),
        (
            "scenario: negative-weight\nsystem_prompt: Body\nassertions:\n  - id: negative\n    type: output_contains\n    weight: -1\n    pattern: done\n",
            &["assertions[0].weight", "positive"][..],
        ),
        (
            "scenario: nan-weight\nsystem_prompt: Body\nassertions:\n  - id: nan\n    type: output_contains\n    weight: .nan\n    pattern: done\n",
            &["assertions[0].weight", "finite"][..],
        ),
        (
            "scenario: infinite-weight\nsystem_prompt: Body\nassertions:\n  - id: infinite\n    type: output_contains\n    weight: .inf\n    pattern: done\n",
            &["assertions[0].weight", "finite"][..],
        ),
        (
            "scenario: zero-turn-count\nsystem_prompt: Body\nassertions:\n  - id: turns\n    type: turn_count_at_most\n    max: 0\n",
            &["assertions[0].max", "positive"][..],
        ),
    ];

    for (yaml, expected_parts) in cases {
        let err = Scenario::from_yaml_str(yaml).expect_err("scenario validation rejects case");
        let message = err.to_string();
        for expected in expected_parts {
            assert!(
                message.contains(expected),
                "expected {message:?} to contain {expected:?}"
            );
        }
    }
}

#[test]
fn scenario_validation_rejects_git_fixture_options_without_git_init() {
    let branch_err = Scenario::from_yaml_str(
        "scenario: branch-without-git\nsystem_prompt: Body\nfixtures:\n  git_branch: feature/demo\n",
    )
    .expect_err("git_branch rejected without git_init");
    assert!(branch_err.to_string().contains("fixtures.git_branch"));
    assert!(branch_err.to_string().contains("fixtures.git_init"));

    let staged_err = Scenario::from_yaml_str(
        "scenario: staged-without-git\nsystem_prompt: Body\nfixtures:\n  files_staged:\n    - path: staged.txt\n      content: staged\n",
    )
    .expect_err("files_staged rejected without git_init");
    assert!(staged_err.to_string().contains("fixtures.files_staged"));
    assert!(staged_err.to_string().contains("fixtures.git_init"));
}

#[cfg(windows)]
#[test]
fn path_helper_strips_windows_verbatim_prefix() {
    assert_eq!(
        strip_windows_verbatim_prefix(Path::new(r"\\?\C:\tmp\ai-tester")),
        Path::new(r"C:\tmp\ai-tester")
    );
}

#[cfg(unix)]
#[test]
fn path_helper_rejects_write_through_symlink_outside_sandbox() {
    use std::os::unix::fs::symlink;

    let sandbox = TempDir::new().expect("sandbox temp dir");
    let outside = TempDir::new().expect("outside temp dir");
    symlink(outside.path(), sandbox.path().join("linked")).expect("symlink created");

    let err = resolve_write_target_inside(sandbox.path(), Path::new("linked/escape.txt"))
        .expect_err("symlink escape rejected");
    assert!(err.to_string().contains("escapes sandbox"));
}

fn temp_ai_tester_dirs(scenario_name: &str) -> Vec<std::path::PathBuf> {
    let prefix = format!("ai-tester-{scenario_name}-");
    std::fs::read_dir(std::env::temp_dir())
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect()
}

#[test]
fn scenario_accepts_token_budget_alias_and_rejects_prompt_conflicts() {
    let yaml = "scenario: budget\nskill: aif-commit\ntoken-budget: 123\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("hyphenated token budget parses");
    assert_eq!(scenario.token_budget, Some(123.0));

    let bad = "scenario: conflict\nskill: aif-commit\nsystem_prompt: hi\n";
    let err = Scenario::from_yaml_str(bad).expect_err("multiple prompt sources rejected");
    assert!(err.to_string().contains("exactly one"));
}

#[test]
fn scenario_loader_tracks_explicit_runner_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let scenario_path = tmp.path().join("scenario.yaml");
    std::fs::write(
        &scenario_path,
        "scenario: defaults\nsystem_prompt: Body\nrunner:\n  runtime: codex\n",
    )
    .expect("scenario written");

    let loaded = load_scenario_file(&scenario_path).expect("scenario loads");
    assert!(loaded.source_meta.runner_runtime_set);
    assert!(!loaded.source_meta.runner_model_set);
    assert!(!loaded.source_meta.runner_permission_mode_set);
    assert!(!loaded.source_meta.runner_mode_set);
    assert!(!loaded.source_meta.runner_reasoning_set);

    std::fs::write(
        &scenario_path,
        "scenario: explicit\nsystem_prompt: Body\nrunner:\n  model: explicit-model\n  permission_mode: plan\n  mode: review\n  reasoning: high\n",
    )
    .expect("scenario written");
    let loaded = load_scenario_file(&scenario_path).expect("scenario loads");
    assert!(!loaded.source_meta.runner_runtime_set);
    assert!(loaded.source_meta.runner_model_set);
    assert!(loaded.source_meta.runner_permission_mode_set);
    assert!(loaded.source_meta.runner_mode_set);
    assert!(loaded.source_meta.runner_reasoning_set);
    assert_eq!(loaded.scenario.runner.mode.as_deref(), Some("review"));
    assert_eq!(loaded.scenario.runner.reasoning.as_deref(), Some("high"));

    std::fs::write(
        &scenario_path,
        "scenario: acp-agent\nsystem_prompt: Body\nrunner:\n  runtime: acp\n  agent: local\n",
    )
    .expect("scenario written");
    let loaded = load_scenario_file(&scenario_path).expect("scenario loads");
    assert_eq!(loaded.scenario.runner.agent.as_deref(), Some("local"));
    assert!(loaded.source_meta.runner_runtime_set);
    assert!(loaded.source_meta.runner_agent_set);
}

#[test]
fn allowed_tools_tokenizer_preserves_scopes() {
    let parsed = tokenize_allowed_tools(Some("Bash(git *), Read, mcp__server__tool(scope, two)"));

    assert_eq!(
        parsed.raw,
        vec!["Bash(git *)", "Read", "mcp__server__tool(scope, two)"]
    );
    assert_eq!(parsed.parsed[0].name, "Bash");
    assert_eq!(parsed.parsed[0].scopes, vec!["git *"]);
    assert_eq!(parsed.parsed[2].scopes, vec!["scope", "two"]);
}

#[test]
fn skill_frontmatter_parses_body_hash_and_token_budget() {
    let tmp = TempDir::new().expect("temp dir");
    let skill = tmp.path().join("SKILL.md");
    std::fs::write(
        &skill,
        "---\nname: demo\ndescription: Demo skill\nallowed-tools: Bash(git *), Read\ntoken-budget: 5000\n---\n\nBody text\n",
    )
    .expect("skill written");

    let parsed = parse_skill_md(&skill).expect("skill parses");
    assert_eq!(parsed.frontmatter.name, "demo");
    assert_eq!(parsed.frontmatter.description, "Demo skill");
    assert_eq!(parsed.frontmatter.token_budget, Some(5000.0));
    assert_eq!(parsed.allowed_tools.raw, vec!["Bash(git *)", "Read"]);
    assert_eq!(parsed.body.trim(), "Body text");
    assert_eq!(parsed.body_hash.len(), 64);
}

#[test]
fn project_config_walks_up_and_resolves_skills_dir() {
    let tmp = TempDir::new().expect("temp dir");
    let nested = tmp.path().join("a/b");
    std::fs::create_dir_all(&nested).expect("nested created");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./custom-skills\ndefaults:\n  model: custom\n  permission_mode: plan\n  mode: review\n  reasoning: high\n  setup_timeout_seconds: 12\n",
    )
    .expect("config written");

    let config = load_project_config(&nested).expect("config loads");
    assert_eq!(config.root_dir, tmp.path());
    assert_eq!(config.skills_dir, tmp.path().join("custom-skills"));
    assert_eq!(config.defaults.model.as_deref(), Some("custom"));
    assert_eq!(config.defaults.permission_mode.as_deref(), Some("plan"));
    assert_eq!(config.defaults.mode.as_deref(), Some("review"));
    assert_eq!(config.defaults.reasoning.as_deref(), Some("high"));
    assert_eq!(config.defaults.setup_timeout_seconds, Some(12));
}

#[test]
fn project_config_parses_acp_turn_timeout_default() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "defaults:\n  acp_turn_timeout_seconds: 90\n",
    )
    .expect("config written");

    let config = load_project_config(tmp.path()).expect("config loads");
    assert_eq!(config.defaults.acp_turn_timeout_seconds, Some(90));
}

#[test]
fn project_config_rejects_zero_acp_turn_timeout() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "defaults:\n  acp_turn_timeout_seconds: 0\n",
    )
    .expect("config written");

    let err = load_project_config(tmp.path()).expect_err("zero ACP turn timeout rejected");
    let message = err.to_string();
    assert!(message.contains("defaults.acp_turn_timeout_seconds"));
    assert!(message.contains("positive"));
}

#[test]
fn project_config_rejects_zero_setup_timeout() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "defaults:\n  setup_timeout_seconds: 0\n",
    )
    .expect("config written");

    let err = load_project_config(tmp.path()).expect_err("zero setup timeout rejected");
    let message = err.to_string();
    assert!(message.contains("defaults.setup_timeout_seconds"));
    assert!(message.contains("positive"));
}

#[test]
fn project_config_parses_acp_agent_registry_and_default_agent() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\ndefaults:\n  runtime: acp\n  agent: local\nacp_agents:\n  local:\n    command: fake-acp\n    args: [--stdio]\n    env:\n      ACP_FLAG: \"1\"\n",
    )
    .expect("config written");

    let config = load_project_config(tmp.path()).expect("config loads");

    assert_eq!(config.defaults.runtime.as_deref(), Some("acp"));
    assert_eq!(config.defaults.agent.as_deref(), Some("local"));
    let local = config.acp_agents.get("local").expect("local acp agent");
    assert_eq!(local.command, "fake-acp");
    assert_eq!(local.args, vec!["--stdio"]);
    assert_eq!(local.env.get("ACP_FLAG").map(String::as_str), Some("1"));
}

#[test]
fn acp_agent_resolution_uses_builtins_and_manual_override() {
    let tmp = TempDir::new().expect("temp dir");
    let config = load_project_config(tmp.path()).expect("config loads without file");

    let builtin = resolve_acp_agent_for_run(&config, "gemini").expect("gemini built-in resolves");
    assert_eq!(builtin.name, "gemini");
    assert_eq!(builtin.command(), "npx");
    assert_eq!(
        builtin.args(),
        vec![
            "-y",
            "--",
            "@google/gemini-cli@latest",
            "--experimental-acp"
        ]
    );
    assert!(matches!(
        builtin.launch,
        AcpAgentLaunch::Builtin(BuiltinAcpAgentProfile::Gemini)
    ));

    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  gemini:\n    command: fake-acp\n    args: [--stdio]\n",
    )
    .expect("config written");
    let config = load_project_config(tmp.path()).expect("config loads");
    let manual = resolve_acp_agent_for_run(&config, "gemini").expect("manual gemini resolves");
    assert_eq!(manual.command(), "fake-acp");
    assert_eq!(manual.args(), vec!["--stdio"]);
    assert!(matches!(manual.launch, AcpAgentLaunch::Configured(_)));
}

#[test]
fn acp_agent_resolution_unknown_lists_available_names() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp\n",
    )
    .expect("config written");
    let config = load_project_config(tmp.path()).expect("config loads");
    let err = resolve_acp_agent_for_run(&config, "missing").expect_err("unknown agent rejected");
    let message = err.to_string();
    assert!(message.contains("unknown ACP agent `missing`"));
    assert!(message.contains("local"));
    assert!(message.contains("gemini"));
    assert!(message.contains("zed-claude"));
    assert!(message.contains("zed-codex"));
}

#[test]
fn project_config_parses_mcp_servers_profiles_and_resolves_precedence() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        r#"skills_dir: ./skills
defaults:
  runtime: acp
  agent: local
  mcp_profile: mock
acp_agents:
  local:
    command: fake-acp
mcp_servers:
  codegraph:
    command: project-codegraph
    args: [--project]
    env:
      API_TOKEN: project-secret
  docs:
    type: http
    url: http://127.0.0.1:3001/mcp
    headers:
      Authorization: Bearer project-secret
  events:
    type: sse
    url: http://127.0.0.1:3002/events
mcp_profiles:
  mock:
    servers: [codegraph]
  full:
    servers: [codegraph, docs, events]
"#,
    )
    .expect("config written");

    let config = load_project_config(tmp.path()).expect("config loads");
    assert_eq!(config.defaults.mcp_profile.as_deref(), Some("mock"));
    assert_eq!(
        config
            .mcp_servers
            .get("docs")
            .expect("docs server")
            .transport,
        McpServerTransport::Http
    );

    let scenario = Scenario::from_yaml_str(
        r#"scenario: mcp
system_prompt: Body
runner:
  runtime: acp
  agent: local
  mcp_profile: full
mcp_servers:
  codegraph:
    command: scenario-codegraph
    args: [--scenario-fixture]
    env:
      API_TOKEN: scenario-secret
  scenario_only:
    command: scenario-only
"#,
    )
    .expect("scenario parses");

    let resolved = resolve_mcp_servers_for_run(
        &config,
        &scenario.mcp_servers,
        scenario.runner.mcp_profile.as_deref(),
        None,
    )
    .expect("mcp servers resolve");
    let names = resolved
        .servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["codegraph", "docs", "events", "scenario_only"]);
    assert_eq!(
        resolved.servers[0].config.command.as_deref(),
        Some("scenario-codegraph")
    );

    let cli_resolved = resolve_mcp_servers_for_run(
        &config,
        &scenario.mcp_servers,
        scenario.runner.mcp_profile.as_deref(),
        Some("mock"),
    )
    .expect("cli profile wins");
    let cli_names = cli_resolved
        .servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(cli_names, vec!["codegraph", "scenario_only"]);

    let diagnostic = mcp_servers_diagnostic(&resolved.servers);
    assert!(diagnostic.contains("API_TOKEN"));
    assert!(diagnostic.contains("<redacted>"));
    assert!(!diagnostic.contains("scenario-secret"));
    assert!(!diagnostic.contains("project-secret"));
    assert!(!diagnostic.contains("Bearer project-secret"));
}

#[test]
fn mcp_server_resolution_rejects_unknown_profile_and_missing_required_fields() {
    let tmp = TempDir::new().expect("temp dir");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        r#"mcp_servers:
  missing_command:
    args: [--no-command]
mcp_profiles:
  bad:
    servers: [does_not_exist]
"#,
    )
    .expect("config written");
    let config = load_project_config(tmp.path()).expect("config loads");
    let scenario =
        Scenario::from_yaml_str("scenario: mcp\nsystem_prompt: Body\n").expect("scenario parses");

    let unknown_profile =
        resolve_mcp_servers_for_run(&config, &scenario.mcp_servers, None, Some("unknown"))
            .expect_err("unknown profile rejected");
    assert!(unknown_profile.to_string().contains("unknown MCP profile"));

    let unknown_server =
        resolve_mcp_servers_for_run(&config, &scenario.mcp_servers, None, Some("bad"))
            .expect_err("unknown active server rejected");
    assert!(unknown_server.to_string().contains("unknown MCP server"));

    let missing_command = resolve_mcp_servers_for_run(&config, &scenario.mcp_servers, None, None)
        .expect_err("missing stdio command rejected");
    assert!(missing_command.to_string().contains("requires `command`"));
}

#[test]
fn redactor_removes_json_and_plain_text_secrets() {
    let redactor = Redactor::new(vec![
        "known-secret".to_string(),
        "Bearer configured-secret".to_string(),
    ]);

    let json = redactor.redact_line(
        r#"{"token":"raw-secret","nested":{"api_key":"known-secret"},"env":{"name":"API_TOKEN","value":"known-secret"},"url":"http://127.0.0.1:3001/mcp?token=raw-secret"}"#,
    );
    assert!(json.contains("<redacted>"));
    assert!(!json.contains("raw-secret"));
    assert!(!json.contains("known-secret"));
    assert!(json.contains("http://127.0.0.1:3001/mcp?<redacted>"));

    let plain = redactor.redact_line(
        "Authorization: Bearer configured-secret TOKEN=known-secret password='raw-secret'",
    );
    assert!(plain.contains("<redacted>"));
    assert!(!plain.contains("configured-secret"));
    assert!(!plain.contains("known-secret"));
    assert!(!plain.contains("raw-secret"));
}

#[test]
fn acp_runtime_preflight_accepts_configured_absolute_command_path() {
    let tmp = TempDir::new().expect("temp dir");
    let current_exe = std::env::current_exe().expect("current exe");
    let current_exe = current_exe.to_string_lossy().replace('\\', "/");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        format!(
            "skills_dir: ./skills\nacp_agents:\n  local:\n    command: {current_exe}\n    args: []\n"
        ),
    )
    .expect("config written");

    let config = load_project_config(tmp.path()).expect("config loads");
    let scenario = Scenario::from_yaml_str(
        "scenario: acp-absolute-command\nsystem_prompt: Body\nrunner:\n  runtime: acp\n  agent: local\n",
    )
    .expect("scenario parses");

    let status = runtime_status_for_scenario(&scenario, &config);
    assert!(
        status.ready,
        "unexpected runtime status: {}",
        status.message.unwrap_or_else(|| "no message".to_string())
    );
}

#[test]
fn regex_supports_leading_inline_flags() {
    let re = compile_pattern("(?is)hello.world").expect("regex compiles");
    assert!(re.is_match("HELLO\nworld"));
}

#[test]
fn assertions_evaluate_tool_calls_output_turns_and_token_budget() {
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool("1", "Bash", serde_json::json!({"command": "git status"})),
            Turn::assistant_with_tool(
                "2",
                "Bash",
                serde_json::json!({"command": "git commit -m feat"}),
            ),
        ],
        "Commit created: feat(auth): add login".to_string(),
        5,
        Some(100.0),
    );

    let assertions = vec![
        AssertionSpec::tool_called(
            "calls-status",
            "Bash",
            serde_json::json!({"command": "^git status"}),
        ),
        AssertionSpec::no_tool_called("no-write", "Write"),
        AssertionSpec::output_contains("mentions-feat", "\\bfeat\\b"),
        AssertionSpec::no_output_contains("no-regression", "panic|error"),
        AssertionSpec::turn_count_at_most("efficient", 6),
    ];

    let results = evaluate_assertions(&assertions, &trace);
    assert!(results.iter().all(|r| r.pass), "{results:#?}");
    assert_eq!(compute_weighted_score(&results), 1.0);
}

#[test]
fn assertions_evaluate_sandbox_files_json_and_commands() {
    let tmp = TempDir::new().expect("temp dir");
    fs::write(tmp.path().join("result.txt"), "status=done\n").expect("result written");
    fs::write(
        tmp.path().join("config.json"),
        r#"{"feature":{"enabled":true},"plugins":["core","audit"]}"#,
    )
    .expect("json written");

    let mut trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);
    trace.runner.sandbox_path = Some(tmp.path().display().to_string());
    let command = if cfg!(windows) {
        "echo ok"
    } else {
        "printf ok"
    };
    let assertions = vec![
        AssertionSpec::FileContains {
            id: "file-has-status".to_string(),
            weight: 1.0,
            path: "result.txt".to_string(),
            pattern: "status=done".to_string(),
        },
        AssertionSpec::FileNotContains {
            id: "file-no-secret".to_string(),
            weight: 1.0,
            path: "result.txt".to_string(),
            pattern: "secret".to_string(),
        },
        AssertionSpec::JsonValid {
            id: "json-valid".to_string(),
            weight: 1.0,
            path: "config.json".to_string(),
        },
        AssertionSpec::JsonPathEquals {
            id: "json-path".to_string(),
            weight: 1.0,
            path: "config.json".to_string(),
            json_path: "feature.enabled".to_string(),
            value: serde_json::json!(true),
        },
        AssertionSpec::FileNotExists {
            id: "no-extra-file".to_string(),
            weight: 1.0,
            path: "extra.txt".to_string(),
        },
        AssertionSpec::CommandOutputContains {
            id: "command-output".to_string(),
            weight: 1.0,
            command: command.to_string(),
            pattern: "ok".to_string(),
            timeout_seconds: Some(5),
        },
    ];

    let results = evaluate_assertions(&assertions, &trace);
    for id in [
        "file-has-status",
        "file-no-secret",
        "json-valid",
        "json-path",
        "no-extra-file",
        "command-output",
        "no_unanswered_questions",
    ] {
        assert!(
            results.iter().any(|result| result.id == id && result.pass),
            "{id} should pass: {results:#?}"
        );
    }
    assert_eq!(compute_weighted_score(&results), 1.0);
}

#[test]
fn assertions_report_safety_violations_even_when_correctness_passes() {
    let tmp = TempDir::new().expect("temp dir");
    let mut trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "fs/write_text_file",
                serde_json::json!({"path": "../escape.txt", "content": "leak"}),
            ),
            Turn::assistant_with_tool("2", "Bash", serde_json::json!({"command": "printf ok"})),
        ],
        "correct output".to_string(),
        2,
        None,
    );
    trace.runner.sandbox_path = Some(tmp.path().display().to_string());
    let assertions = vec![
        AssertionSpec::output_contains("correctness-passes", "correct output"),
        AssertionSpec::NoPathEscape {
            id: "stay-in-sandbox".to_string(),
            weight: 1.0,
            tools: None,
            allow_outside: None,
        },
        AssertionSpec::NoToolCalled {
            id: "no-shell".to_string(),
            weight: 1.0,
            tool: Some("Bash".to_string()),
            tool_pattern: None,
            tool_kind: None,
            title_pattern: None,
            args_match: None,
            raw_input_match: None,
        },
    ];

    let results = evaluate_assertions(&assertions, &trace);
    let correctness = results
        .iter()
        .find(|result| result.id == "correctness-passes")
        .expect("correctness assertion");
    assert!(correctness.pass, "{results:#?}");

    let path_escape = results
        .iter()
        .find(|result| result.id == "stay-in-sandbox")
        .expect("path escape assertion");
    assert!(!path_escape.pass, "{results:#?}");
    assert!(path_escape.detail.contains("path escape detected"));
    assert!(path_escape.detail.contains("../escape.txt"));

    let forbidden_tool = results
        .iter()
        .find(|result| result.id == "no-shell")
        .expect("forbidden tool assertion");
    assert!(!forbidden_tool.pass, "{results:#?}");
    assert!(forbidden_tool
        .detail
        .contains("unexpected `Bash` call matched"));
}

#[test]
fn assertions_report_json_file_and_command_failures_without_panicking() {
    let tmp = TempDir::new().expect("temp dir");
    fs::write(tmp.path().join("invalid.json"), "{not-json").expect("invalid json written");

    let mut trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);
    trace.runner.sandbox_path = Some(tmp.path().display().to_string());

    let failing_command = if cfg!(windows) {
        "echo stdout && echo stderr 1>&2 && exit /b 7"
    } else {
        "printf stdout; printf stderr >&2; exit 7"
    };
    let timeout_command = if cfg!(windows) {
        "ping -n 3 127.0.0.1 > nul"
    } else {
        "sleep 2"
    };
    let assertions = vec![
        AssertionSpec::JsonValid {
            id: "invalid-json".to_string(),
            weight: 1.0,
            path: "invalid.json".to_string(),
        },
        AssertionSpec::FileContains {
            id: "missing-file".to_string(),
            weight: 1.0,
            path: "missing.txt".to_string(),
            pattern: "expected".to_string(),
        },
        AssertionSpec::CommandSucceeds {
            id: "command-exit-code".to_string(),
            weight: 1.0,
            command: failing_command.to_string(),
            timeout_seconds: Some(5),
        },
        AssertionSpec::CommandOutputContains {
            id: "command-timeout".to_string(),
            weight: 1.0,
            command: timeout_command.to_string(),
            pattern: "never".to_string(),
            timeout_seconds: Some(0),
        },
    ];

    let results = evaluate_assertions(&assertions, &trace);
    for id in [
        "invalid-json",
        "missing-file",
        "command-exit-code",
        "command-timeout",
    ] {
        let result = results
            .iter()
            .find(|result| result.id == id)
            .expect("assertion result");
        assert!(!result.pass, "{id}: {results:#?}");
    }

    let invalid_json = results
        .iter()
        .find(|result| result.id == "invalid-json")
        .expect("invalid json result");
    assert!(invalid_json.detail.contains("parse JSON `invalid.json`"));

    let missing_file = results
        .iter()
        .find(|result| result.id == "missing-file")
        .expect("missing file result");
    assert!(missing_file
        .detail
        .contains("resolve `missing.txt` inside sandbox"));

    let exit_code = results
        .iter()
        .find(|result| result.id == "command-exit-code")
        .expect("exit code result");
    assert!(exit_code.detail.contains("command failed"));
    assert!(exit_code.detail.contains("stdout"));
    assert!(exit_code.detail.contains("stderr"));

    let timeout = results
        .iter()
        .find(|result| result.id == "command-timeout")
        .expect("timeout result");
    assert!(timeout.detail.contains("command timed out after 0s"));
}

#[test]
fn command_output_contains_matches_stderr_as_well_as_stdout() {
    let tmp = TempDir::new().expect("temp dir");
    let mut trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);
    trace.runner.sandbox_path = Some(tmp.path().display().to_string());
    let command = if cfg!(windows) {
        "echo stderr-only 1>&2"
    } else {
        "printf stderr-only >&2"
    };

    let results = evaluate_assertions(
        &[AssertionSpec::CommandOutputContains {
            id: "stderr-match".to_string(),
            weight: 1.0,
            command: command.to_string(),
            pattern: "stderr-only".to_string(),
            timeout_seconds: Some(5),
        }],
        &trace,
    );

    let result = results
        .iter()
        .find(|result| result.id == "stderr-match")
        .expect("stderr assertion");
    assert!(result.pass, "{results:#?}");
}

#[test]
fn tool_called_accepts_tool_pattern() {
    let yaml = "scenario: pattern\nsystem_prompt: Body\nassertions:\n  - id: codegraph\n    type: tool_called\n    tool_pattern: '^mcp__.*__codegraph_context$'\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "mcp__ai_workspace__codegraph_context",
            serde_json::json!({"task": "find bug"}),
        )],
        "done".to_string(),
        1,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "codegraph")
        .expect("assertion result");
    assert!(result.pass, "{results:#?}");
}

#[test]
fn tool_called_capture_records_matched_field() {
    let yaml = "scenario: capture-command\nsystem_prompt: Body\nassertions:\n  - id: calls-status\n    type: tool_called\n    tool: Bash\n    args_match:\n      command: '^git status'\n    capture: [command]\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "Bash",
            serde_json::json!({"command": "git status --short"}),
        )],
        "done".to_string(),
        1,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "calls-status")
        .expect("assertion result");
    assert!(result.pass, "{results:#?}");
    assert_eq!(result.captures.len(), 1);
    assert_eq!(result.captures[0].field, "command");
    assert_eq!(result.captures[0].value, "git status --short");
    assert!(!result.captures[0].truncated);
    assert_eq!(result.captures[0].original_length, 18);
    assert_eq!(result.captures[0].step, None);
}

#[test]
fn args_match_supports_nested_dot_paths_json_pointer_and_missing_paths() {
    let yaml = "scenario: nested-args\nsystem_prompt: Body\nassertions:\n  - id: top-level-command\n    type: tool_called\n    tool: Bash\n    args_match:\n      command: '^git status$'\n  - id: nested-dot-command\n    type: tool_called\n    tool: Bash\n    args_match:\n      rawInput.command: '^cargo test$'\n  - id: nested-pointer-command\n    type: tool_called\n    tool: Bash\n    args_match:\n      /rawInput/command: '^cargo test$'\n  - id: acp-location\n    type: tool_called\n    tool: execute\n    args_match:\n      _acpLocations.0.path: 'src/main.rs'\n  - id: missing-path-empty\n    type: tool_called\n    tool: Bash\n    args_match:\n      rawInput.missing: '^$'\n  - id: missing-path-non-empty\n    type: tool_called\n    tool: Bash\n    args_match:\n      rawInput.missing: 'required'\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "Bash",
                serde_json::json!({
                    "command": "git status",
                    "rawInput": { "command": "cargo test" }
                }),
            ),
            Turn::assistant_with_tool(
                "2",
                "execute",
                serde_json::json!({
                    "_acpLocations": [{ "path": "src/main.rs" }],
                    "command": "cargo test"
                }),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    for id in [
        "top-level-command",
        "nested-dot-command",
        "nested-pointer-command",
        "acp-location",
        "missing-path-empty",
    ] {
        let result = results
            .iter()
            .find(|result| result.id == id)
            .expect("assertion result");
        assert!(result.pass, "{id}: {results:#?}");
    }
    let missing_non_empty = results
        .iter()
        .find(|result| result.id == "missing-path-non-empty")
        .expect("missing non-empty result");
    assert!(!missing_non_empty.pass, "{results:#?}");
}

#[test]
fn acp_friendly_matchers_match_tool_kind_title_and_raw_input() {
    let yaml = "scenario: acp-friendly\nsystem_prompt: Body\nassertions:\n  - id: acp-flattened\n    type: tool_called\n    tool_kind: execute\n    title_pattern: '^Run tests$'\n    raw_input_match:\n      command: '^cargo test$'\n  - id: acp-wrapped\n    type: tool_called\n    tool_kind: execute\n    title_pattern: '^Run wrapped$'\n    raw_input_match:\n      command: '^cargo nextest$'\n  - id: no-rm-command\n    type: no_tool_called\n    tool_kind: execute\n    raw_input_match:\n      command: '^rm -rf'\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "execute",
                serde_json::json!({
                    "command": "cargo test",
                    "_acpKind": "execute",
                    "_acpTitle": "Run tests"
                }),
            ),
            Turn::assistant_with_tool(
                "2",
                "execute",
                serde_json::json!({
                    "rawInput": { "command": "cargo nextest" },
                    "_acpKind": "execute",
                    "_acpTitle": "Run wrapped"
                }),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    for id in ["acp-flattened", "acp-wrapped", "no-rm-command"] {
        let result = results
            .iter()
            .find(|result| result.id == id)
            .expect("assertion result");
        assert!(result.pass, "{id}: {results:#?}");
    }
}

#[test]
fn invalid_regex_nested_args_and_raw_input_match_report_field() {
    let yaml = "scenario: invalid-nested-matchers\nsystem_prompt: Body\nassertions:\n  - id: invalid-nested-args\n    type: tool_called\n    tool: Bash\n    args_match:\n      rawInput.command: '['\n  - id: invalid-raw-input\n    type: tool_called\n    tool_kind: execute\n    raw_input_match:\n      command: '['\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let nested_args = results
        .iter()
        .find(|result| result.id == "invalid-nested-args")
        .expect("nested args result");
    assert!(!nested_args.pass, "{results:#?}");
    assert!(nested_args.detail.contains("invalid args_match regex"));
    assert!(nested_args.detail.contains("rawInput.command"));
    assert!(nested_args.detail.contains("["));

    let raw_input = results
        .iter()
        .find(|result| result.id == "invalid-raw-input")
        .expect("raw input result");
    assert!(!raw_input.pass, "{results:#?}");
    assert!(raw_input.detail.contains("invalid raw_input_match regex"));
    assert!(raw_input.detail.contains("command"));
    assert!(raw_input.detail.contains("["));
}

#[test]
fn tool_call_sequence_supports_acp_friendly_steps() {
    let yaml = "scenario: acp-sequence\nsystem_prompt: Body\nassertions:\n  - id: tests-before-status\n    type: tool_call_sequence\n    sequence:\n      - tool_kind: execute\n        title_pattern: '^Run tests$'\n        raw_input_match:\n          command: '^cargo test$'\n      - tool: Bash\n        args_match:\n          command: '^git status$'\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "execute",
                serde_json::json!({
                    "command": "cargo test",
                    "_acpKind": "execute",
                    "_acpTitle": "Run tests"
                }),
            ),
            Turn::assistant_with_tool("2", "Bash", serde_json::json!({"command": "git status"})),
        ],
        "done".to_string(),
        2,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "tests-before-status")
        .expect("sequence result");
    assert!(result.pass, "{results:#?}");
}

#[test]
fn tool_called_capture_truncates_and_uses_selected_call_index() {
    let yaml = "scenario: capture-index\nsystem_prompt: Body\nassertions:\n  - id: second-bash\n    type: tool_called\n    tool: Bash\n    call_index: 1\n    capture: [command]\n    capture_max_chars: 8\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool("1", "Bash", serde_json::json!({"command": "git status"})),
            Turn::assistant_with_tool(
                "2",
                "Bash",
                serde_json::json!({"command": "echo привет мир"}),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "second-bash")
        .expect("assertion result");
    assert!(result.pass, "{results:#?}");
    assert_eq!(result.captures.len(), 1);
    assert_eq!(result.captures[0].value, "echo при");
    assert!(result.captures[0].truncated);
    assert_eq!(result.captures[0].original_length, 15);
}

#[test]
fn tool_call_sequence_capture_records_step_fields_and_missing_values() {
    let yaml = "scenario: sequence-capture\nsystem_prompt: Body\nassertions:\n  - id: status-before-commit\n    type: tool_call_sequence\n    capture_max_chars: 0\n    sequence:\n      - tool: Bash\n        args_match:\n          command: '^git status'\n        capture: [command, missing]\n      - tool: Bash\n        args_match:\n          command: '^git commit'\n        capture: [command]\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "Bash",
                serde_json::json!({"command": "git status --short"}),
            ),
            Turn::assistant_with_tool(
                "2",
                "Bash",
                serde_json::json!({"command": "git commit -m feat"}),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "status-before-commit")
        .expect("assertion result");
    assert!(result.pass, "{results:#?}");
    assert_eq!(result.captures.len(), 3);
    assert_eq!(result.captures[0].field, "command");
    assert_eq!(result.captures[0].value, "");
    assert!(result.captures[0].truncated);
    assert_eq!(result.captures[0].original_length, 18);
    assert_eq!(result.captures[0].step, Some(1));
    assert_eq!(result.captures[1].field, "missing");
    assert_eq!(result.captures[1].value, "");
    assert!(!result.captures[1].truncated);
    assert_eq!(result.captures[1].original_length, 0);
    assert_eq!(result.captures[1].step, Some(1));
    assert_eq!(result.captures[2].step, Some(2));
}

#[test]
fn invalid_regex_no_tool_called_tool_pattern_fails_assertion() {
    let yaml = "scenario: invalid-pattern\nsystem_prompt: Body\nassertions:\n  - id: no-invalid-pattern\n    type: no_tool_called\n    tool_pattern: '['\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "no-invalid-pattern")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
    assert!(result.detail.contains("invalid tool_pattern regex"));
    assert!(result.detail.contains("["));
}

#[test]
fn invalid_regex_no_tool_called_args_match_fails_assertion() {
    let yaml = "scenario: invalid-args\nsystem_prompt: Body\nassertions:\n  - id: no-invalid-args\n    type: no_tool_called\n    tool: Bash\n    args_match:\n      command: '['\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "no-invalid-args")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
    assert!(result.detail.contains("invalid args_match regex"));
    assert!(result.detail.contains("command"));
    assert!(result.detail.contains("["));
}

#[test]
fn invalid_regex_tool_called_args_match_reports_diagnostic() {
    let yaml = "scenario: invalid-tool-called-args\nsystem_prompt: Body\nassertions:\n  - id: call-invalid-args\n    type: tool_called\n    tool: Bash\n    args_match:\n      command: '['\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(Vec::new(), "done".to_string(), 1, None);

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "call-invalid-args")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
    assert!(result.detail.contains("invalid args_match regex"));
    assert!(result.detail.contains("command"));
    assert!(result.detail.contains("["));
    assert!(!result.detail.contains("no `Bash` call matched"));
}

#[test]
fn valid_no_tool_called_tool_pattern_matches_calls() {
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "Bash",
            serde_json::json!({"command": "git status"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let assertions = vec![
        AssertionSpec::NoToolCalled {
            id: "no-write-pattern".to_string(),
            weight: 1.0,
            tool: None,
            tool_pattern: Some("^Write$".to_string()),
            tool_kind: None,
            title_pattern: None,
            args_match: None,
            raw_input_match: None,
        },
        AssertionSpec::NoToolCalled {
            id: "no-bash-pattern".to_string(),
            weight: 1.0,
            tool: None,
            tool_pattern: Some("^Ba.*$".to_string()),
            tool_kind: None,
            title_pattern: None,
            args_match: None,
            raw_input_match: None,
        },
    ];

    let results = evaluate_assertions(&assertions, &trace);
    let no_write = results
        .iter()
        .find(|result| result.id == "no-write-pattern")
        .expect("no-write result");
    let no_bash = results
        .iter()
        .find(|result| result.id == "no-bash-pattern")
        .expect("no-bash result");
    assert!(no_write.pass, "{results:#?}");
    assert!(!no_bash.pass, "{results:#?}");
}

#[test]
fn no_output_contains_fails_when_final_output_matches() {
    let trace = TraceRecord::synthetic(Vec::new(), "WARN [+check] failed".to_string(), 1, None);
    let assertions = vec![AssertionSpec::no_output_contains(
        "no-check-warning",
        "WARN \\[\\+check\\]",
    )];

    let results = evaluate_assertions(&assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "no-check-warning")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
    assert_eq!(result.kind, "no_output_contains");
}

#[test]
fn file_read_matches_claude_read_and_codex_bash_readers() {
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let claude_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "read-1",
            "Read",
            serde_json::json!({"file_path": "src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let claude_results = evaluate_assertions(&assertions, &claude_trace);
    assert!(
        claude_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{claude_results:#?}"
    );

    let codex_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "bash-1",
            "Bash",
            serde_json::json!({"command": "sed -n '1,220p' src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let codex_results = evaluate_assertions(&assertions, &codex_trace);
    assert!(
        codex_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{codex_results:#?}"
    );

    let acp_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "acp-read-1",
            "fs/read_text_file",
            serde_json::json!({"path": "src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let acp_results = evaluate_assertions(&assertions, &acp_trace);
    assert!(
        acp_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{acp_results:#?}"
    );
}

#[test]
fn file_read_matches_acp_normalized_read_path_sources() {
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let cases = [
        (
            "direct path",
            Turn::assistant_with_tool(
                "acp-read-direct",
                "read",
                serde_json::json!({"path": "src/runtime/mod.rs"}),
            ),
        ),
        (
            "direct file_path",
            Turn::assistant_with_tool(
                "acp-read-file-path",
                "read",
                serde_json::json!({"file_path": "src/runtime/mod.rs"}),
            ),
        ),
        (
            "rawInput path",
            Turn::assistant_with_tool(
                "acp-read-raw-path",
                "read",
                serde_json::json!({"rawInput": {"path": "src/runtime/mod.rs"}}),
            ),
        ),
        (
            "rawInput file_path",
            Turn::assistant_with_tool(
                "acp-read-raw-file-path",
                "read",
                serde_json::json!({"rawInput": {"file_path": "src/runtime/mod.rs"}}),
            ),
        ),
        (
            "_acpLocations path",
            Turn::assistant_with_tool(
                "acp-read-location-path",
                "read",
                serde_json::json!({"_acpLocations": [{"path": "src/runtime/mod.rs"}]}),
            ),
        ),
        (
            "_acpLocations uri",
            Turn::assistant_with_tool(
                "acp-read-location-uri",
                "read",
                serde_json::json!({"_acpLocations": [{"uri": "file:///repo/src/runtime/mod.rs"}]}),
            ),
        ),
    ];

    for (label, turn) in cases {
        let trace = TraceRecord::synthetic(vec![turn], "done".to_string(), 1, None);
        let results = evaluate_assertions(&assertions, &trace);
        assert!(
            results
                .iter()
                .any(|result| result.id == "reads-runtime" && result.pass),
            "{label}: {results:#?}"
        );
    }
}

#[test]
fn file_read_matches_acp_execute_reader_commands_only() {
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let reader_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "acp-exec-reader",
            "execute",
            serde_json::json!({"rawInput": {"command": "sed -n '1,80p' src/runtime/mod.rs"}}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let reader_results = evaluate_assertions(&assertions, &reader_trace);
    assert!(
        reader_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{reader_results:#?}"
    );

    let non_reader_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "acp-exec-non-reader",
            "execute",
            serde_json::json!({"rawInput": {"command": "cargo test src/runtime/mod.rs"}}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let non_reader_results = evaluate_assertions(&assertions, &non_reader_trace);
    let non_reader = non_reader_results
        .iter()
        .find(|result| result.id == "reads-runtime")
        .expect("file_read result");
    assert!(!non_reader.pass, "{non_reader_results:#?}");

    let edit_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "acp-edit",
            "edit",
            serde_json::json!({"path": "src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let edit_results = evaluate_assertions(&assertions, &edit_trace);
    let edit = edit_results
        .iter()
        .find(|result| result.id == "reads-runtime")
        .expect("file_read result");
    assert!(!edit.pass, "{edit_results:#?}");
}

#[test]
fn file_read_does_not_match_non_reader_bash_mentions() {
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "bash-1",
            "Bash",
            serde_json::json!({"command": "cargo test src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let results = evaluate_assertions(&assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "reads-runtime")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
}

#[test]
fn assertions_report_failures_for_unwanted_tool_and_unanswered_questions() {
    let mut trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "Write",
                serde_json::json!({"file_path": "../escape.txt"}),
            ),
            Turn::assistant_with_tool(
                "2",
                "AskUserQuestion",
                serde_json::json!({"question": "Proceed?"}),
            ),
        ],
        "done".to_string(),
        10,
        None,
    );
    trace.tool_call_summary.unanswered_questions = 1;

    let assertions = vec![
        AssertionSpec::no_tool_called("no-write", "Write"),
        AssertionSpec::turn_count_at_most("efficient", 6),
    ];

    let results = evaluate_assertions(&assertions, &trace);
    let failed: Vec<_> = results
        .iter()
        .filter(|r| !r.pass)
        .map(|r| r.id.as_str())
        .collect();
    assert!(failed.contains(&"no-write"));
    assert!(failed.contains(&"efficient"));
    assert!(failed.contains(&"no_unanswered_questions"));
}

#[test]
fn no_path_escape_inspects_tool_path_inputs_against_sandbox() {
    let tmp = TempDir::new().expect("temp dir");
    let mut trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool("1", "Write", serde_json::json!({"file_path": "src/lib.rs"})),
            Turn::assistant_with_tool(
                "2",
                "Read",
                serde_json::json!({"file_path": "../escape.txt"}),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );
    trace.runner.sandbox_path = Some(tmp.path().to_string_lossy().to_string());

    let assertions = vec![AssertionSpec::NoPathEscape {
        id: "paths-stay-inside".to_string(),
        weight: 1.0,
        tools: None,
        allow_outside: None,
    }];
    let results = evaluate_assertions(&assertions, &trace);

    let path_result = results
        .iter()
        .find(|result| result.id == "paths-stay-inside")
        .expect("path assertion exists");
    assert!(!path_result.pass);
    assert!(path_result.detail.contains("../escape.txt"));
}

#[test]
fn no_path_escape_inspects_acp_fs_and_terminal_paths() {
    let tmp = TempDir::new().expect("temp dir");
    fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    fs::write(tmp.path().join("src/lib.rs"), "ok").expect("file written");
    let inside_file = tmp.path().join("src/lib.rs");

    let mut inside_trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "fs/read_text_file",
                serde_json::json!({"path": inside_file.display().to_string()}),
            ),
            Turn::assistant_with_tool(
                "2",
                "fs/write_text_file",
                serde_json::json!({"path": "generated/output.txt"}),
            ),
            Turn::assistant_with_tool(
                "3",
                "terminal/create",
                serde_json::json!({"cwd": tmp.path().display().to_string(), "command": "sh"}),
            ),
        ],
        "done".to_string(),
        1,
        None,
    );
    inside_trace.runner.sandbox_path = Some(tmp.path().to_string_lossy().to_string());

    let assertions = vec![AssertionSpec::NoPathEscape {
        id: "paths-stay-inside".to_string(),
        weight: 1.0,
        tools: None,
        allow_outside: None,
    }];
    let inside_results = evaluate_assertions(&assertions, &inside_trace);
    assert!(
        inside_results
            .iter()
            .any(|result| result.id == "paths-stay-inside" && result.pass),
        "{inside_results:#?}"
    );

    let outside_path = tmp.path().parent().unwrap().join("outside.txt");
    let mut outside_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "4",
            "fs/read_text_file",
            serde_json::json!({"path": outside_path.display().to_string()}),
        )],
        "done".to_string(),
        1,
        None,
    );
    outside_trace.runner.sandbox_path = Some(tmp.path().to_string_lossy().to_string());

    let outside_results = evaluate_assertions(&assertions, &outside_trace);
    let result = outside_results
        .iter()
        .find(|result| result.id == "paths-stay-inside")
        .expect("assertion result");
    assert!(!result.pass, "{outside_results:#?}");
    assert!(result.detail.contains("fs/read_text_file.path"));
}

#[test]
fn trace_serializes_with_v2_schema_version() {
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "Bash",
            serde_json::json!({"command": "git status"}),
        )],
        "ok".to_string(),
        1,
        None,
    );
    let json = serde_json::to_value(&trace).expect("trace serializes");

    assert_eq!(json["schemaVersion"], "2.0.0");
    assert_eq!(json["toolCallSummary"]["byTool"]["Bash"], 1);
}

#[test]
fn synthetic_tool_call_records_result_defaults() {
    let call = ToolCallRecord::new("abc", "Bash", serde_json::json!({"command": "git status"}));

    assert_eq!(call.id, "abc");
    assert_eq!(call.name, "Bash");
    assert!(!call.result_is_error);
    assert!(call.result_content.is_none());
}

#[test]
fn codex_jsonl_parser_normalizes_shell_and_message_events() {
    let jsonl = r#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"turn.started"}
{"type":"item.started","item":{"type":"command_execution","id":"cmd-1","command":"git status"}}
{"type":"item.completed","item":{"type":"command_execution","id":"cmd-1","status":"completed","aggregated_output":"clean"}}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2,"cached_input_tokens":3}}
"#;

    let parsed = parse_codex_jsonl(jsonl, 40, false).expect("codex jsonl parses");
    assert_eq!(parsed.session_id.as_deref(), Some("thread-1"));
    assert_eq!(parsed.final_output, "done");
    assert_eq!(parsed.turns[0].tool_calls[0].name, "Bash");
    assert_eq!(
        parsed.turns[0].tool_calls[0].result_content.as_deref(),
        Some("clean")
    );
    assert_eq!(parsed.cost.input_tokens, 10);
    assert_eq!(parsed.cost.cache_read_tokens, 3);
}

#[test]
fn claude_jsonl_parser_normalizes_tool_results_and_usage() {
    let jsonl = r#"{"type":"system","subtype":"init","session_id":"session-1"}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"git status"}},{"type":"text","text":"hello"}],"usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"clean","is_error":false}]}}
{"type":"result","subtype":"success","result":"done","usage":{"input_tokens":3,"output_tokens":4,"cache_creation_input_tokens":5,"cache_read_input_tokens":6},"total_cost_usd":0.01}
"#;

    let parsed = parse_claude_jsonl(jsonl, 12, true).expect("claude jsonl parses");
    assert_eq!(parsed.session_id.as_deref(), Some("session-1"));
    assert_eq!(parsed.final_output, "done");
    assert_eq!(parsed.turns[0].text_deltas, vec!["hello"]);
    assert_eq!(
        parsed.turns[0].tool_calls[0].result_content.as_deref(),
        Some("clean")
    );
    assert_eq!(parsed.cost.input_tokens, 3);
    assert_eq!(parsed.cost.cache_creation_tokens, 5);
    assert_eq!(parsed.cost.usd_estimate, 0.01);
    assert!(parsed.max_turns_user_set);
}

#[test]
fn claude_jsonl_parser_does_not_mark_subprocess_questions_answered() {
    let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q-1","name":"AskUserQuestion","input":{"question":"Proceed with commit?"}}]}}
{"type":"result","subtype":"success","result":"done"}
"#;

    let unanswered =
        parse_claude_jsonl_with_user_responses(jsonl, 12, false, &[]).expect("jsonl parses");
    assert_eq!(unanswered.unanswered_questions, 1);
    assert!(unanswered.turns[0].tool_calls[0].answered.is_none());

    let unsupported = parse_claude_jsonl_with_user_responses(
        jsonl,
        12,
        false,
        &[UserResponse {
            match_question: "Proceed".to_string(),
            choose: "Yes".to_string(),
        }],
    )
    .expect("jsonl parses");
    assert_eq!(unsupported.unanswered_questions, 1);
    assert!(unsupported.turns[0].tool_calls[0].answered.is_none());
    assert!(unsupported
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("cannot deliver user_responses")));
}
