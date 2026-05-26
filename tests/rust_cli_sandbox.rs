use std::fs;
use std::path::Path;

use ai_tester::sandbox::{create_sandbox, SandboxOptions};
use ai_tester::scenario::{FixtureFile, Fixtures};
use ai_tester::trace::{write_trace, TraceRecord};
use assert_cmd::Command;
use predicates::prelude::*;
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
fn cli_placeholder_commands_are_explicit_stubs() {
    for (command, expected) in [
        ("trend", "trend: not implemented"),
        ("compare", "compare: not implemented"),
        ("trace", "trace: not implemented"),
    ] {
        let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
        let args = match command {
            "trend" => vec![command, "skill"],
            "compare" => vec![command, "a", "b"],
            "trace" => vec![command, "run-id"],
            _ => unreachable!(),
        };
        cmd.args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
}

#[test]
fn cli_history_reads_v2_traces_from_runs_dir() {
    let tmp = TempDir::new().expect("temp dir");
    let trace = TraceRecord::synthetic(Vec::new(), "ok".to_string(), 2, None);
    let path = write_trace(tmp.path(), &trace).expect("trace written");
    assert!(path.exists());

    let mut cmd = Command::cargo_bin("ai-tester").expect("binary");
    cmd.current_dir(tmp.path())
        .args(["history", "--last", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Run history"))
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
        .stdout(predicate::str::contains("FAIL turn_budget"))
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
        "skills_dir: ./skills\ndefaults:\n  model: config-model\n  permission_mode: plan\n",
    )
    .expect("config written");

    let scenario = tmp.path().join("scenario.yaml");
    fs::write(
        &scenario,
        "scenario: config-defaults\nsystem_prompt: You are helpful.\nrunner:\n  runtime: codex\nassertions:\n  - id: says-done\n    type: output_contains\n    pattern: done\n",
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
        .stdout(predicate::str::contains("FAIL no_unanswered_questions"))
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

fn join_path_prefix(bin_dir: &Path, old_path: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut out = std::ffi::OsString::from(bin_dir.as_os_str());
    let sep = if cfg!(windows) { ";" } else { ":" };
    out.push(sep);
    out.push(old_path);
    out
}
