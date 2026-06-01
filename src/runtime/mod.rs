use std::collections::HashMap;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::scenario::{Scenario, UserResponse};
use crate::trace::{ToolCallRecord, TraceCost, TraceError, Turn, TurnUsage};
use crate::ui::{self, Tone};

pub mod acp;

pub struct RuntimeStatus {
    pub name: String,
    pub description: String,
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

#[derive(Debug, Clone, Copy)]
enum ProcessOutputMode {
    Buffered,
    StreamJsonl,
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
    parse_claude_jsonl_with_user_responses(jsonl, max_turns_effective, max_turns_user_set, &[])
}

pub fn parse_claude_jsonl_with_user_responses(
    jsonl: &str,
    max_turns_effective: u32,
    max_turns_user_set: bool,
    user_responses: &[UserResponse],
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
            "assistant" => handle_claude_assistant(&event, &mut out, user_responses),
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

fn handle_claude_assistant(
    event: &Value,
    out: &mut RuntimeRunResult,
    user_responses: &[UserResponse],
) {
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
                    let mut call = ToolCallRecord::new(
                        block.get("id").and_then(Value::as_str).unwrap_or_default(),
                        block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        block
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    );
                    annotate_question_call(&mut call, user_responses, out);
                    turn.tool_calls.push(call);
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

fn annotate_question_call(
    call: &mut ToolCallRecord,
    user_responses: &[UserResponse],
    out: &mut RuntimeRunResult,
) {
    if !matches!(call.name.as_str(), "AskUserQuestion" | "Questions") {
        return;
    }
    let question = question_text(&call.input);
    out.unanswered_questions += 1;
    if !user_responses.is_empty() {
        out.diagnostics.push(format!(
            "Claude subprocess adapter cannot deliver user_responses to `{}` question: {}",
            call.name, question
        ));
    }
}

fn question_text(input: &Value) -> String {
    if let Some(question) = input.get("question").and_then(Value::as_str) {
        return question.to_string();
    }
    if let Some(questions) = input.get("questions") {
        return match questions {
            Value::String(value) => value.clone(),
            Value::Array(items) => items
                .iter()
                .map(|item| {
                    item.get("question")
                        .or_else(|| item.get("text"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| stringify_value(item))
                })
                .collect::<Vec<_>>()
                .join("\n"),
            other => stringify_value(other),
        };
    }
    stringify_value(input)
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

pub fn list_runtime_statuses(config: &crate::config::ProjectConfig) -> Vec<RuntimeStatus> {
    let mut statuses = vec![
        preflight(
            "claude",
            "Claude Code via `claude -p --output-format stream-json`.",
        ),
        preflight("codex", "OpenAI Codex via `codex exec --json`."),
    ];
    for (name, agent) in &config.acp_agents {
        statuses.push(preflight_dynamic(
            format!("acp:{name}"),
            format!(
                "ACP agent via `{}`{}.",
                agent.command,
                if agent.args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", agent.args.join(" "))
                }
            ),
            &agent.command,
        ));
    }
    statuses
}

pub fn runtime_status_for_scenario(
    scenario: &Scenario,
    config: &crate::config::ProjectConfig,
) -> RuntimeStatus {
    match scenario.runner.runtime.as_str() {
        "acp" => {
            let Some(agent_name) = scenario.runner.agent.as_deref() else {
                return RuntimeStatus {
                    name: "acp".to_string(),
                    description: "Generic Agent Client Protocol runtime.".to_string(),
                    ready: false,
                    message: Some(
                        "`runtime: acp` requires `runner.agent`, `defaults.agent`, or `--agent`"
                            .to_string(),
                    ),
                };
            };
            let Some(agent) = config.acp_agents.get(agent_name) else {
                return RuntimeStatus {
                    name: format!("acp:{agent_name}"),
                    description: "Configured ACP agent.".to_string(),
                    ready: false,
                    message: Some(format!(
                        "`runtime: acp` references unknown agent `{agent_name}`"
                    )),
                };
            };
            preflight_dynamic(
                format!("acp:{agent_name}"),
                format!("ACP agent via `{}`.", agent.command),
                &agent.command,
            )
        }
        "claude" => preflight(
            "claude",
            "Claude Code via `claude -p --output-format stream-json`.",
        ),
        "codex" => preflight("codex", "OpenAI Codex via `codex exec --json`."),
        other => RuntimeStatus {
            name: other.to_string(),
            description: "Unknown runtime.".to_string(),
            ready: false,
            message: Some(format!("unknown runtime `{other}`")),
        },
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeRunRequest {
    pub runtime: String,
    pub skill_body: String,
    pub scenario: Scenario,
    pub cwd: PathBuf,
    pub user_messages: Vec<String>,
    pub user_responses: Vec<UserResponse>,
    pub allowed_tools: Vec<String>,
    pub skill_install_rel_path: Option<String>,
    pub progress: bool,
    pub idle_warn_seconds: u64,
    pub acp_agent_name: Option<String>,
    pub acp_agent: Option<crate::config::AcpAgentConfig>,
    pub mcp_servers: Vec<crate::config::NamedMcpServerConfig>,
    pub acp_config: AcpConfigRequest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpConfigRequest {
    pub model: Option<String>,
    pub mode: Option<String>,
    pub reasoning: Option<String>,
}

pub fn runtime_ready(name: &str) -> bool {
    command_exists(name)
}

pub fn run_runtime(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    match req.runtime.as_str() {
        "acp" => acp::run_acp(req),
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
            let args = build_codex_args(&req);
            run_process_with_stdin_jsonl("codex", &args, &prompt, req.progress)?
        } else {
            let session = session_id.clone().unwrap_or_else(|| "--last".to_string());
            let args = vec![
                "exec".to_string(),
                "resume".to_string(),
                session,
                "--json".to_string(),
                "-".to_string(),
            ];
            run_process_with_stdin_jsonl("codex", &args, &prompt, req.progress)?
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

fn build_codex_args(req: &RuntimeRunRequest) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "--cd".to_string(),
        req.cwd.display().to_string(),
    ];
    append_codex_permission_args(&mut args, &req.scenario.runner.permission_mode);
    args.extend([
        "-m".to_string(),
        req.scenario.runner.model.clone(),
        "-".to_string(),
    ]);
    args
}

fn append_codex_permission_args(args: &mut Vec<String>, mode: &str) {
    if mode == "bypassPermissions" {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    } else {
        args.push("-s".to_string());
        args.push(map_codex_permission_mode(mode).to_string());
    }
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
        let args = build_claude_args(
            &req,
            max_turns,
            session_id.as_deref(),
            user_message,
            idx == 0,
        );
        let stdout = run_process(
            "claude",
            &args,
            Some(&req.cwd),
            None,
            if req.progress {
                ProcessOutputMode::StreamJsonl
            } else {
                ProcessOutputMode::Buffered
            },
        )?;
        let parsed = parse_claude_jsonl_with_user_responses(
            &stdout,
            max_turns,
            max_turns_user_set,
            &req.user_responses,
        )?;
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

fn run_process_with_stdin_jsonl(
    command: &str,
    args: &[String],
    stdin: &str,
    progress: bool,
) -> anyhow::Result<String> {
    run_process(
        command,
        args,
        None,
        Some(stdin),
        if progress {
            ProcessOutputMode::StreamJsonl
        } else {
            ProcessOutputMode::Buffered
        },
    )
}

fn run_process(
    command: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    stdin: Option<&str>,
    output_mode: ProcessOutputMode,
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
    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let stderr_handle = std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut buffer);
        buffer
    });

    let stdout = match output_mode {
        ProcessOutputMode::Buffered => read_stdout_buffered(stdout)?,
        ProcessOutputMode::StreamJsonl => read_stdout_streaming_jsonl(stdout)?,
    };

    let status = child.wait()?;
    let stderr = stderr_handle.join().unwrap_or_default();
    if !status.success() {
        anyhow::bail!(
            "{command} failed ({}): {}",
            exit_label(&status),
            failure_detail(&stderr, &stdout)
        );
    }
    Ok(stdout)
}

fn exit_label(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "killed by signal".to_string(),
    }
}

/// Build a human-readable failure detail. CLIs like `claude`/`codex` often write
/// the actual error to stdout (JSON events) and leave stderr empty, so fall back
/// to stdout when stderr has nothing useful.
fn failure_detail(stderr: &str, stdout: &str) -> String {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return truncate_detail(stderr);
    }
    if let Some(msg) = error_from_jsonl(stdout) {
        return truncate_detail(&msg);
    }
    let tail = stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ");
    if tail.is_empty() {
        "no output on stdout/stderr".to_string()
    } else {
        truncate_detail(&tail)
    }
}

/// Scan JSONL/JSON stdout for error-ish string fields and join what we find.
fn error_from_jsonl(stdout: &str) -> Option<String> {
    let mut messages = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let is_error = value.get("is_error").and_then(Value::as_bool) == Some(true)
            || value
                .get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|s| s.contains("error"))
            || value.get("type").and_then(Value::as_str) == Some("error")
            || value.get("error").is_some();
        if !is_error {
            continue;
        }
        for key in ["error", "message", "result"] {
            if let Some(text) = value.get(key).and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    messages.push(text.trim().to_string());
                    break;
                }
            }
            if let Some(text) = value
                .get(key)
                .and_then(|v| v.get("message"))
                .and_then(Value::as_str)
            {
                if !text.trim().is_empty() {
                    messages.push(text.trim().to_string());
                    break;
                }
            }
        }
    }
    if messages.is_empty() {
        None
    } else {
        messages.dedup();
        Some(messages.join(" | "))
    }
}

fn truncate_detail(detail: &str) -> String {
    const MAX: usize = 600;
    let normalized = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX {
        normalized
    } else {
        let head = normalized.chars().take(MAX).collect::<String>();
        format!("{head}…")
    }
}

fn read_stdout_buffered(stdout: impl Read) -> anyhow::Result<String> {
    let mut buffer = String::new();
    let mut reader = BufReader::new(stdout);
    reader.read_to_string(&mut buffer)?;
    Ok(buffer)
}

fn read_stdout_streaming_jsonl(stdout: impl Read) -> anyhow::Result<String> {
    let mut captured = String::new();
    let mut progress = JsonlProgress::new();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line?;
        progress.print_line(&line);
        captured.push_str(&line);
        captured.push('\n');
    }
    progress.finish();
    Ok(captured)
}

struct JsonlProgress {
    live: Option<LiveProgress>,
    active_items: HashMap<String, String>,
}

impl JsonlProgress {
    fn new() -> Self {
        Self {
            live: LiveProgress::new(),
            active_items: HashMap::new(),
        }
    }

    fn print_line(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "turn.started" => {
                if let Some(live) = &self.live {
                    live.set_status("Thinking");
                } else {
                    println!("    {} started", ui::tag("turn", Tone::Info));
                }
            }
            "turn.completed" => {
                if self.live.is_some() {
                    self.completed("Turn completed", Tone::Success);
                } else {
                    println!("    {} completed", ui::tag("turn", Tone::Success));
                }
            }
            "turn.failed" => {
                if self.live.is_some() {
                    self.completed("Turn failed", Tone::Error);
                } else {
                    println!("    {} failed", ui::tag("turn", Tone::Error));
                }
            }
            "item.started" => {
                if let Some(item) = event.get("item") {
                    self.item_started(item);
                }
            }
            "item.completed" => {
                if let Some(item) = event.get("item") {
                    self.item_completed(item);
                }
            }
            "assistant" => self.claude_assistant(&event),
            "user" => self.claude_user(&event),
            "result" => {
                if event.get("subtype").and_then(Value::as_str) == Some("success") {
                    if self.live.is_some() {
                        self.completed("Runtime result received", Tone::Success);
                    }
                } else {
                    self.status("Runtime finished");
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self) {
        if let Some(live) = &mut self.live {
            live.finish();
        }
    }

    fn status(&mut self, text: &str) {
        if let Some(live) = &self.live {
            live.set_status(text);
        } else {
            println!(
                "    {} {}",
                ui::tag("turn", Tone::Info),
                text.to_ascii_lowercase()
            );
        }
    }

    fn item_started(&mut self, item: &Value) {
        let label = item_progress_label(item);
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            self.active_items.insert(id.to_string(), label.clone());
        }
        if let Some(live) = &self.live {
            live.set_status(&label);
        } else {
            print_item_progress("started", item);
        }
    }

    fn item_completed(&mut self, item: &Value) {
        let label = item
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| self.active_items.remove(id))
            .unwrap_or_else(|| item_progress_label(item));
        if self.live.is_some() {
            self.completed(&label, item_progress_tone(item, true));
        } else {
            print_item_progress("completed", item);
        }
    }

    fn claude_assistant(&mut self, event: &Value) {
        let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
            let label = tool_call_progress_label(
                block.get("name").and_then(Value::as_str).unwrap_or("tool"),
                block.get("input").unwrap_or(&Value::Null),
            );
            if !id.is_empty() {
                self.active_items.insert(id.to_string(), label.clone());
            }
            if let Some(live) = &self.live {
                live.set_status(&label);
            } else {
                println!("    {} {label} started", ui::tag("tool", Tone::Accent));
            }
        }
    }

    fn claude_user(&mut self, event: &Value) {
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
            let label = self
                .active_items
                .remove(id)
                .unwrap_or_else(|| "Tool call".to_string());
            let tone = if block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                Tone::Error
            } else {
                Tone::Success
            };
            if self.live.is_some() {
                self.completed(&label, tone);
            } else {
                println!("    {} {label} completed", ui::tag("tool", tone));
            }
        }
    }

    fn completed(&mut self, text: &str, tone: Tone) {
        if let Some(live) = &self.live {
            live.print_completed(text, tone);
            live.set_status("Processing");
        } else {
            let tag_tone = if matches!(tone, Tone::Error) {
                Tone::Error
            } else {
                Tone::Success
            };
            println!(
                "    {} {}",
                ui::tag("turn", tag_tone),
                text.to_ascii_lowercase()
            );
        }
    }
}

struct LiveProgress {
    status: Arc<Mutex<String>>,
    output_lock: Arc<Mutex<()>>,
    needs_gap: Arc<AtomicBool>,
    gap_rendered: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LiveProgress {
    fn new() -> Option<Self> {
        if !std::io::stdout().is_terminal() && !truthy_env("AI_TESTER_FORCE_PROGRESS") {
            return None;
        }
        let status = Arc::new(Mutex::new("Starting".to_string()));
        let output_lock = Arc::new(Mutex::new(()));
        let needs_gap = Arc::new(AtomicBool::new(true));
        let gap_rendered = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let thread_status = Arc::clone(&status);
        let thread_output_lock = Arc::clone(&output_lock);
        let thread_needs_gap = Arc::clone(&needs_gap);
        let thread_gap_rendered = Arc::clone(&gap_rendered);
        let thread_done = Arc::clone(&done);
        let handle = thread::spawn(move || {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut idx = 0usize;
            while !thread_done.load(Ordering::Relaxed) {
                let message = thread_status
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_else(|_| "Working".to_string());
                let message = truncate_progress_for_terminal(&message, 8);
                if let Ok(_guard) = thread_output_lock.lock() {
                    if thread_needs_gap.swap(false, Ordering::Relaxed) {
                        println!();
                        thread_gap_rendered.store(true, Ordering::Relaxed);
                    }
                    print!(
                        "\r\x1b[2K    {} {}",
                        ui::paint(frames[idx % frames.len()], Tone::Info),
                        paint_progress_label(&message, Tone::Muted)
                    );
                    let _ = std::io::stdout().flush();
                }
                idx = idx.wrapping_add(1);
                thread::sleep(Duration::from_millis(90));
            }
            if let Ok(_guard) = thread_output_lock.lock() {
                if thread_gap_rendered.swap(false, Ordering::Relaxed) {
                    print!("\r\x1b[2K\x1b[1A\r\x1b[2K");
                }
                print!("\r\x1b[2K");
                let _ = std::io::stdout().flush();
            }
        });
        Some(Self {
            status,
            output_lock,
            needs_gap,
            gap_rendered,
            done,
            handle: Some(handle),
        })
    }

    fn set_status(&self, text: &str) {
        if let Ok(mut status) = self.status.lock() {
            *status = truncate_progress_value(text, 300);
        }
    }

    fn print_completed(&self, text: &str, tone: Tone) {
        if let Ok(_guard) = self.output_lock.lock() {
            let text = truncate_progress_for_terminal(text, 8);
            if self.gap_rendered.swap(false, Ordering::Relaxed) {
                print!("\r\x1b[2K\x1b[1A\r\x1b[2K");
            } else {
                print!("\r\x1b[2K");
            }
            println!(
                "    {} {}",
                ui::paint("●", tone),
                paint_progress_label(&text, Tone::Strong)
            );
            self.needs_gap.store(true, Ordering::Relaxed);
            let _ = std::io::stdout().flush();
        }
    }

    fn finish(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LiveProgress {
    fn drop(&mut self) {
        self.finish();
    }
}

fn print_item_progress(state: &str, item: &Value) {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "command_execution" => {
            let suffix = item
                .get("command")
                .and_then(Value::as_str)
                .map(|command| format!(": {}", truncate_progress_value(command, 140)))
                .unwrap_or_default();
            println!("    {} Bash {state}{suffix}", ui::tag("tool", Tone::Accent));
        }
        "file_change" => {
            if let Some(changes) = item.get("changes").and_then(Value::as_array) {
                let paths = changes
                    .iter()
                    .filter_map(|change| change.get("path").and_then(Value::as_str))
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    println!(
                        "    {} changes {state}: {}",
                        ui::tag("file", Tone::Warning),
                        truncate_progress_value(&paths.join(", "), 140)
                    );
                }
            }
        }
        "agent_message" if state == "completed" => {
            println!(
                "    {} message completed",
                ui::tag("assistant", Tone::Success)
            )
        }
        other if !other.is_empty() => {
            println!("    {} {other} {state}", ui::tag("item", Tone::Muted))
        }
        _ => {}
    }
}

fn item_progress_label(item: &Value) -> String {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            tool_call_progress_label("Bash", &serde_json::json!({ "command": command }))
        }
        "file_change" => {
            let paths = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if paths.is_empty() {
                "File changes".to_string()
            } else {
                format!("FileChange({})", truncate_progress_value(&paths, 120))
            }
        }
        "agent_message" => "Assistant message".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "Runtime event".to_string(),
    }
}

fn item_progress_tone(item: &Value, default_success: bool) -> Tone {
    if item.get("status").and_then(Value::as_str) == Some("failed") {
        Tone::Error
    } else if default_success {
        Tone::Success
    } else {
        Tone::Muted
    }
}

fn tool_call_progress_label(name: &str, input: &Value) -> String {
    let detail = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .or_else(|| input.get("command"))
        .or_else(|| input.get("pattern"))
        .and_then(Value::as_str)
        .map(|value| truncate_progress_value(value, 120))
        .or_else(|| {
            if input.is_object() && !input.as_object().is_some_and(|object| object.is_empty()) {
                Some(truncate_progress_value(&input.to_string(), 120))
            } else {
                None
            }
        });
    match detail {
        Some(detail) if !detail.is_empty() => format!("{name}({detail})"),
        _ => name.to_string(),
    }
}

fn paint_progress_label(text: &str, detail_tone: Tone) -> String {
    let Some(open_paren) = text.find('(') else {
        return ui::paint(text, detail_tone);
    };
    format!(
        "{}{}",
        ui::paint(&text[..open_paren], Tone::Warning),
        ui::paint(&text[open_paren..], detail_tone)
    )
}

fn truncate_progress_for_terminal(value: &str, reserved_cols: usize) -> String {
    let max_chars = terminal_columns()
        .saturating_sub(reserved_cols)
        .clamp(24, 120);
    truncate_progress_value(value, max_chars)
}

#[cfg(unix)]
fn terminal_columns() -> usize {
    use std::os::raw::{c_int, c_ulong};

    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: c_ulong = 0x4008_7468;
    #[cfg(not(target_os = "macos"))]
    const TIOCGWINSZ: c_ulong = 0x5413;

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }

    let mut winsize = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { ioctl(1, TIOCGWINSZ, &mut winsize) };
    if rc == 0 && winsize.ws_col > 0 {
        return winsize.ws_col as usize;
    }
    terminal_columns_from_env()
}

#[cfg(not(unix))]
fn terminal_columns() -> usize {
    terminal_columns_from_env()
}

fn terminal_columns_from_env() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
}

fn truncate_progress_value(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn truthy_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && value != "0" && value != "false" && value != "no"
    })
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

fn build_claude_args(
    req: &RuntimeRunRequest,
    max_turns: u32,
    session_id: Option<&str>,
    user_message: &str,
    include_system_prompt: bool,
) -> Vec<String> {
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
    if include_system_prompt {
        args.push("--append-system-prompt".to_string());
        args.push(build_claude_system_prompt(
            &req.skill_body,
            req.skill_install_rel_path.as_deref(),
        ));
    }
    if !req.allowed_tools.is_empty() {
        args.push("--allowedTools".to_string());
        args.push(req.allowed_tools.join(","));
    }
    if let Some(sources) = &req.scenario.runner.setting_sources {
        if !sources.is_empty() {
            args.push("--setting-sources".to_string());
            args.push(sources.join(","));
        }
    }
    if let Some(session) = session_id {
        args.push("--resume".to_string());
        args.push(session.to_string());
    }
    args.push(build_claude_user_prompt(
        req.skill_install_rel_path.as_deref(),
        user_message,
    ));
    args
}

fn build_claude_system_prompt(skill_body: &str, skill_install_rel_path: Option<&str>) -> String {
    let mut parts = vec![skill_body.to_string()];
    if let Some(path) = skill_install_rel_path {
        parts.push(format!(
            "## Skill installation context (ai-tester)\n\nThis skill is installed at `{path}` relative to the current working directory."
        ));
    }
    parts.join("\n\n")
}

fn build_claude_user_prompt(skill_install_rel_path: Option<&str>, user_message: &str) -> String {
    let mut parts = Vec::new();
    if let Some(path) = skill_install_rel_path {
        parts.push(format!(
            "## Skill installation context (ai-tester)\n\nThe active skill is installed at `{path}` relative to the current working directory."
        ));
    }
    parts.push(format!("## User request\n\n{user_message}"));
    parts.join("\n\n---\n\n")
}

fn map_codex_permission_mode(mode: &str) -> &'static str {
    match mode {
        "bypassPermissions" => "danger-full-access",
        "acceptEdits" => "workspace-write",
        "plan" => "read-only",
        _ => "workspace-write",
    }
}

fn preflight(name: &str, description: &str) -> RuntimeStatus {
    preflight_dynamic(name.to_string(), description.to_string(), name)
}

fn preflight_dynamic(name: String, description: String, command: &str) -> RuntimeStatus {
    let ready = command_exists(command);
    RuntimeStatus {
        name,
        description,
        ready,
        message: if ready {
            None
        } else {
            Some(format!("`{command}` CLI not found on PATH"))
        },
    }
}

fn command_exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file();
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Runner;

    #[test]
    fn failure_detail_prefers_stderr() {
        let detail = failure_detail("  boom on stderr  ", "ignored stdout");
        assert_eq!(detail, "boom on stderr");
    }

    #[test]
    fn failure_detail_extracts_claude_result_error() {
        let stdout = r#"{"type":"system","subtype":"init"}
{"type":"result","subtype":"error_during_execution","is_error":true,"result":"model gpt-5.4 not found"}"#;
        let detail = failure_detail("", stdout);
        assert_eq!(detail, "model gpt-5.4 not found");
    }

    #[test]
    fn failure_detail_extracts_codex_turn_error() {
        let stdout = r#"{"type":"turn.started"}
{"type":"turn.failed","error":{"message":"usage limit reached"}}"#;
        let detail = failure_detail("", stdout);
        assert_eq!(detail, "usage limit reached");
    }

    #[test]
    fn failure_detail_falls_back_to_stdout_tail() {
        let detail = failure_detail("", "first\nplain text error\n");
        assert_eq!(detail, "first | plain text error");
    }

    #[test]
    fn failure_detail_handles_empty_output() {
        assert_eq!(failure_detail("", ""), "no output on stdout/stderr");
    }

    #[test]
    fn claude_args_put_skill_in_system_prompt_and_scope_tools() {
        let mut scenario = Scenario::from_yaml_str(
            "scenario: claude-args\nsystem_prompt: Body\nrunner:\n  runtime: claude\n  model: test-model\n  permission_mode: plan\n  setting_sources: [project]\n",
        )
        .expect("scenario parses");
        scenario.runner = Runner {
            allowed_tools_override: None,
            ..scenario.runner
        };
        let req = RuntimeRunRequest {
            runtime: "claude".to_string(),
            skill_body: "SKILL BODY".to_string(),
            scenario,
            cwd: PathBuf::from("."),
            user_messages: vec!["do it".to_string()],
            user_responses: Vec::new(),
            allowed_tools: vec!["Read".to_string(), "Bash(git *)".to_string()],
            skill_install_rel_path: Some(".claude/skills/demo/SKILL.md".to_string()),
            progress: false,
            idle_warn_seconds: 30,
            acp_agent_name: None,
            acp_agent: None,
            mcp_servers: Vec::new(),
            acp_config: AcpConfigRequest::default(),
        };

        let args = build_claude_args(&req, 3, None, "do it", true);
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--model" && pair[1] == "test-model"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--permission-mode" && pair[1] == "plan"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--allowedTools" && pair[1] == "Read,Bash(git *)"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--setting-sources" && pair[1] == "project"));
        let system_prompt = args
            .windows(2)
            .find(|pair| pair[0] == "--append-system-prompt")
            .map(|pair| pair[1].as_str())
            .expect("append system prompt arg exists");
        assert!(system_prompt.contains("SKILL BODY"));
        let user_prompt = args.last().expect("prompt exists");
        assert!(user_prompt.contains("do it"));
        assert!(!user_prompt.contains("SKILL BODY"));
    }
}
