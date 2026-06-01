use std::fs;
use std::path::Path;

use ai_tester::assertions::AssertionResult;
use ai_tester::sandbox::{create_sandbox, SandboxOptions};
use ai_tester::scenario::{FixtureFile, Fixtures};
use ai_tester::trace::{
    write_trace, ToolCallRecord, ToolCallSummary, TraceError, TraceRecord, Turn,
};
use assert_cmd::Command;
use chrono::{DateTime, Utc};
use predicates::prelude::*;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn sandbox_writes_fixtures_and_rejects_path_escape() {
    let fixtures = Fixtures {
        files_unstaged: vec![FixtureFile {
            path: "nested/TODO.md".to_string(),
            content: Some("- audit\n".to_string()),
            content_from: None,
        }],
        ..Default::default()
    };

    let sandbox = create_sandbox("writes-files", &fixtures, SandboxOptions::default())
        .expect("sandbox creates");
    let todo = fs::read_to_string(sandbox.path.join("nested/TODO.md")).expect("fixture exists");
    assert_eq!(todo, "- audit\n");
    sandbox.cleanup().expect("cleanup succeeds");
    assert!(!sandbox.path.exists());

    let escaping = Fixtures {
        files_unstaged: vec![FixtureFile {
            path: "../escape.txt".to_string(),
            content: Some("nope".to_string()),
            content_from: None,
        }],
        ..Default::default()
    };
    let err = create_sandbox("escape", &escaping, SandboxOptions::default()).unwrap_err();
    assert!(err.to_string().contains("escapes sandbox"));
}

#[test]
fn sandbox_git_init_creates_baseline_commit_and_branch() {
    let fixtures = Fixtures {
        git_init: true,
        git_branch: Some("feature/demo".to_string()),
        files_committed: vec![FixtureFile {
            path: "README.md".to_string(),
            content: Some("# Demo\n".to_string()),
            content_from: None,
        }],
        files_staged: vec![FixtureFile {
            path: "src/lib.rs".to_string(),
            content: Some("pub fn demo() {}\n".to_string()),
            content_from: None,
        }],
        ..Default::default()
    };

    let sandbox = create_sandbox("git-baseline", &fixtures, SandboxOptions::default())
        .expect("git sandbox creates");
    assert!(sandbox.path.join(".git").exists());
    assert_eq!(
        git_output(&sandbox.path, &["branch", "--show-current"]),
        "feature/demo"
    );
    assert!(git_output(&sandbox.path, &["status", "--short"]).contains("A  src/lib.rs"));
    sandbox.cleanup().expect("cleanup succeeds");
}

#[test]
fn cli_init_writes_project_config() {
    let tmp = TempDir::new().expect("temp dir");
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args([
            "init",
            "--skills-dir",
            "./my-skills",
            "--model",
            "test-model",
            "--permission-mode",
            "plan",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(".ai-tester.yaml"));

    let config = fs::read_to_string(tmp.path().join(".ai-tester.yaml")).expect("config written");
    assert!(config.contains("skills_dir: ./my-skills"));
    assert!(config.contains("model: test-model"));
    assert!(config.contains("permission_mode: plan"));
}

#[test]
fn cli_init_with_builtin_acp_agent_writes_minimal_template() {
    let tmp = TempDir::new().expect("temp dir");
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["init", "--acp-agent", "gemini"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".ai-tester.yaml"));

    let config = fs::read_to_string(tmp.path().join(".ai-tester.yaml")).expect("config written");
    assert!(config.contains("runtime: acp"));
    assert!(config.contains("agent: gemini"));
    assert!(config.contains("permission_mode: bypassPermissions"));
    assert!(!config.contains("acp_agents:"));
    assert!(!config.contains("model:"));
}

#[test]
fn cli_run_dry_run_loads_file_without_creating_runtime_sandbox() {
    let tmp = TempDir::new().expect("temp dir");
    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: inline-demo\nsystem_prompt: You are helpful.\nassertions: []\n",
    )
    .expect("scenario written");

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["run", "--file", scenario.to_str().unwrap(), "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("inline-demo"))
        .stdout(predicate::str::contains("OK"));
}

#[test]
fn cli_run_file_accepts_scenario_path_without_yaml_extension() {
    let tmp = TempDir::new().expect("temp dir");
    let prompts = tmp.path().join("prompts");
    fs::create_dir_all(&prompts).expect("prompts dir");
    fs::write(
        prompts.join("audit-ai-tester.yaml"),
        "scenario: extensionless-file\nsystem_prompt: You are helpful.\nassertions: []\n",
    )
    .expect("scenario written");

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["run", "--file", "prompts/audit-ai-tester", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("extensionless-file"))
        .stdout(predicate::str::contains("OK"));
}

#[test]
fn cli_run_dry_run_loads_standalone_scenario_dir() {
    let tmp = TempDir::new().expect("temp dir");
    let prompts = tmp.path().join("prompts");
    fs::create_dir_all(&prompts).expect("prompts dir");
    fs::write(
        prompts.join("audit.yaml"),
        "scenario: prompt-audit\nsystem_prompt: You are helpful.\nassertions: []\n",
    )
    .expect("scenario written");
    fs::write(
        prompts.join("_draft.yaml"),
        "scenario: skipped\nsystem_prompt: You are helpful.\nassertions: []\n",
    )
    .expect("draft written");

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["run", "--dir", "prompts", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prompt-audit"))
        .stdout(predicate::str::contains("scenarios  1"))
        .stdout(predicate::str::contains("skipped").not());
}

#[test]
fn cli_trend_filters_limits_and_emits_json() {
    let tmp = TempDir::new().expect("temp dir");
    let old = write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "demo-old",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );
    let new = write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "demo-new",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-02T10:00:00Z",
            pass: false,
            score: Some(0.5),
            tool: "Bash",
        },
    );
    let _other = write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "other-skill",
            skill: "other",
            scenario: "smoke",
            finished_at: "2026-05-03T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );
    assert!(old.exists());
    assert!(new.exists());

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    let output = cmd
        .current_dir(tmp.path())
        .args(["trend", "demo", "--scenario", "smoke", "--last", "2"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8 stdout");
    assert!(stdout.contains("ai-tester trend"));
    assert!(stdout.contains("demo-old"));
    assert!(stdout.contains("demo-new"));
    assert!(stdout.find("demo-old") < stdout.find("demo-new"));
    assert!(!stdout.contains("other-skill"));

    let mut json_cmd = Command::cargo_bin("ai-tester").expect("binary");
    json_cmd
        .current_dir(tmp.path())
        .args(["trend", "demo", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runId\": \"demo-new\""))
        .stdout(predicate::str::contains("\"weightedScore\": 0.5"));
}

#[test]
fn cli_trace_prints_summary_and_raw_json() {
    let tmp = TempDir::new().expect("temp dir");
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "trace-target",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: false,
            score: Some(0.5),
            tool: "Bash",
        },
    );

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["trace", "trace-target"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ai-tester trace"))
        .stdout(predicate::str::contains("demo/smoke"))
        .stdout(predicate::str::contains("Assertions"))
        .stdout(predicate::str::contains("Tool calls"))
        .stdout(predicate::str::contains("Turns"))
        .stdout(predicate::str::contains("final output for trace-target"));

    let mut json_cmd = Command::cargo_bin("ai-tester").expect("binary");
    json_cmd
        .current_dir(tmp.path())
        .args(["trace", "trace-target", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runId\": \"trace-target\""))
        .stdout(predicate::str::contains("\"schemaVersion\": \"2.0.0\""));
}

#[test]
fn cli_compare_prints_deltas_and_json() {
    let tmp = TempDir::new().expect("temp dir");
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "compare-a",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "compare-b",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-02T10:00:00Z",
            pass: false,
            score: Some(0.5),
            tool: "Bash",
        },
    );

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["compare", "compare-a", "compare-b"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ai-tester compare"))
        .stdout(predicate::str::contains("compare-a"))
        .stdout(predicate::str::contains("compare-b"))
        .stdout(predicate::str::contains("score"))
        .stdout(predicate::str::contains("Assertions"))
        .stdout(predicate::str::contains("Tool calls"))
        .stdout(predicate::str::contains("errors"));

    let mut json_cmd = Command::cargo_bin("ai-tester").expect("binary");
    json_cmd
        .current_dir(tmp.path())
        .args(["compare", "compare-a", "compare-b", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runA\""))
        .stdout(predicate::str::contains("\"scoreDelta\": -0.5"))
        .stdout(predicate::str::contains("\"assertionChanges\""));
}

#[test]
fn cli_trace_and_compare_missing_run_return_config_error() {
    let tmp = TempDir::new().expect("temp dir");

    let mut trace_cmd = Command::cargo_bin("ai-tester").expect("binary");
    trace_cmd
        .current_dir(tmp.path())
        .args(["trace", "missing"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("No runs/ directory found"));

    let mut compare_cmd = Command::cargo_bin("ai-tester").expect("binary");
    compare_cmd
        .current_dir(tmp.path())
        .args(["compare", "missing-a", "missing-b"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("No runs/ directory found"));
}

#[test]
fn cli_history_and_trend_skip_invalid_and_non_v2_traces() {
    let tmp = TempDir::new().expect("temp dir");
    let runs_dir = tmp.path().join("runs/demo");
    fs::create_dir_all(&runs_dir).expect("runs dir");
    fs::write(runs_dir.join("broken.json"), "{not json").expect("invalid json written");
    fs::write(runs_dir.join("unreadable-utf8.json"), [0xff, 0xfe, 0xfd])
        .expect("non-utf8 json written");
    fs::write(
        runs_dir.join("old-schema.json"),
        "{\"schemaVersion\":\"1.0.0\",\"runId\":\"old-schema\"}\n",
    )
    .expect("old trace written");
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "valid-v2",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );

    let mut history_cmd = Command::cargo_bin("ai-tester").expect("binary");
    history_cmd
        .current_dir(tmp.path())
        .args(["history", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runId\": \"valid-v2\""))
        .stdout(predicate::str::contains("old-schema").not());

    let mut trend_cmd = Command::cargo_bin("ai-tester").expect("binary");
    trend_cmd
        .current_dir(tmp.path())
        .args(["trend", "demo", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runId\": \"valid-v2\""))
        .stdout(predicate::str::contains("old-schema").not());
}

#[test]
fn cli_trace_and_compare_accept_trace_file_paths() {
    let tmp = TempDir::new().expect("temp dir");
    let trace_a = write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "path-a",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );
    let trace_b = write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "path-b",
            skill: "demo",
            scenario: "smoke",
            finished_at: "2026-05-02T10:00:00Z",
            pass: false,
            score: Some(0.5),
            tool: "Bash",
        },
    );

    let mut trace_cmd = Command::cargo_bin("ai-tester").expect("binary");
    trace_cmd
        .current_dir(tmp.path())
        .args(["trace", trace_a.to_str().expect("utf8 trace path")])
        .assert()
        .success()
        .stdout(predicate::str::contains("path-a"));

    let mut compare_cmd = Command::cargo_bin("ai-tester").expect("binary");
    compare_cmd
        .current_dir(tmp.path())
        .args([
            "compare",
            trace_a.to_str().expect("utf8 trace path"),
            trace_b.to_str().expect("utf8 trace path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("path-a"))
        .stdout(predicate::str::contains("path-b"));
}

#[test]
fn cli_trace_reports_ambiguous_run_id() {
    let tmp = TempDir::new().expect("temp dir");
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "duplicate-run",
            skill: "demo-a",
            scenario: "smoke",
            finished_at: "2026-05-01T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );
    write_named_trace(
        tmp.path(),
        TraceSeed {
            run_id: "duplicate-run",
            skill: "demo-b",
            scenario: "smoke",
            finished_at: "2026-05-02T10:00:00Z",
            pass: true,
            score: Some(1.0),
            tool: "Read",
        },
    );

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["trace", "duplicate-run"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("ambiguous trace id"))
        .stdout(predicate::str::contains("duplicate-run"));
}

#[test]
fn cli_trend_last_zero_uses_default_limit() {
    let tmp = TempDir::new().expect("temp dir");
    for index in 0..21 {
        let run_id = format!("series-{index:02}");
        let finished_at = format!("2026-05-{day:02}T10:00:00Z", day = index + 1);
        write_named_trace(
            tmp.path(),
            TraceSeed {
                run_id: &run_id,
                skill: "demo",
                scenario: "smoke",
                finished_at: &finished_at,
                pass: true,
                score: Some(1.0),
                tool: "Read",
            },
        );
    }

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    let output = cmd
        .current_dir(tmp.path())
        .args(["trend", "demo", "--last", "0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8 stdout");
    assert!(!stdout.contains("series-00"));
    assert!(stdout.contains("series-01"));
    assert!(stdout.contains("series-20"));
}

#[test]
fn cli_history_reads_v2_traces_from_runs_dir() {
    let tmp = TempDir::new().expect("temp dir");
    let trace = TraceRecord::synthetic(Vec::new(), "ok".to_string(), 2, None);
    let path = write_trace(tmp.path().join("runs"), &trace).expect("trace written");
    assert!(path.exists());

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["history", "--last", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ai-tester history"))
        .stdout(predicate::str::contains("synthetic/synthetic"));

    let mut json_cmd = Command::cargo_bin("ai-tester").expect("binary");
    json_cmd
        .current_dir(tmp.path())
        .args(["history", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"runId\""));
}

#[test]
fn cli_run_with_fake_codex_writes_trace_and_evaluates_assertions() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_codex(&bin_dir);

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-codex\nsystem_prompt: You are helpful.\nrunner:\n  runtime: codex\n  model: fake-model\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--runtime",
            "codex",
            "--quiet",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_fake-codex"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    assert!(trace_json.contains("\"schemaVersion\": \"2.0.0\""));
    assert!(trace_json.contains("\"overallPass\": true"));
}

#[test]
fn cli_run_with_fake_acp_writes_trace_and_evaluates_assertions() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp(&bin_dir, false);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n  - id: ran-command\n    type: tool_called\n    tool: execute\n    args_match:\n      command: \"cargo test\"\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--runtime",
            "acp",
            "--agent",
            "local",
            "--quiet",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_fake-acp"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    let trace: serde_json::Value = serde_json::from_str(&trace_json).expect("trace json");
    assert_eq!(trace["finalOutput"], "done");
    assert_eq!(trace["toolCallSummary"]["byTool"]["execute"], 1);
    assert_eq!(
        trace["turns"][0]["toolCalls"][0]["input"]["_acpTitle"],
        "Run tests"
    );
}

#[test]
fn cli_run_with_fake_acp_permission_request_does_not_hang() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp(&bin_dir, true);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-permission\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\n  permission_mode: plan\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));
}

#[test]
fn cli_run_with_fake_acp_stops_before_prompt_past_max_turns() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp(&bin_dir, false);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-max-turns\nsystem_prompt: You are helpful.\nmax_turns: 1\nuser_prompts:\n  - first\n  - second\nrunner:\n  runtime: acp\n  agent: local\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_fake-acp-max-turns"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    let trace: serde_json::Value = serde_json::from_str(&trace_json).expect("trace json");
    assert_eq!(trace["finalOutput"], "done");
    assert_eq!(trace["runner"]["turnsUsed"], 1);
    assert_eq!(trace["runner"]["hitMaxTurns"], true);
}

#[test]
fn cli_run_with_fake_acp_client_capabilities_logs_fs_and_terminal_operations() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_client_capabilities(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp-capabilities\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-capabilities\nsystem_prompt: You are helpful.\nfixtures:\n  files_committed:\n    - path: notes/input.txt\n      content: \"hello from sandbox\\n\"\nrunner:\n  runtime: acp\n  agent: local\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n  - id: paths-stay-inside\n    type: no_path_escape\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--quiet",
            "--idle-warn",
            "3",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_fake-acp-capabilities"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    let trace: serde_json::Value = serde_json::from_str(&trace_json).expect("trace json");
    assert_eq!(trace["finalOutput"], "done");
    assert_eq!(trace["toolCallSummary"]["byTool"]["fs/read_text_file"], 1);
    assert_eq!(trace["toolCallSummary"]["byTool"]["fs/write_text_file"], 1);
    assert_eq!(trace["toolCallSummary"]["byTool"]["terminal/create"], 1);
    assert_eq!(
        trace["toolCallSummary"]["byTool"]["terminal/wait_for_exit"],
        1
    );
    assert_eq!(trace["toolCallSummary"]["byTool"]["terminal/kill"], 1);
    assert!(trace_json.contains("\"_acpResolvedPath\""));
    assert!(trace_json.contains("\"_acpResolvedCwd\""));
    assert!(trace_json.contains("\"signal\": \"timeout\""));
}

#[test]
fn cli_run_with_fake_acp_forwards_mcp_profiles_and_redacts_trace() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_mcp_forwarding(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        r#"skills_dir: ./skills
defaults:
  runtime: acp
  agent: local
  mcp_profile: mock
acp_agents:
  local:
    command: fake-acp-mcp
    args: []
mcp_servers:
  codegraph:
    command: project-codegraph
    args: [--project]
    env:
      API_TOKEN: project-secret
  docs:
    type: http
    url: http://127.0.0.1:3001/project
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
    mcp_servers:
      docs:
        type: http
        url: http://127.0.0.1:3001/profile
        headers:
          Authorization: Bearer profile-secret
"#,
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        r#"scenario: fake-acp-mcp
system_prompt: You are helpful.
runner:
  runtime: acp
  agent: local
mcp_servers:
  codegraph:
    command: scenario-codegraph
    args: [--scenario-fixture]
    env:
      API_TOKEN: scenario-secret
  scenario_only:
    command: scenario-only
assertions:
  - id: says-done
    type: output_contains
    pattern: done
"#,
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);

    let mut default_profile = Command::cargo_bin("ai-tester").expect("binary");
    default_profile
        .current_dir(tmp.path())
        .env("PATH", &new_path)
        .env("EXPECTED_MCP_PROFILE", "mock")
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let mut full_profile = Command::cargo_bin("ai-tester").expect("binary");
    full_profile
        .current_dir(tmp.path())
        .env("PATH", &new_path)
        .env("EXPECTED_MCP_PROFILE", "full")
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--quiet",
            "--mcp-profile",
            "full",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let trace_dir = tmp.path().join("runs/inline_fake-acp-mcp");
    let trace_json = fs::read_dir(&trace_dir)
        .expect("trace dir")
        .map(|entry| fs::read_to_string(entry.expect("entry").path()).expect("trace readable"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(trace_json.contains("ACP MCP servers"));
    assert!(trace_json.contains("<redacted>"));
    assert!(!trace_json.contains("scenario-secret"));
    assert!(!trace_json.contains("project-secret"));
    assert!(!trace_json.contains("profile-secret"));
    assert!(!trace_json.contains("Bearer profile-secret"));
}

#[test]
fn cli_run_with_fake_acp_negotiates_model_mode_reasoning_and_traces() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_config_negotiation(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp-config\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-config\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--quiet",
            "--model",
            "gpt-5-codex",
            "--mode",
            "plan",
            "--reasoning",
            "high",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_fake-acp-config"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    assert!(trace_json.contains("ACP effective config"));
    assert!(trace_json.contains("gpt-5-codex"));
    assert!(trace_json.contains("plan"));
    assert!(trace_json.contains("high"));
}

#[test]
fn cli_run_with_fake_acp_rejects_unsupported_explicit_model() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_config_negotiation(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp-config\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-unsupported-model\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\nassertions: []\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--model",
            "unsupported",
        ])
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "unsupported ACP model `unsupported`",
        ));
}

#[test]
fn cli_run_with_fake_acp_writes_redacted_transcript() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    let log_dir = tmp.path().join("acp-logs");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_transcript(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        r#"skills_dir: ./skills
acp_agents:
  local:
    command: fake-acp-transcript
    args: []
    env:
      ACP_TOKEN: acp-agent-secret
mcp_servers:
  codegraph:
    command: mock-codegraph
    env:
      API_TOKEN: mcp-secret
"#,
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-transcript\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--quiet",
            "--acp-log",
            log_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let transcript_files = fs::read_dir(&log_dir)
        .expect("log dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("log entries");
    assert_eq!(transcript_files.len(), 1);
    let transcript = fs::read_to_string(transcript_files[0].path()).expect("transcript readable");
    assert!(transcript.contains(r#""direction":"stdin""#));
    assert!(transcript.contains(r#""direction":"stdout""#));
    assert!(transcript.contains(r#""direction":"stderr""#));
    assert!(transcript.contains("initialize"));
    assert!(transcript.contains("session/new"));
    assert!(transcript.contains("<redacted>"));
    assert!(!transcript.contains("acp-agent-secret"));
    assert!(!transcript.contains("mcp-secret"));
    assert!(!transcript.contains("stderr-secret"));
}

#[test]
fn cli_run_with_fake_acp_invalid_stdout_reports_transcript_path() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    let log_dir = tmp.path().join("acp-logs");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp_invalid_stdout(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp-invalid\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-acp-invalid\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\n  agent: local\nassertions: []\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--acp-log",
            log_dir.to_str().unwrap(),
            "--idle-warn",
            "1",
        ])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("ACP transcript"))
        .stdout(predicate::str::contains(".acp.jsonl"));

    let transcript_files = fs::read_dir(&log_dir)
        .expect("log dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("log entries");
    assert_eq!(transcript_files.len(), 1);
    let transcript = fs::read_to_string(transcript_files[0].path()).expect("transcript readable");
    assert!(transcript.contains("not-json"));
    assert!(transcript.contains("<redacted>"));
    assert!(!transcript.contains("invalid-secret"));
    assert!(!transcript.contains("bad-secret"));
}

#[test]
fn cli_run_acp_requires_configured_agent() {
    let tmp = TempDir::new().expect("temp dir");
    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: missing-acp-agent\nsystem_prompt: You are helpful.\nrunner:\n  runtime: acp\nassertions: []\n",
    )
    .expect("scenario written");

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("runner.agent"));
}

#[test]
fn cli_runtimes_lists_configured_acp_agents() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_acp(&bin_dir, false);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp\n    args: []\n",
    )
    .expect("config written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["runtimes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acp:local"))
        .stdout(predicate::str::contains("ready"));
}

#[test]
fn cli_runtimes_lists_builtin_acp_agents() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_npx_acp(&bin_dir);

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["runtimes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acp:gemini"))
        .stdout(predicate::str::contains("@google/gemini-cli@latest"))
        .stdout(predicate::str::contains("acp:zed-claude"))
        .stdout(predicate::str::contains(
            "@zed-industries/claude-code-acp@latest",
        ))
        .stdout(predicate::str::contains("acp:zed-codex"))
        .stdout(predicate::str::contains("@zed-industries/codex-acp@latest"))
        .stdout(predicate::str::contains("ready"));
}

#[test]
fn cli_run_uses_builtin_gemini_without_acp_agents_block() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    let npx_args = tmp.path().join("npx-args.txt");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_npx_acp(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\ndefaults:\n  runtime: acp\n  agent: gemini\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: builtin-gemini\nsystem_prompt: You are helpful.\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .env("FAKE_NPX_ARGS_OUT", &npx_args)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let args = fs::read_to_string(npx_args).expect("npx args captured");
    assert!(args.contains("-y"));
    assert!(args.contains("--"));
    assert!(args.contains("@google/gemini-cli@latest"));
    assert!(args.contains("--experimental-acp"));
}

#[test]
fn cli_run_manual_acp_agent_overrides_builtin_name() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    let npx_args = tmp.path().join("npx-args.txt");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_npx_acp(&bin_dir);
    write_fake_acp(&bin_dir, false);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\ndefaults:\n  runtime: acp\n  agent: gemini\nacp_agents:\n  gemini:\n    command: fake-acp\n    args: []\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: manual-overrides-gemini\nsystem_prompt: You are helpful.\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .env("FAKE_NPX_ARGS_OUT", &npx_args)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    assert!(
        !npx_args.exists(),
        "manual override should not invoke built-in npx"
    );
}

#[test]
fn cli_run_with_fake_codex_prints_live_progress() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_codex(&bin_dir);

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: fake-codex-progress\nsystem_prompt: You are helpful.\nrunner:\n  runtime: codex\n  model: fake-model\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("[1/1] fake-codex-progress"))
        .stdout(predicate::str::contains("  progress"))
        .stdout(predicate::str::contains("[turn] started"))
        .stdout(predicate::str::contains("[assistant] message completed"))
        .stdout(predicate::str::contains("● PASS"));
}

#[test]
fn cli_run_fails_when_explicit_max_turns_is_hit() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_codex_two_turns(&bin_dir);

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: turn-budget\nsystem_prompt: You are helpful.\nmax_turns: 1\nrunner:\n  runtime: codex\n  model: fake-model\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("turn_budget"))
        .stdout(predicate::str::contains("FAIL"));
}

#[test]
fn cli_run_applies_project_defaults_to_omitted_runner_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_codex(&bin_dir);
    fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./skills\ndefaults:\n  runtime: codex\n  model: config-model\n  permission_mode: plan\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: config-defaults\nsystem_prompt: You are helpful.\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    let traces = fs::read_dir(tmp.path().join("runs/inline_config-defaults"))
        .expect("trace dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("trace entries");
    assert_eq!(traces.len(), 1);
    let trace_json = fs::read_to_string(traces[0].path()).expect("trace readable");
    let trace: serde_json::Value = serde_json::from_str(&trace_json).expect("trace json");
    assert_eq!(trace["runner"]["model"], "config-model");
    assert_eq!(trace["runner"]["permissionMode"], "plan");
}

#[test]
fn cli_run_with_fake_claude_question_does_not_false_pass_user_responses() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_claude_question(&bin_dir);

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: claude-question\nsystem_prompt: You are helpful.\nrunner:\n  runtime: claude\n  model: fake-model\nuser_responses:\n  - match_question: Proceed\n    choose: Yes\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario written");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "--file", scenario.to_str().unwrap(), "--quiet"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("no_unanswered_questions"))
        .stdout(predicate::str::contains("FAIL"));
}

#[test]
fn cli_run_discovers_skill_scenarios_and_installs_skill_in_sandbox() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_codex(&bin_dir);

    fs::write(tmp.path().join(".ai-tester.yaml"), "skills_dir: ./skills\n").expect("config");
    let skill_dir = tmp.path().join("skills/demo");
    fs::create_dir_all(skill_dir.join("tests")).expect("skill dirs");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\nallowed-tools: Bash(git *), Read\n---\n\nDemo body\n",
    )
    .expect("skill");
    fs::write(
        skill_dir.join("tests/basic.yaml"),
        "scenario: fake-skill\nskill: demo\nrunner:\n  runtime: codex\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
    )
    .expect("scenario");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = join_path_prefix(&bin_dir, &old_path);
    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .env("PATH", new_path)
        .args(["run", "demo", "--runtime", "codex", "--quiet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));

    assert!(tmp.path().join("runs/demo").is_dir());
}

fn git_output(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_fake_codex(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let path = bin_dir.join("codex.cmd");
        fs::write(
            path,
            "@echo off\r\n\
:check_args\r\n\
if \"%~1\"==\"\" goto after_args\r\n\
if \"%~1\"==\"-a\" (echo unexpected legacy approval flag 1>&2 && exit /b 2)\r\n\
shift\r\n\
goto check_args\r\n\
:after_args\r\n\
echo {\"type\":\"thread.started\",\"thread_id\":\"fake-thread\"}\r\n\
echo {\"type\":\"turn.started\"}\r\n\
echo {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}\r\n\
echo {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"cached_input_tokens\":0}}\r\n",
        )
        .expect("fake codex written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("codex");
        fs::write(
            &path,
            "#!/bin/sh\n\
case \" $* \" in *\" -a \"*) echo 'unexpected legacy approval flag' >&2; exit 2;; esac\n\
cat >/dev/null\n\
echo '{\"type\":\"thread.started\",\"thread_id\":\"fake-thread\"}'\n\
echo '{\"type\":\"turn.started\"}'\n\
echo '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}'\n\
echo '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"cached_input_tokens\":0}}'\n",
        )
        .expect("fake codex written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_codex_two_turns(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let path = bin_dir.join("codex.cmd");
        fs::write(
            path,
            "@echo off\r\n\
:check_args\r\n\
if \"%~1\"==\"\" goto after_args\r\n\
if \"%~1\"==\"-a\" (echo unexpected legacy approval flag 1>&2 && exit /b 2)\r\n\
shift\r\n\
goto check_args\r\n\
:after_args\r\n\
echo {\"type\":\"thread.started\",\"thread_id\":\"fake-thread\"}\r\n\
echo {\"type\":\"turn.started\"}\r\n\
echo {\"type\":\"turn.started\"}\r\n\
echo {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}\r\n\
echo {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"cached_input_tokens\":0}}\r\n",
        )
        .expect("fake codex written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("codex");
        fs::write(
            &path,
            "#!/bin/sh\n\
case \" $* \" in *\" -a \"*) echo 'unexpected legacy approval flag' >&2; exit 2;; esac\n\
cat >/dev/null\n\
echo '{\"type\":\"thread.started\",\"thread_id\":\"fake-thread\"}'\n\
echo '{\"type\":\"turn.started\"}'\n\
echo '{\"type\":\"turn.started\"}'\n\
echo '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}'\n\
echo '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"cached_input_tokens\":0}}'\n",
        )
        .expect("fake codex written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_claude_question(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let path = bin_dir.join("claude.cmd");
        fs::write(
            path,
            "@echo off\r\n\
echo {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"q-1\",\"name\":\"AskUserQuestion\",\"input\":{\"question\":\"Proceed with commit?\"}}]}}\r\n\
echo {\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}\r\n",
        )
        .expect("fake claude written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("claude");
        fs::write(
            &path,
            "#!/bin/sh\n\
echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"q-1\",\"name\":\"AskUserQuestion\",\"input\":{\"question\":\"Proceed with commit?\"}}]}}'\n\
echo '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}'\n",
        )
        .expect("fake claude written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp(bin_dir: &Path, request_permission: bool) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp.cmd");
        let ps1_path = bin_dir.join("fake-acp.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp.ps1\"\r\n",
        )
        .expect("fake acp wrapper written");
        let permission_script = if request_permission {
            r#"
        Write-Json @{
            jsonrpc = "2.0"
            id = "perm-1"
            method = "session/request_permission"
            params = @{
                sessionId = "s1"
                toolCall = @{
                    toolCallId = "tool-1"
                    title = "Run tests"
                    kind = "execute"
                    rawInput = @{ command = "cargo test" }
                }
                options = @(
                    @{ optionId = "allow"; name = "Allow"; kind = "allow_once" },
                    @{ optionId = "reject"; name = "Reject"; kind = "reject_once" }
                )
            }
        }
        [Console]::In.ReadLine() | Out-Null
"#
        } else {
            ""
        };
        fs::write(
            ps1_path,
            format!(
                r#"
function Write-Json($value) {{
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}}

while ($null -ne ($line = [Console]::In.ReadLine())) {{
    if ([string]::IsNullOrWhiteSpace($line)) {{
        continue
    }}
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {{
        Write-Json @{{
            jsonrpc = "2.0"
            id = $message.id
            result = @{{
                protocolVersion = 1
                agentCapabilities = @{{}}
                authMethods = @()
                agentInfo = @{{ name = "fake-acp"; version = "1.0.0" }}
            }}
        }}
    }} elseif ($message.method -eq "session/new") {{
        Write-Json @{{
            jsonrpc = "2.0"
            id = $message.id
            result = @{{
                sessionId = "s1"
                configOptions = @()
            }}
        }}
    }} elseif ($message.method -eq "session/prompt") {{
{permission_script}
        Write-Json @{{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{{
                sessionId = "s1"
                update = @{{
                    sessionUpdate = "tool_call"
                    toolCallId = "tool-1"
                    title = "Run tests"
                    kind = "execute"
                    status = "in_progress"
                    rawInput = @{{ command = "cargo test" }}
                }}
            }}
        }}
        Write-Json @{{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{{
                sessionId = "s1"
                update = @{{
                    sessionUpdate = "tool_call_update"
                    toolCallId = "tool-1"
                    status = "completed"
                    rawOutput = @{{ stdout = "clean" }}
                    content = @(@{{ type = "content"; content = @{{ type = "text"; text = "clean" }} }})
                }}
            }}
        }}
        Write-Json @{{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{{
                sessionId = "s1"
                update = @{{
                    sessionUpdate = "agent_message_chunk"
                    content = @{{ type = "text"; text = "done" }}
                }}
            }}
        }}
        Write-Json @{{
            jsonrpc = "2.0"
            id = $message.id
            result = @{{ stopReason = "end_turn" }}
        }}
    }} elseif ($message.method -eq "session/close") {{
        Write-Json @{{
            jsonrpc = "2.0"
            id = $message.id
            result = @{{}}
        }}
        exit 0
    }}
}}
"#
            ),
        )
        .expect("fake acp script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp");
        let permission_script = if request_permission {
            r#"
    echo '{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"tool-1","title":"Run tests","kind":"execute","rawInput":{"command":"cargo test"}},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}}'
    read -r ignored
"#
        } else {
            ""
        };
        fs::write(
            &path,
            format!(
                r#"#!/bin/sh
while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{}},\"authMethods\":[],\"agentInfo\":{{\"name\":\"fake-acp\",\"version\":\"1.0.0\"}}}}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"sessionId\":\"s1\",\"configOptions\":[]}}}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
{permission_script}
      echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Run tests","kind":"execute","status":"in_progress","rawInput":{{"command":"cargo test"}}}}}}}}'
      echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed","rawOutput":{{"stdout":"clean"}},"content":[{{"type":"content","content":{{"type":"text","text":"clean"}}}}]}}}}}}'
      echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"done"}}}}}}}}'
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"stopReason\":\"end_turn\"}}}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{}}}}"
      exit 0
      ;;
  esac
done
"#,
            ),
        )
        .expect("fake acp written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_npx_acp(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("npx.cmd");
        let ps1_path = bin_dir.join("npx.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0npx.ps1\" %*\r\n",
        )
        .expect("fake npx wrapper written");
        fs::write(
            ps1_path,
            r#"
if ($env:FAKE_NPX_ARGS_OUT) {
    [System.IO.File]::WriteAllText($env:FAKE_NPX_ARGS_OUT, ($args -join "`n"))
}

function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                protocolVersion = 1
                agentCapabilities = @{}
                authMethods = @()
                agentInfo = @{ name = "fake-npx-acp"; version = "1.0.0" }
            }
        }
    } elseif ($message.method -eq "session/new") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ sessionId = "s1"; configOptions = @() }
        }
    } elseif ($message.method -eq "session/prompt") {
        Write-Json @{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{
                sessionId = "s1"
                update = @{
                    sessionUpdate = "agent_message_chunk"
                    content = @{ type = "text"; text = "done" }
                }
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ stopReason = "end_turn" }
        }
    } elseif ($message.method -eq "session/close") {
        Write-Json @{ jsonrpc = "2.0"; id = $message.id; result = @{} }
        exit 0
    }
}
"#,
        )
        .expect("fake npx script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("npx");
        fs::write(
            &path,
            r#"#!/bin/sh
if [ -n "$FAKE_NPX_ARGS_OUT" ]; then
  printf '%s\n' "$@" > "$FAKE_NPX_ARGS_OUT"
fi
while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{},\"authMethods\":[],\"agentInfo\":{\"name\":\"fake-npx-acp\",\"version\":\"1.0.0\"}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"s1\",\"configOptions\":[]}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("fake npx written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp_client_capabilities(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp-capabilities.cmd");
        let ps1_path = bin_dir.join("fake-acp-capabilities.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-capabilities.ps1\"\r\n",
        )
        .expect("fake acp capabilities wrapper written");
        fs::write(
            ps1_path,
            r#"
function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}

function Send-Request($id, $method, $params) {
    Write-Json @{
        jsonrpc = "2.0"
        id = $id
        method = $method
        params = $params
    }
    $responseLine = [Console]::In.ReadLine()
    if ([string]::IsNullOrWhiteSpace($responseLine)) {
        return $null
    }
    return $responseLine | ConvertFrom-Json
}

$capsOk = $false
$sessionCwd = $null
while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {
        $caps = $message.params.clientCapabilities
        $capsOk = [bool]($caps.fs.readTextFile -and $caps.fs.writeTextFile -and $caps.terminal)
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                protocolVersion = 1
                agentCapabilities = @{}
                authMethods = @()
                agentInfo = @{ name = "fake-acp-capabilities"; version = "1.0.0" }
            }
        }
    } elseif ($message.method -eq "session/new") {
        $sessionCwd = [string]$message.params.cwd
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                sessionId = "s1"
                configOptions = @()
            }
        }
    } elseif ($message.method -eq "session/prompt") {
        if ($capsOk) {
            $inputPath = Join-Path $sessionCwd "notes/input.txt"
            $outputPath = Join-Path $sessionCwd "notes/output.txt"
            $read = Send-Request "fs-read-1" "fs/read_text_file" @{
                sessionId = "s1"
                path = $inputPath
                line = 1
                limit = 1
            }
            $content = if ($null -ne $read.result.content) { $read.result.content } else { "missing" }
            $null = Send-Request "fs-write-1" "fs/write_text_file" @{
                sessionId = "s1"
                path = $outputPath
                content = $content
            }
            $created = Send-Request "term-create-1" "terminal/create" @{
                sessionId = "s1"
                command = "powershell"
                args = @("-NoProfile", "-Command", "Start-Sleep -Seconds 5")
                cwd = $sessionCwd
                outputByteLimit = 1024
            }
            $terminalId = [string]$created.result.terminalId
            $null = Send-Request "term-output-1" "terminal/output" @{
                sessionId = "s1"
                terminalId = $terminalId
            }
            $null = Send-Request "term-wait-1" "terminal/wait_for_exit" @{
                sessionId = "s1"
                terminalId = $terminalId
            }
            $null = Send-Request "term-kill-1" "terminal/kill" @{
                sessionId = "s1"
                terminalId = $terminalId
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{
                sessionId = "s1"
                update = @{
                    sessionUpdate = "agent_message_chunk"
                    content = @{ type = "text"; text = "done" }
                }
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ stopReason = "end_turn" }
        }
    } elseif ($message.method -eq "session/close") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{}
        }
        exit 0
    }
}
"#,
        )
        .expect("fake acp capabilities script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp-capabilities");
        fs::write(
            &path,
            r#"#!/bin/sh
caps_ok=0
session_cwd=""

send_request() {
  printf '%s\n' "$1"
  IFS= read -r response
}

while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      case "$line" in
        *'"readTextFile":true*'"writeTextFile":true*'"terminal":true*) caps_ok=1 ;;
        *) caps_ok=0 ;;
      esac
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{},\"authMethods\":[],\"agentInfo\":{\"name\":\"fake-acp-capabilities\",\"version\":\"1.0.0\"}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      session_cwd=$(printf '%s' "$line" | sed -n 's/.*"cwd":"\([^"]*\)".*/\1/p')
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"s1\",\"configOptions\":[]}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
      if [ "$caps_ok" = "1" ]; then
        input_path="$session_cwd/notes/input.txt"
        output_path="$session_cwd/notes/output.txt"
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"fs-read-1\",\"method\":\"fs/read_text_file\",\"params\":{\"sessionId\":\"s1\",\"path\":\"$input_path\",\"line\":1,\"limit\":1}}"
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"fs-write-1\",\"method\":\"fs/write_text_file\",\"params\":{\"sessionId\":\"s1\",\"path\":\"$output_path\",\"content\":\"written from acp\n\"}}"
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"term-create-1\",\"method\":\"terminal/create\",\"params\":{\"sessionId\":\"s1\",\"command\":\"sh\",\"args\":[\"-c\",\"sleep 5\"],\"cwd\":\"$session_cwd\",\"outputByteLimit\":1024}}"
        terminal_id=$(printf '%s' "$response" | sed -n 's/.*"terminalId":"\([^"]*\)".*/\1/p')
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"term-output-1\",\"method\":\"terminal/output\",\"params\":{\"sessionId\":\"s1\",\"terminalId\":\"$terminal_id\"}}"
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"term-wait-1\",\"method\":\"terminal/wait_for_exit\",\"params\":{\"sessionId\":\"s1\",\"terminalId\":\"$terminal_id\"}}"
        send_request "{\"jsonrpc\":\"2.0\",\"id\":\"term-kill-1\",\"method\":\"terminal/kill\",\"params\":{\"sessionId\":\"s1\",\"terminalId\":\"$terminal_id\"}}"
      fi
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("fake acp capabilities written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp_mcp_forwarding(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp-mcp.cmd");
        let ps1_path = bin_dir.join("fake-acp-mcp.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-mcp.ps1\"\r\n",
        )
        .expect("fake acp mcp wrapper written");
        fs::write(
            ps1_path,
            r#"
function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}

function Fail-Request($id, $message) {
    Write-Json @{
        jsonrpc = "2.0"
        id = $id
        error = @{ code = -32000; message = $message }
    }
    exit 0
}

function Find-Server($servers, $name) {
    @($servers) | Where-Object { $_.name -eq $name } | Select-Object -First 1
}

function Has-Env($server, $name, $value) {
    $null -ne (@($server.env) | Where-Object { $_.name -eq $name -and $_.value -eq $value } | Select-Object -First 1)
}

function Has-Header($server, $name, $value) {
    $null -ne (@($server.headers) | Where-Object { $_.name -eq $name -and $_.value -eq $value } | Select-Object -First 1)
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                protocolVersion = 1
                agentCapabilities = @{ mcpCapabilities = @{ http = $true; sse = $true } }
                authMethods = @()
                agentInfo = @{ name = "fake-acp-mcp"; version = "1.0.0" }
            }
        }
    } elseif ($message.method -eq "session/new") {
        $servers = @($message.params.mcpServers)
        $expected = $env:EXPECTED_MCP_PROFILE
        $codegraph = Find-Server $servers "codegraph"
        $scenarioOnly = Find-Server $servers "scenario_only"
        if ($null -eq $codegraph -or $codegraph.command -ne "scenario-codegraph" -or -not (Has-Env $codegraph "API_TOKEN" "scenario-secret")) {
            Fail-Request $message.id "missing scenario codegraph MCP server"
        }
        if ($null -eq $scenarioOnly -or $scenarioOnly.command -ne "scenario-only") {
            Fail-Request $message.id "missing scenario-only MCP server"
        }
        if ($expected -eq "mock") {
            if ($servers.Count -ne 2 -or $null -ne (Find-Server $servers "docs") -or $null -ne (Find-Server $servers "events")) {
                Fail-Request $message.id "mock profile should only include codegraph and scenario_only"
            }
        } elseif ($expected -eq "full") {
            $docs = Find-Server $servers "docs"
            $events = Find-Server $servers "events"
            if ($servers.Count -ne 4 -or $null -eq $docs -or $docs.type -ne "http" -or $docs.url -ne "http://127.0.0.1:3001/profile" -or -not (Has-Header $docs "Authorization" "Bearer profile-secret")) {
                Fail-Request $message.id "full profile should include profile-overridden docs"
            }
            if ($null -eq $events -or $events.type -ne "sse" -or $events.url -ne "http://127.0.0.1:3002/events") {
                Fail-Request $message.id "full profile should include events"
            }
        } else {
            Fail-Request $message.id "EXPECTED_MCP_PROFILE is not set"
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ sessionId = "s1"; configOptions = @() }
        }
    } elseif ($message.method -eq "session/prompt") {
        Write-Json @{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{
                sessionId = "s1"
                update = @{
                    sessionUpdate = "agent_message_chunk"
                    content = @{ type = "text"; text = "done" }
                }
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ stopReason = "end_turn" }
        }
    } elseif ($message.method -eq "session/close") {
        Write-Json @{ jsonrpc = "2.0"; id = $message.id; result = @{} }
        exit 0
    }
}
"#,
        )
        .expect("fake acp mcp script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp-mcp");
        fs::write(
            &path,
            r#"#!/bin/sh
fail_request() {
  id="$1"
  message="$2"
  echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32000,\"message\":\"$message\"}}"
  exit 0
}

while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"mcpCapabilities\":{\"http\":true,\"sse\":true}},\"authMethods\":[],\"agentInfo\":{\"name\":\"fake-acp-mcp\",\"version\":\"1.0.0\"}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      case "$line" in
        *'"name":"codegraph"'*'"command":"scenario-codegraph"'*'"name":"API_TOKEN","value":"scenario-secret"'*) ;;
        *) fail_request "$id" "missing scenario codegraph MCP server" ;;
      esac
      case "$line" in
        *'"name":"scenario_only"'*'"command":"scenario-only"'*) ;;
        *) fail_request "$id" "missing scenario-only MCP server" ;;
      esac
      if [ "$EXPECTED_MCP_PROFILE" = "mock" ]; then
        case "$line" in
          *'"name":"docs"'*|*'"name":"events"'*) fail_request "$id" "mock profile should exclude docs/events" ;;
        esac
      elif [ "$EXPECTED_MCP_PROFILE" = "full" ]; then
        case "$line" in
          *'"type":"http"'*'"name":"docs"'*'"url":"http://127.0.0.1:3001/profile"'*'"name":"Authorization","value":"Bearer profile-secret"'*) ;;
          *) fail_request "$id" "full profile should include profile-overridden docs" ;;
        esac
        case "$line" in
          *'"type":"sse"'*'"name":"events"'*'"url":"http://127.0.0.1:3002/events"'*) ;;
          *) fail_request "$id" "full profile should include events" ;;
        esac
      else
        fail_request "$id" "EXPECTED_MCP_PROFILE is not set"
      fi
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"s1\",\"configOptions\":[]}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("fake acp mcp written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp_config_negotiation(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp-config.cmd");
        let ps1_path = bin_dir.join("fake-acp-config.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-config.ps1\"\r\n",
        )
        .expect("fake acp config wrapper written");
        fs::write(
            ps1_path,
            r#"
function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}

function Fail-Request($id, $message) {
    Write-Json @{
        jsonrpc = "2.0"
        id = $id
        error = @{ code = -32000; message = $message }
    }
    exit 0
}

function Config-Options($mode, $model, $reasoning) {
    @(
        @{
            id = "mode_selector"
            name = "Mode"
            category = "mode"
            type = "select"
            currentValue = $mode
            options = @(
                @{ value = "default"; name = "Default" },
                @{ value = "plan"; name = "Plan" }
            )
        },
        @{
            id = "model_selector"
            name = "Model"
            category = "model"
            type = "select"
            currentValue = $model
            options = @(
                @{ value = "sonnet"; name = "Claude Sonnet" },
                @{ value = "gpt-5-codex"; name = "GPT 5 Codex" }
            )
        },
        @{
            id = "reasoning"
            name = "Reasoning"
            category = "thought_level"
            type = "select"
            currentValue = $reasoning
            options = @(
                @{ value = "low"; name = "Low" },
                @{ value = "medium"; name = "Medium" },
                @{ value = "high"; name = "High" }
            )
        }
    )
}

$mode = "default"
$model = "sonnet"
$reasoning = "medium"
$applied = @()
while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                protocolVersion = 1
                agentCapabilities = @{}
                authMethods = @()
                agentInfo = @{ name = "fake-acp-config"; version = "1.0.0" }
            }
        }
    } elseif ($message.method -eq "session/new") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                sessionId = "s1"
                configOptions = Config-Options $mode $model $reasoning
            }
        }
    } elseif ($message.method -eq "session/set_config_option") {
        $configId = [string]$message.params.configId
        $value = [string]$message.params.value
        if ($configId -eq "mode_selector" -and $value -eq "plan") {
            $mode = $value
        } elseif ($configId -eq "model_selector" -and $value -eq "gpt-5-codex") {
            $model = $value
        } elseif ($configId -eq "reasoning" -and $value -eq "high") {
            $reasoning = $value
        } else {
            Fail-Request $message.id "unexpected config set $configId=$value"
        }
        $applied += $configId
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                configOptions = Config-Options $mode $model $reasoning
            }
        }
    } elseif ($message.method -eq "session/prompt") {
        if (($applied -join ",") -ne "mode_selector,model_selector,reasoning") {
            Fail-Request $message.id "prompt arrived before expected config negotiation"
        }
        Write-Json @{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{
                sessionId = "s1"
                update = @{
                    sessionUpdate = "agent_message_chunk"
                    content = @{ type = "text"; text = "done" }
                }
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ stopReason = "end_turn" }
        }
    } elseif ($message.method -eq "session/close") {
        Write-Json @{ jsonrpc = "2.0"; id = $message.id; result = @{} }
        exit 0
    }
}
"#,
        )
        .expect("fake acp config script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp-config");
        fs::write(
            &path,
            r#"#!/bin/sh
config_options() {
  mode="$1"
  model="$2"
  reasoning="$3"
  printf '"configOptions":[{"id":"mode_selector","name":"Mode","category":"mode","type":"select","currentValue":"%s","options":[{"value":"default","name":"Default"},{"value":"plan","name":"Plan"}]},{"id":"model_selector","name":"Model","category":"model","type":"select","currentValue":"%s","options":[{"value":"sonnet","name":"Claude Sonnet"},{"value":"gpt-5-codex","name":"GPT 5 Codex"}]},{"id":"reasoning","name":"Reasoning","category":"thought_level","type":"select","currentValue":"%s","options":[{"value":"low","name":"Low"},{"value":"medium","name":"Medium"},{"value":"high","name":"High"}]}]' "$mode" "$model" "$reasoning"
}

fail_request() {
  id="$1"
  message="$2"
  echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32000,\"message\":\"$message\"}}"
  exit 0
}

mode="default"
model="sonnet"
reasoning="medium"
applied=""
while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{},\"authMethods\":[],\"agentInfo\":{\"name\":\"fake-acp-config\",\"version\":\"1.0.0\"}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"s1\",$(config_options "$mode" "$model" "$reasoning")}}"
      ;;
    *'"method":"session/set_config_option"'*|*'"method": "session/set_config_option"'*)
      case "$line" in
        *'"configId":"mode_selector"'*'"value":"plan"'*) mode="plan"; applied="${applied}mode_selector," ;;
        *'"configId":"model_selector"'*'"value":"gpt-5-codex"'*) model="gpt-5-codex"; applied="${applied}model_selector," ;;
        *'"configId":"reasoning"'*'"value":"high"'*) reasoning="high"; applied="${applied}reasoning," ;;
        *) fail_request "$id" "unexpected config set" ;;
      esac
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{$(config_options "$mode" "$model" "$reasoning")}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
      if [ "$applied" != "mode_selector,model_selector,reasoning," ]; then
        fail_request "$id" "prompt arrived before expected config negotiation"
      fi
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("fake acp config written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp_transcript(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp-transcript.cmd");
        let ps1_path = bin_dir.join("fake-acp-transcript.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-transcript.ps1\"\r\n",
        )
        .expect("fake acp transcript wrapper written");
        fs::write(
            ps1_path,
            r#"
function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}

[Console]::Error.WriteLine("Authorization: Bearer stderr-secret ACP_TOKEN=$env:ACP_TOKEN")
[Console]::Error.Flush()

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq "initialize") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{
                protocolVersion = 1
                agentCapabilities = @{ mcpCapabilities = @{ http = $true; sse = $true } }
                authMethods = @()
                agentInfo = @{ name = "fake-acp-transcript"; version = "1.0.0" }
            }
        }
    } elseif ($message.method -eq "session/new") {
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ sessionId = "s1"; configOptions = @() }
        }
    } elseif ($message.method -eq "session/prompt") {
        Write-Json @{
            jsonrpc = "2.0"
            method = "session/update"
            params = @{
                sessionId = "s1"
                update = @{
                    sessionUpdate = "agent_message_chunk"
                    content = @{ type = "text"; text = "done" }
                }
            }
        }
        Write-Json @{
            jsonrpc = "2.0"
            id = $message.id
            result = @{ stopReason = "end_turn" }
        }
    } elseif ($message.method -eq "session/close") {
        Write-Json @{ jsonrpc = "2.0"; id = $message.id; result = @{} }
        exit 0
    }
}
"#,
        )
        .expect("fake acp transcript script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp-transcript");
        fs::write(
            &path,
            r#"#!/bin/sh
echo "Authorization: Bearer stderr-secret ACP_TOKEN=$ACP_TOKEN" >&2
while IFS= read -r line || [ -n "$line" ]; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"mcpCapabilities\":{\"http\":true,\"sse\":true}},\"authMethods\":[],\"agentInfo\":{\"name\":\"fake-acp-transcript\",\"version\":\"1.0.0\"}}}"
      ;;
    *'"method":"session/new"'*|*'"method": "session/new"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"s1\",\"configOptions\":[]}}"
      ;;
    *'"method":"session/prompt"'*|*'"method": "session/prompt"'*)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
      ;;
    *'"method":"session/close"'*|*'"method": "session/close"'*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("fake acp transcript written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn write_fake_acp_invalid_stdout(bin_dir: &Path) {
    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("fake-acp-invalid.cmd");
        let ps1_path = bin_dir.join("fake-acp-invalid.ps1");
        fs::write(
            cmd_path,
            "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-invalid.ps1\"\r\n",
        )
        .expect("fake acp invalid wrapper written");
        fs::write(
            ps1_path,
            r#"
[Console]::Error.WriteLine("TOKEN=bad-secret")
[Console]::Error.Flush()
[Console]::Out.WriteLine("not-json Authorization: Bearer invalid-secret")
[Console]::Out.Flush()
Start-Sleep -Milliseconds 100
"#,
        )
        .expect("fake acp invalid script written");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("fake-acp-invalid");
        fs::write(
            &path,
            "#!/bin/sh\n\
echo 'TOKEN=bad-secret' >&2\n\
echo 'not-json Authorization: Bearer invalid-secret'\n\
sleep 0.1\n",
        )
        .expect("fake acp invalid written");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn join_path_prefix(bin_dir: &Path, old_path: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut out = std::ffi::OsString::from(bin_dir.as_os_str());
    let sep = if cfg!(windows) { ";" } else { ":" };
    out.push(sep);
    out.push(old_path);
    out
}

struct TraceSeed<'a> {
    run_id: &'a str,
    skill: &'a str,
    scenario: &'a str,
    finished_at: &'a str,
    pass: bool,
    score: Option<f64>,
    tool: &'a str,
}

fn write_named_trace(root: &Path, seed: TraceSeed<'_>) -> std::path::PathBuf {
    let mut trace = TraceRecord::synthetic(
        vec![Turn {
            index: 0,
            role: "assistant".to_string(),
            text_deltas: vec![format!("assistant text for {}", seed.run_id)],
            tool_calls: vec![ToolCallRecord::new(
                "call-1",
                seed.tool,
                json!({"command": "echo ok", "file_path": "README.md"}),
            )],
            usage: None,
        }],
        format!("final output for {}", seed.run_id),
        1,
        None,
    );
    trace.run_id = seed.run_id.to_string();
    trace.skill.name = seed.skill.to_string();
    trace.scenario.name = seed.scenario.to_string();
    let finished_at = DateTime::parse_from_rfc3339(seed.finished_at)
        .expect("valid timestamp")
        .with_timezone(&Utc);
    trace.runner.started_at = finished_at;
    trace.runner.finished_at = finished_at;
    trace.runner.duration_ms = if seed.pass { 1_000 } else { 2_500 };
    trace.runner.turns_used = 1;
    trace.tool_call_summary = ToolCallSummary::from_turns(&trace.turns, 0);
    trace.scoring.overall_pass = seed.pass;
    trace.scoring.all_passed = seed.pass;
    trace.scoring.weighted_score = seed.score;
    trace.cost.input_tokens = 10;
    trace.cost.output_tokens = if seed.pass { 5 } else { 8 };
    trace.cost.usd_estimate = if seed.pass { 0.001 } else { 0.002 };
    trace.assertions = vec![AssertionResult {
        id: "expected-output".to_string(),
        kind: "output_contains".to_string(),
        pass: seed.pass,
        detail: if seed.pass {
            "final output matched".to_string()
        } else {
            "final output did not match pattern".to_string()
        },
        weight: 1.0,
        score: None,
        min_score: None,
        rationale: None,
        captures: Vec::new(),
    }];
    if !seed.pass {
        trace.errors.push(TraceError {
            kind: "runtime".to_string(),
            message: format!("runtime error for {}", seed.run_id),
        });
    }
    write_trace(root.join("runs"), &trace).expect("trace written")
}
