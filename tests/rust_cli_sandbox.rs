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
    let path = write_trace(tmp.path(), &trace).expect("trace written");
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
    write_trace(root, &trace).expect("trace written")
}
