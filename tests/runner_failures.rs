#![allow(deprecated)]

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn partial(runtime: &str) -> &'static str {
    match runtime {
        "codex" => include_str!("fixtures/runtime-failures/codex-partial.jsonl"),
        "claude" => include_str!("fixtures/runtime-failures/claude-partial.jsonl"),
        _ => unreachable!(),
    }
}

fn completion(runtime: &str) -> &'static str {
    match runtime {
        "codex" => include_str!("fixtures/runtime-failures/codex-completion.jsonl"),
        "claude" => include_str!("fixtures/runtime-failures/claude-completion.jsonl"),
        _ => unreachable!(),
    }
}

// The executable writes an actual sandbox file and a separately observed journal.
// Its stream can stop before acknowledging either write.
fn write_fake_runtime(bin: &Path, runtime: &str) {
    if runtime == "acp" {
        write_fake_acp_fault(bin);
        return;
    }
    #[cfg(windows)]
    {
        fs::write(
            bin.join(format!("{runtime}.cmd")),
            // Consume the known Codex --cd position in CMD. CLI flags, including
            // its stdin marker '-', must not enter PowerShell's parameter binder.
            format!("@echo off\r\nset \"AI_TESTER_RUNTIME_CWD=\"\r\nif \"%~1\"==\"exec\" if \"%~4\"==\"--cd\" set \"AI_TESTER_RUNTIME_CWD=%~5\"\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0{runtime}.ps1\"\r\nexit /b %errorlevel%\r\n"),
        )
        .unwrap();
        fs::write(
            bin.join(format!("{runtime}.ps1")),
            r#"
if ($env:AI_TESTER_EXIT -ne '8') { [Console]::In.ReadToEnd() | Out-Null }
if ($env:AI_TESTER_RUNTIME_CWD) {
    Set-Location -LiteralPath $env:AI_TESTER_RUNTIME_CWD
} elseif (Test-Path -LiteralPath $env:AI_TESTER_CWD_STATE) {
    Set-Location -LiteralPath ([IO.File]::ReadAllText($env:AI_TESTER_CWD_STATE))
}
[IO.File]::WriteAllText($env:AI_TESTER_CWD_STATE, (Get-Location).Path)

Add-Content -LiteralPath $env:AI_TESTER_ATTEMPTS -Value 'attempt'
$stream = $env:AI_TESTER_STREAM
if ((Test-Path -LiteralPath $env:AI_TESTER_FIRST_STREAM) -and ((Get-Content -LiteralPath $env:AI_TESTER_ATTEMPTS).Count -eq 1)) {
    $stream = $env:AI_TESTER_FIRST_STREAM
}
[Console]::Out.Write([IO.File]::ReadAllText($stream))
[Console]::Out.Flush()
if ($env:AI_TESTER_COMMIT_EFFECT -eq '1') {
    [IO.File]::WriteAllText((Join-Path (Get-Location) 'effect.txt'), 'committed')
    [IO.File]::WriteAllText($env:AI_TESTER_JOURNAL, 'committed')
}
[Console]::Error.WriteLine('synthetic runtime exit')
exit ([int]$env:AI_TESTER_EXIT)
"#,
        )
        .unwrap();
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join(runtime);
        fs::write(
            &path,
            r#"#!/bin/sh
if [ "$AI_TESTER_EXIT" != 8 ]; then cat >/dev/null; fi
while [ "$#" -gt 0 ]; do
    if [ "$1" = '--cd' ]; then shift; cd "$1" || exit 99; fi
    shift
done
if [ -f "$AI_TESTER_CWD_STATE" ]; then
    cd "$(cat "$AI_TESTER_CWD_STATE")" || exit 99
fi
pwd > "$AI_TESTER_CWD_STATE"
printf 'attempt\n' >> "$AI_TESTER_ATTEMPTS"
stream="$AI_TESTER_STREAM"
if [ -f "$AI_TESTER_FIRST_STREAM" ] && [ "$(wc -l < "$AI_TESTER_ATTEMPTS" | tr -d ' ')" = 1 ]; then
    stream="$AI_TESTER_FIRST_STREAM"
fi
cat "$stream"
if [ "$AI_TESTER_COMMIT_EFFECT" = 1 ]; then
    printf committed > effect.txt
    printf committed > "$AI_TESTER_JOURNAL"
fi
echo 'synthetic runtime exit' >&2
exit "$AI_TESTER_EXIT"
"#,
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn write_fake_acp_fault(bin: &Path) {
    #[cfg(windows)]
    {
        fs::write(bin.join("fake-acp-fault.cmd"), "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-acp-fault.ps1\"\r\n").unwrap();
        fs::write(bin.join("fake-acp-fault.ps1"), r#"
[IO.File]::WriteAllText($env:AI_TESTER_PID, "$PID")
function Write-Json($value) {
    [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 32))
    [Console]::Out.Flush()
}
while ($null -ne ($line = [Console]::In.ReadLine())) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' {
            Write-Json @{ jsonrpc = '2.0'; id = $message.id; result = @{ protocolVersion = 1; agentCapabilities = @{}; authMethods = @() } }
        }
        'session/new' {
            Set-Location -LiteralPath $message.params.cwd
            Write-Json @{ jsonrpc = '2.0'; id = $message.id; result = @{ sessionId = 'fault-session' } }
        }
        'session/prompt' {
            Add-Content -LiteralPath $env:AI_TESTER_ATTEMPTS -Value 'attempt'
            if (-not $env:AI_TESTER_ACP_TAIL) {
                [Console]::Out.Write([IO.File]::ReadAllText($env:AI_TESTER_STREAM))
                [Console]::Out.Flush()
            }
            if ($env:AI_TESTER_COMMIT_EFFECT -eq '1') {
                [IO.File]::WriteAllText((Join-Path (Get-Location) 'effect.txt'), 'committed')
                [IO.File]::WriteAllText($env:AI_TESTER_JOURNAL, 'committed')
            }
            if ($env:AI_TESTER_ACP_TAIL) {
                $response = @{ jsonrpc = '2.0'; id = $message.id; result = @{ stopReason = 'end_turn' } } | ConvertTo-Json -Compress -Depth 32
                $burst = [IO.File]::ReadAllText($env:AI_TESTER_STREAM) + $response + "`n" + [IO.File]::ReadAllText($env:AI_TESTER_ACP_TAIL)
                [Console]::Out.Write($burst)
                [Console]::Out.Flush()
                exit 0
            }
            if ($env:AI_TESTER_EXIT -eq '-1') {
                while ($null -ne [Console]::In.ReadLine()) {} # ignore cancellation and close
                exit 0
            }
            if ($env:AI_TESTER_ACP_STOP) {
                Write-Json @{ jsonrpc = '2.0'; id = $message.id; result = @{ stopReason = $env:AI_TESTER_ACP_STOP } }
                if ($env:AI_TESTER_EXIT -eq '0') { continue }
            }
            exit ([int]$env:AI_TESTER_EXIT)
        }
        'session/close' {
            Write-Json @{ jsonrpc = '2.0'; id = $message.id; result = @{} }
            exit 0
        }
    }
}
"#).unwrap();
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join("fake-acp-fault");
        fs::write(&path, r#"#!/bin/sh
printf '%s' "$$" > "$AI_TESTER_PID"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"fault-session"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf 'attempt\n' >> "$AI_TESTER_ATTEMPTS"
            if [ -z "$AI_TESTER_ACP_TAIL" ]; then cat "$AI_TESTER_STREAM"; fi
            if [ "$AI_TESTER_COMMIT_EFFECT" = 1 ]; then
                printf committed > effect.txt
                printf committed > "$AI_TESTER_JOURNAL"
            fi
            if [ -n "$AI_TESTER_ACP_TAIL" ]; then
                burst=$(cat "$AI_TESTER_STREAM"; printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"; cat "$AI_TESTER_ACP_TAIL")
                printf '%s\n' "$burst"
                exit 0
            fi
            if [ "$AI_TESTER_EXIT" = -1 ]; then
                while IFS= read -r ignored; do :; done
                exit 0
            fi
            if [ -n "$AI_TESTER_ACP_STOP" ]; then
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"%s"}}\n' "$id" "$AI_TESTER_ACP_STOP"
                if [ "$AI_TESTER_EXIT" = 0 ]; then continue; fi
            fi
            exit "$AI_TESTER_EXIT"
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            exit 0
            ;;
    esac
done
"#).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn process_is_alive(pid: &str) -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}

struct Fault<'a> {
    stream: &'a str,
    exit: i32,
    commit_effect: bool,
    first_turn: Option<&'a str>,
}

fn check_run(runtime: &str, fault: &Fault<'_>, format: &str, stopped: &str) -> Value {
    check_run_with_acp_tail(runtime, fault, format, stopped, None)
}

fn check_run_with_acp_tail(
    runtime: &str,
    fault: &Fault<'_>,
    format: &str,
    stopped: &str,
    tail: Option<&str>,
) -> Value {
    let tmp = TempDir::new().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    write_fake_runtime(&bin, runtime);
    let stream = tmp.path().join("stream.jsonl");
    fs::write(&stream, fault.stream).unwrap();
    let tail_path = tmp.path().join("tail.jsonl");
    if let Some(tail) = tail {
        fs::write(&tail_path, tail).unwrap();
    }
    let first_stream = tmp.path().join("first.jsonl");
    if let Some(first) = fault.first_turn {
        fs::write(&first_stream, first).unwrap();
    }
    let attempts = tmp.path().join("attempts.txt");
    let journal = tmp.path().join("journal.txt");
    fs::write(tmp.path().join(".ai-tester.yaml"), if runtime == "acp" {
        "skills_dir: ./skills\nacp_agents:\n  local:\n    command: fake-acp-fault\n    args: []\n"
    } else { "skills_dir: ./skills\n" }).unwrap();
    let scenario = tmp.path().join("scenario.yaml");
    // Force a broken stdin pipe when the fake exits without reading the prompt.
    let prompt = if runtime == "codex" && fault.exit == 8 {
        "x".repeat(1024 * 1024)
    } else {
        "Test fixture.".to_string()
    };
    fs::write(
        &scenario,
        format!(
            "scenario: runtime-fault\nsystem_prompt: {prompt}\nrunner:\n  runtime: {runtime}\n{}{}assertions:\n  - id: called\n    type: tool_called\n    tool: {}\n    args_match:\n      command: write-effect\n  - id: effect\n    type: file_contains\n    path: effect.txt\n    pattern: committed\n",
            if runtime == "acp" { "  agent: local\n" } else { "" },
            if fault.first_turn.is_some() { "user_prompts: [first, second, third]\n" } else { "" },
            if runtime == "acp" { "execute" } else { "Bash" }
        ),
    ).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(bin).chain(std::env::split_paths(&path));
    let expected_code = if stopped == "end_turn" { 0 } else { 2 };
    let mut cmd = Command::cargo_bin("ai-tester").unwrap();
    let pid_file = tmp.path().join("runtime.pid");
    cmd.env_remove("AI_TESTER_ACP_TAIL");
    if tail.is_some() {
        cmd.env("AI_TESTER_ACP_TAIL", &tail_path);
    }
    let output = cmd
        .current_dir(tmp.path())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("AI_TESTER_STREAM", stream)
        .env("AI_TESTER_FIRST_STREAM", first_stream)
        .env("AI_TESTER_ATTEMPTS", &attempts)
        .env("AI_TESTER_CWD_STATE", tmp.path().join("runtime-cwd.txt"))
        .env("AI_TESTER_JOURNAL", &journal)
        .env(
            "AI_TESTER_COMMIT_EFFECT",
            if fault.commit_effect { "1" } else { "0" },
        )
        .env("AI_TESTER_EXIT", fault.exit.to_string())
        .env("AI_TESTER_PID", &pid_file)
        .env(
            "AI_TESTER_ACP_STOP",
            if runtime == "acp" && matches!(stopped, "cancelled" | "end_turn") {
                stopped
            } else {
                ""
            },
        )
        .args([
            "run",
            "--file",
            scenario.to_str().unwrap(),
            "--format",
            format,
            "--acp-turn-timeout",
            "1",
            "--idle-warn",
            "10",
        ])
        .timeout(Duration::from_secs(30))
        .assert()
        .code(expected_code)
        .get_output()
        .clone();
    let trace_dir = tmp.path().join("runs/inline_runtime-fault");
    let traces = fs::read_dir(trace_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(traces.len(), 1);
    let trace: Value =
        serde_json::from_str(&fs::read_to_string(traces[0].path()).unwrap()).unwrap();
    assert_eq!(
        trace["runner"]["stoppedReason"], stopped,
        "{}",
        trace["errors"]
    );
    assert_eq!(trace["scoring"]["overallPass"], stopped == "end_turn");
    assert_eq!(
        trace["errors"].as_array().unwrap().is_empty(),
        stopped == "end_turn"
    );
    assert_eq!(
        trace["toolCallSummary"]["total"],
        if fault.first_turn.is_some() { 2 } else { 1 }
    );
    let assertions = trace["assertions"].as_array().unwrap();
    assert_eq!(
        assertions.iter().find(|a| a["id"] == "called").unwrap()["pass"],
        true
    );
    assert_eq!(
        assertions.iter().find(|a| a["id"] == "effect").unwrap()["pass"],
        fault.commit_effect
    );
    assert_eq!(journal.exists(), fault.commit_effect);
    if fault.commit_effect {
        assert_eq!(fs::read_to_string(&journal).unwrap(), "committed");
    }
    if stopped != "end_turn" {
        let last_call = &trace["turns"].as_array().unwrap().last().unwrap()["toolCalls"][0];
        assert!(last_call["resultContent"].is_null());
        assert_eq!(last_call["resultIsError"], false); // absent response is not a confirmed failure
    }
    let attempt_count = fs::read_to_string(attempts).unwrap().lines().count();
    assert_eq!(
        attempt_count,
        if fault.first_turn.is_some() { 2 } else { 1 }
    );
    assert!(!Path::new(trace["runner"]["sandboxPath"].as_str().unwrap()).exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    if format == "json" {
        let report: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(report["summary"]["total"], 1);
        assert_eq!(report["summary"]["overallPass"], stopped == "end_turn");
        assert_eq!(
            report["summary"]["errors"],
            if stopped == "end_turn" { 0 } else { 1 }
        );
        assert_eq!(report["runs"][0], trace);
    } else if stopped != "end_turn" {
        assert!(stdout.contains("FAIL"), "{stdout}");
        assert!(!stdout.contains("PASS"), "{stdout}");
        assert!(stdout.contains(if stopped == "error" {
            "synthetic runtime exit"
        } else {
            stopped
        }));
    }
    if runtime == "acp" {
        let pid = fs::read_to_string(pid_file).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_alive(&pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!process_is_alive(&pid), "fake ACP process {pid} leaked");
    }
    trace
}

#[test]
fn acp_disconnect_truncation_and_cancellation_preserve_effects_and_clean_up() {
    let partial = include_str!("fixtures/runtime-failures/acp-partial.jsonl");
    let truncated = format!("{partial}{{\"jsonrpc\":");
    for (stream, exit, stopped) in [
        (partial, 7, "incomplete"),
        (truncated.as_str(), 0, "incomplete"),
        (partial, 0, "cancelled"),
    ] {
        for format in ["json", "markdown", "live"] {
            check_run(
                "acp",
                &Fault {
                    stream,
                    exit,
                    commit_effect: true,
                    first_turn: None,
                },
                format,
                stopped,
            );
        }
    }
    check_run(
        "acp",
        &Fault {
            stream: partial,
            exit: 0,
            commit_effect: true,
            first_turn: None,
        },
        "json",
        "end_turn",
    );
}

#[test]
fn acp_silent_hang_preserves_tool_evidence_and_kills_the_unresponsive_process() {
    let started = Instant::now();
    check_run(
        "acp",
        &Fault {
            stream: include_str!("fixtures/runtime-failures/acp-partial.jsonl"),
            exit: -1,
            commit_effect: true,
            first_turn: None,
        },
        "json",
        "timeout",
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn acp_terminal_response_before_eof_keeps_queued_tool_evidence() {
    check_run(
        "acp",
        &Fault {
            stream: include_str!("fixtures/runtime-failures/acp-partial.jsonl"),
            exit: 7,
            commit_effect: true,
            first_turn: None,
        },
        "json",
        "end_turn",
    );
}

#[test]
fn acp_completion_does_not_hide_protocol_errors_in_the_same_stdout_burst() {
    for (tail, error) in [
        ("{\"jsonrpc\":\n", "invalid ACP JSON-RPC stdout line"),
        (
            "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{}}\n",
            "invalid session/update params",
        ),
        ("", ""), // The same burst followed by clean EOF still succeeds.
    ] {
        let trace = check_run_with_acp_tail(
            "acp",
            &Fault {
                stream: include_str!("fixtures/runtime-failures/acp-partial.jsonl"),
                exit: 0,
                commit_effect: true,
                first_turn: None,
            },
            "json",
            if tail.is_empty() {
                "end_turn"
            } else {
                "incomplete"
            },
            Some(tail),
        );
        assert_eq!(trace["scoring"]["allPassed"], true);
        if !tail.is_empty() {
            assert!(
                trace["errors"][0]["message"]
                    .as_str()
                    .unwrap()
                    .contains(error),
                "{trace}"
            );
        }
    }
}

#[test]
fn subprocess_exit_and_truncated_streams_keep_evidence_in_every_report_format() {
    for runtime in ["codex", "claude"] {
        let truncated = format!("{}{{\"type\":", partial(runtime));
        for (stream, exit, stopped) in [
            (partial(runtime), 7, "error"),
            (partial(runtime), 0, "incomplete"),
            (truncated.as_str(), 0, "incomplete"),
        ] {
            for format in ["json", "markdown", "live"] {
                let trace = check_run(
                    runtime,
                    &Fault {
                        stream,
                        exit,
                        commit_effect: true,
                        first_turn: None,
                    },
                    format,
                    stopped,
                );
                assert_eq!(trace["scoring"]["allPassed"], true);
                if exit != 0 {
                    assert!(trace["errors"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| e["kind"] == format!("{runtime}_process")));
                }
            }
        }
    }
}

#[test]
fn tool_call_alone_does_not_prove_an_effect() {
    for runtime in ["codex", "claude"] {
        check_run(
            runtime,
            &Fault {
                stream: partial(runtime),
                exit: 0,
                commit_effect: false,
                first_turn: None,
            },
            "json",
            "incomplete",
        );
    }
}

#[test]
fn completed_stream_and_independent_effect_can_pass() {
    for runtime in ["codex", "claude"] {
        let complete = format!("{}{}", partial(runtime), completion(runtime));
        check_run(
            runtime,
            &Fault {
                stream: &complete,
                exit: 0,
                commit_effect: true,
                first_turn: None,
            },
            "json",
            "end_turn",
        );
    }
}

#[test]
fn interrupted_scripted_turn_preserves_previous_turns_and_stops_followups() {
    for runtime in ["codex", "claude"] {
        let first = format!("{}{}", partial(runtime), completion(runtime));
        check_run(
            runtime,
            &Fault {
                stream: partial(runtime),
                exit: 0,
                commit_effect: true,
                first_turn: Some(&first),
            },
            "json",
            "incomplete",
        );
    }
}

#[test]
fn runtime_exit_before_reading_stdin_still_preserves_stdout_and_effects() {
    let trace = check_run(
        "codex",
        &Fault {
            stream: partial("codex"),
            exit: 8,
            commit_effect: true,
            first_turn: None,
        },
        "json",
        "error",
    );
    assert!(trace["errors"].as_array().unwrap().iter().any(|error| {
        error["message"]
            .as_str()
            .unwrap()
            .contains("failed to write runtime stdin")
    }));
}
