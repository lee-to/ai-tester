use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::scenario::Scenario;
use crate::trace::{ToolCallRecord, TraceCost, TraceError, Turn, TurnUsage};

pub struct RuntimeStatus {
    pub name: &'static str,
    pub description: &'static str,
    pub ready: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeRunResult {
    pub turns: Vec<Turn>,
    pub final_output: String,
    pub turns_used: u32,
    pub max_turns_effective: u32,
    pub max_turns_user_set: bool,
    pub session_id: Option<String>,
    pub cost: TraceCost,
    pub unanswered_questions: usize,
    pub stopped_reason: String,
    pub errors: Vec<TraceError>,
    pub diagnostics: Vec<String>,
}

impl RuntimeRunResult {
    fn new(max_turns_effective: u32, max_turns_user_set: bool) -> Self {
        Self {
            turns: Vec::new(),
            final_output: String::new(),
            turns_used: 0,
            max_turns_effective,
            max_turns_user_set,
            session_id: None,
            cost: TraceCost {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                usd_estimate: 0.0,
                source: "unknown".to_string(),
            },
            unanswered_questions: 0,
            stopped_reason: "other".to_string(),
            errors: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

pub fn parse_codex_jsonl(
    jsonl: &str,
    max_turns_effective: u32,
    max_turns_user_set: bool,
) -> anyhow::Result<RuntimeRunResult> {
    let mut out = RuntimeRunResult::new(max_turns_effective, max_turns_user_set);
    out.cost.source = "codex".to_string();

    for line in jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(err) => {
                out.diagnostics
                    .push(format!("unparseable codex jsonl line: {err}"));
                continue;
            }
        };
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "thread.started" => {
                out.session_id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            "turn.started" => {
                let turn = Turn {
                    index: out.turns.len(),
                    role: "assistant".to_string(),
                    text_deltas: Vec::new(),
                    tool_calls: Vec::new(),
                    usage: None,
                };
                out.turns.push(turn);
                out.turns_used += 1;
                if out.turns_used > max_turns_effective {
                    out.stopped_reason = "max_turns".to_string();
                }
            }
            "item.started" => {
                if let Some(item) = event.get("item") {
                    handle_codex_item(item, false, &mut out);
                }
            }
            "item.completed" => {
                if let Some(item) = event.get("item") {
                    handle_codex_item(item, true, &mut out);
                }
            }
            "turn.completed" => {
                if let Some(usage) = event.get("usage") {
                    out.cost.input_tokens += u64_field(usage, "input_tokens");
                    out.cost.output_tokens += u64_field(usage, "output_tokens");
                    out.cost.cache_read_tokens += u64_field(usage, "cached_input_tokens");
                }
                if out.stopped_reason == "other" {
                    out.stopped_reason = "end_turn".to_string();
                }
            }
            "turn.failed" => {
                out.stopped_reason = "error".to_string();
                out.errors.push(TraceError {
                    kind: "codex_turn_failed".to_string(),
                    message: event
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("turn failed")
                        .to_string(),
                });
            }
            "error" => {
                out.stopped_reason = "error".to_string();
                out.errors.push(TraceError {
                    kind: "codex_stream_error".to_string(),
                    message: event
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("stream error")
                        .to_string(),
                });
            }
            other => out
                .diagnostics
                .push(format!("unsupported codex event: {other}")),
        }
    }
    Ok(out)
}

pub fn parse_claude_jsonl(
    jsonl: &str,
    max_turns_effective: u32,
    max_turns_user_set: bool,
) -> anyhow::Result<RuntimeRunResult> {
    let mut out = RuntimeRunResult::new(max_turns_effective, max_turns_user_set);
    out.cost.source = "claude".to_string();

    for line in jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(err) => {
                out.diagnostics
                    .push(format!("unparseable claude jsonl line: {err}"));
                continue;
            }
        };
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "system" => {
                if event.get("subtype").and_then(Value::as_str) == Some("init") {
                    out.session_id = event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
            "assistant" => handle_claude_assistant(&event, &mut out),
            "user" => handle_claude_user(&event, &mut out),
            "result" => {
                if let Some(result) = event.get("result").and_then(Value::as_str) {
                    out.final_output = result.to_string();
                }
                if let Some(usage) = event.get("usage") {
                    out.cost.input_tokens += u64_field(usage, "input_tokens");
                    out.cost.output_tokens += u64_field(usage, "output_tokens");
                    out.cost.cache_creation_tokens +=
                        u64_field(usage, "cache_creation_input_tokens");
                    out.cost.cache_read_tokens += u64_field(usage, "cache_read_input_tokens");
                }
                if let Some(cost) = event.get("total_cost_usd").and_then(Value::as_f64) {
                    out.cost.usd_estimate = cost;
                }
                match event.get("subtype").and_then(Value::as_str) {
                    Some("error_max_turns") => out.stopped_reason = "max_turns".to_string(),
                    Some("success") => out.stopped_reason = "end_turn".to_string(),
                    Some(other) => {
                        out.stopped_reason = "error".to_string();
                        out.errors.push(TraceError {
                            kind: "claude_result".to_string(),
                            message: other.to_string(),
                        });
                    }
                    None => {}
                }
            }
            other => out
                .diagnostics
                .push(format!("unsupported claude event: {other}")),
        }
    }
    Ok(out)
}

fn handle_codex_item(item: &Value, is_completed: bool, out: &mut RuntimeRunResult) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "command_execution" if !is_completed => {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            current_turn_mut(out).tool_calls.push(ToolCallRecord::new(
                id,
                "Bash",
                serde_json::json!({ "command": command }),
            ));
        }
        "command_execution" => {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            let output = item
                .get("aggregated_output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(call) = find_tool_call_mut(out, id) {
                call.result_content = Some(output);
                call.result_is_error = item.get("status").and_then(Value::as_str) == Some("failed");
            }
        }
        "agent_message" if is_completed => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                out.final_output = text.to_string();
                current_turn_mut(out).text_deltas.push(text.to_string());
            }
        }
        "file_change" if is_completed => {
            if let Some(changes) = item.get("changes").and_then(Value::as_array) {
                for change in changes {
                    let path = change
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let kind = change
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let tool = match kind {
                        "add" => "Write",
                        "delete" => "Bash",
                        _ => "Edit",
                    };
                    let input = if kind == "delete" {
                        serde_json::json!({ "command": format!("rm {path}") })
                    } else {
                        serde_json::json!({ "file_path": path })
                    };
                    current_turn_mut(out).tool_calls.push(ToolCallRecord {
                        id: format!(
                            "{}:{path}",
                            item.get("id").and_then(Value::as_str).unwrap_or("file")
                        ),
                        name: tool.to_string(),
                        input,
                        result_content: Some(format!("{kind} {path}")),
                        result_is_error: item.get("status").and_then(Value::as_str)
                            == Some("failed"),
                        answered: None,
                    });
                }
            }
        }
        _ => {}
    }
}

fn handle_claude_assistant(event: &Value, out: &mut RuntimeRunResult) {
    let mut turn = Turn {
        index: out.turns.len(),
        role: "assistant".to_string(),
        text_deltas: Vec::new(),
        tool_calls: Vec::new(),
        usage: None,
    };
    if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
        for block in content {
            match block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        turn.text_deltas.push(text.to_string());
                    }
                }
                "tool_use" => {
                    turn.tool_calls.push(ToolCallRecord::new(
                        block.get("id").and_then(Value::as_str).unwrap_or_default(),
                        block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        block
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    ));
                }
                _ => {}
            }
        }
    }
    if let Some(usage) = event.pointer("/message/usage") {
        turn.usage = Some(TurnUsage {
            input_tokens: u64_field(usage, "input_tokens"),
            cache_creation_input_tokens: u64_field(usage, "cache_creation_input_tokens"),
            cache_read_input_tokens: u64_field(usage, "cache_read_input_tokens"),
            output_tokens: u64_field(usage, "output_tokens"),
        });
    }
    out.turns_used += 1;
    out.turns.push(turn);
}

fn handle_claude_user(event: &Value, out: &mut RuntimeRunResult) {
    let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
        return;
    };
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let id = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = block
            .get("content")
            .map(stringify_value)
            .unwrap_or_default();
        if let Some(call) = find_tool_call_mut(out, id) {
            call.result_content = Some(content);
            call.result_is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }
    }
}

fn current_turn_mut(out: &mut RuntimeRunResult) -> &mut Turn {
    if out.turns.is_empty() {
        out.turns.push(Turn {
            index: 0,
            role: "assistant".to_string(),
            text_deltas: Vec::new(),
            tool_calls: Vec::new(),
            usage: None,
        });
        out.turns_used = 1;
    }
    out.turns.last_mut().expect("turn exists")
}

fn find_tool_call_mut<'a>(
    out: &'a mut RuntimeRunResult,
    id: &str,
) -> Option<&'a mut ToolCallRecord> {
    out.turns
        .iter_mut()
        .flat_map(|turn| turn.tool_calls.iter_mut())
        .find(|call| call.id == id)
}

fn u64_field(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn stringify_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    item.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

pub fn list_runtime_statuses() -> Vec<RuntimeStatus> {
    vec![
        preflight(
            "claude",
            "Claude Code via `claude -p --output-format stream-json`.",
        ),
        preflight("codex", "OpenAI Codex via `codex exec --json`."),
    ]
}

#[derive(Debug, Clone)]
pub struct RuntimeRunRequest {
    pub runtime: String,
    pub skill_body: String,
    pub scenario: Scenario,
    pub cwd: PathBuf,
    pub user_messages: Vec<String>,
    pub skill_install_rel_path: Option<String>,
}

pub fn runtime_ready(name: &str) -> bool {
    command_exists(name)
}

pub fn run_runtime(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    match req.runtime.as_str() {
        "codex" => run_codex(req),
        "claude" => run_claude(req),
        other => anyhow::bail!("unknown runtime `{other}`"),
    }
}

fn run_codex(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    let max_turns = req
        .scenario
        .max_turns
        .unwrap_or(crate::config::INTERNAL_MAX_TURNS);
    let max_turns_user_set = req.scenario.max_turns.is_some();
    let mut combined = RuntimeRunResult::new(max_turns, max_turns_user_set);
    let mut session_id: Option<String> = None;

    for (idx, user_message) in req.user_messages.iter().enumerate() {
        let prompt = if idx == 0 {
            build_codex_input(
                &req.skill_body,
                req.skill_install_rel_path.as_deref(),
                user_message,
            )
        } else {
            user_message.clone()
        };
        let stdout = if idx == 0 {
            run_process_with_stdin(
                "codex",
                &[
                    "exec".to_string(),
                    "--json".to_string(),
                    "--skip-git-repo-check".to_string(),
                    "--cd".to_string(),
                    req.cwd.display().to_string(),
                    "-a".to_string(),
                    "never".to_string(),
                    "-s".to_string(),
                    map_codex_permission_mode(&req.scenario.runner.permission_mode).to_string(),
                    "-m".to_string(),
                    req.scenario.runner.model.clone(),
                    "-".to_string(),
                ],
                &prompt,
            )?
        } else {
            let session = session_id.clone().unwrap_or_else(|| "--last".to_string());
            run_process_with_stdin(
                "codex",
                &[
                    "exec".to_string(),
                    "resume".to_string(),
                    session,
                    "--json".to_string(),
                    "-".to_string(),
                ],
                &prompt,
            )?
        };
        let parsed = parse_codex_jsonl(&stdout, max_turns, max_turns_user_set)?;
        if session_id.is_none() {
            session_id = parsed.session_id.clone();
        }
        merge_runtime_result(&mut combined, parsed);
    }
    combined.session_id = session_id.or(combined.session_id);
    if combined.stopped_reason == "other" && combined.errors.is_empty() {
        combined.stopped_reason = "end_turn".to_string();
    }
    Ok(combined)
}

fn run_claude(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    let max_turns = req
        .scenario
        .max_turns
        .unwrap_or(crate::config::INTERNAL_MAX_TURNS);
    let max_turns_user_set = req.scenario.max_turns.is_some();
    let mut combined = RuntimeRunResult::new(max_turns, max_turns_user_set);
    let mut session_id: Option<String> = None;
    for (idx, user_message) in req.user_messages.iter().enumerate() {
        let prompt = if idx == 0 {
            build_claude_input(
                &req.skill_body,
                req.skill_install_rel_path.as_deref(),
                user_message,
            )
        } else {
            user_message.clone()
        };
        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--include-partial-messages".to_string(),
            "--model".to_string(),
            req.scenario.runner.model.clone(),
            "--permission-mode".to_string(),
            req.scenario.runner.permission_mode.clone(),
            "--max-turns".to_string(),
            max_turns.to_string(),
        ];
        if let Some(session) = &session_id {
            args.push("--resume".to_string());
            args.push(session.clone());
        }
        args.push(prompt);
        let stdout = run_process("claude", &args, Some(&req.cwd), None)?;
        let parsed = parse_claude_jsonl(&stdout, max_turns, max_turns_user_set)?;
        if session_id.is_none() {
            session_id = parsed.session_id.clone();
        }
        merge_runtime_result(&mut combined, parsed);
    }
    combined.session_id = session_id.or(combined.session_id);
    if combined.stopped_reason == "other" && combined.errors.is_empty() {
        combined.stopped_reason = "end_turn".to_string();
    }
    Ok(combined)
}

fn run_process_with_stdin(command: &str, args: &[String], stdin: &str) -> anyhow::Result<String> {
    run_process(command, args, None, Some(stdin))
}

fn run_process(
    command: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    stdin: Option<&str>,
) -> anyhow::Result<String> {
    let mut process = platform_command(command, args);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    let mut child = process
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = stdin {
        child
            .stdin
            .as_mut()
            .expect("stdin piped")
            .write_all(stdin.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        anyhow::bail!(
            "{command} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(windows)]
fn platform_command(command: &str, args: &[String]) -> Command {
    let mut cmd = Command::new("cmd");
    let line = std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .map(|part| quote_cmd_arg(&part))
        .collect::<Vec<_>>()
        .join(" ");
    cmd.args(["/C", &line]);
    cmd
}

#[cfg(windows)]
fn quote_cmd_arg(arg: &str) -> String {
    if arg.is_empty()
        || arg
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '&' | '|' | '<' | '>' | '^'))
    {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

#[cfg(not(windows))]
fn platform_command(command: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd
}

fn merge_runtime_result(target: &mut RuntimeRunResult, mut source: RuntimeRunResult) {
    let offset = target.turns.len();
    for turn in &mut source.turns {
        turn.index += offset;
    }
    target.turns_used += source.turns_used;
    target.turns.extend(source.turns);
    if !source.final_output.is_empty() {
        target.final_output = source.final_output;
    }
    target.cost.input_tokens += source.cost.input_tokens;
    target.cost.output_tokens += source.cost.output_tokens;
    target.cost.cache_creation_tokens += source.cost.cache_creation_tokens;
    target.cost.cache_read_tokens += source.cost.cache_read_tokens;
    target.cost.usd_estimate += source.cost.usd_estimate;
    target.cost.source = source.cost.source;
    target.unanswered_questions += source.unanswered_questions;
    target.errors.extend(source.errors);
    target.diagnostics.extend(source.diagnostics);
    if target.session_id.is_none() {
        target.session_id = source.session_id;
    }
    target.stopped_reason = source.stopped_reason;
}

fn build_codex_input(
    skill_body: &str,
    skill_install_rel_path: Option<&str>,
    user_message: &str,
) -> String {
    let mut parts = vec![skill_body.to_string()];
    if let Some(path) = skill_install_rel_path {
        parts.push(format!(
            "---\n\n## Skill installation context (ai-tester)\n\nThis skill is installed at `{path}` relative to the current working directory."
        ));
    }
    parts.push(format!("---\n\n## User request\n\n{user_message}"));
    parts.join("\n\n")
}

fn build_claude_input(
    skill_body: &str,
    skill_install_rel_path: Option<&str>,
    user_message: &str,
) -> String {
    build_codex_input(skill_body, skill_install_rel_path, user_message)
}

fn map_codex_permission_mode(mode: &str) -> &'static str {
    match mode {
        "bypassPermissions" => "danger-full-access",
        "acceptEdits" => "workspace-write",
        "plan" => "read-only",
        _ => "workspace-write",
    }
}

fn preflight(name: &'static str, description: &'static str) -> RuntimeStatus {
    let ready = command_exists(name);
    RuntimeStatus {
        name,
        description,
        ready,
        message: if ready {
            None
        } else {
            Some(format!("`{name}` CLI not found on PATH"))
        },
    }
}

fn command_exists(name: &str) -> bool {
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = Command::new("where");
        cmd.arg(name);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("which");
        cmd.arg(name);
        cmd
    };
    cmd.output().is_ok_and(|out| out.status.success())
}
