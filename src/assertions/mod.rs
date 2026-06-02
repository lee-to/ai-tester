use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scenario::AssertionSpec;
use crate::trace::{ToolCallRecord, TraceRecord};
use crate::util::path as path_util;
use crate::util::regex::compile_pattern;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub pass: bool,
    pub detail: String,
    pub weight: f64,
    pub score: Option<f64>,
    pub min_score: Option<f64>,
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<CaptureRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRecord {
    pub field: String,
    pub value: String,
    pub truncated: bool,
    pub original_length: usize,
    pub step: Option<usize>,
}

pub fn evaluate_assertions(
    assertions: &[AssertionSpec],
    trace: &TraceRecord,
) -> Vec<AssertionResult> {
    let mut out = assertions
        .iter()
        .map(|spec| evaluate_assertion(spec, trace))
        .collect::<Vec<_>>();
    out.push(evaluate_no_unanswered_questions(trace));
    if let Some(result) = evaluate_token_budget(trace) {
        out.push(result);
    }
    out
}

pub fn compute_weighted_score(results: &[AssertionResult]) -> f64 {
    let total: f64 = results.iter().map(|r| r.weight).sum();
    if total <= f64::EPSILON {
        return 1.0;
    }
    let passed: f64 = results
        .iter()
        .filter(|r| r.pass)
        .map(|r| r.score.unwrap_or(r.weight))
        .sum();
    passed / total
}

fn evaluate_assertion(spec: &AssertionSpec, trace: &TraceRecord) -> AssertionResult {
    match spec {
        AssertionSpec::ToolCalled {
            id,
            weight,
            tool,
            tool_pattern,
            tool_kind,
            title_pattern,
            args_match,
            raw_input_match,
            call_index,
            capture,
            capture_max_chars,
        } => evaluate_tool_called(
            id,
            *weight,
            tool.as_deref(),
            tool_pattern.as_deref(),
            tool_kind.as_deref(),
            title_pattern.as_deref(),
            args_match.as_ref(),
            raw_input_match.as_ref(),
            *call_index,
            capture.as_deref(),
            *capture_max_chars,
            trace,
        ),
        AssertionSpec::ToolCallSequence {
            id,
            weight,
            sequence,
            capture_max_chars,
        } => {
            let mut next_index = 0usize;
            let mut captures = Vec::new();
            let calls = all_tool_calls(trace);
            for (step_index, step) in sequence.iter().enumerate() {
                let matcher = match ToolCallMatcher::new(
                    step.tool.as_deref(),
                    None,
                    step.tool_kind.as_deref(),
                    step.title_pattern.as_deref(),
                    step.args_match.as_ref(),
                    step.raw_input_match.as_ref(),
                ) {
                    Ok(matcher) => matcher,
                    Err(err) => return base_result(id, "tool_call_sequence", false, *weight, err),
                };
                let found = calls[next_index..]
                    .iter()
                    .position(|call| matcher.matches(call));
                let Some(offset) = found else {
                    return base_result(
                        id,
                        "tool_call_sequence",
                        false,
                        *weight,
                        format!("missing sequence step for {}", matcher.description()),
                    );
                };
                let matched_call = calls[next_index + offset];
                captures.extend(capture_fields(
                    &matched_call.input,
                    step.capture.as_deref(),
                    *capture_max_chars,
                    Some(step_index + 1),
                ));
                next_index += offset + 1;
            }
            with_captures(
                base_result(
                    id,
                    "tool_call_sequence",
                    true,
                    *weight,
                    "sequence matched".to_string(),
                ),
                captures,
            )
        }
        AssertionSpec::NoToolCalled {
            id,
            weight,
            tool,
            tool_pattern,
            tool_kind,
            title_pattern,
            args_match: expected_args,
            raw_input_match,
        } => {
            let matcher = match ToolCallMatcher::new(
                tool.as_deref(),
                tool_pattern.as_deref(),
                tool_kind.as_deref(),
                title_pattern.as_deref(),
                expected_args.as_ref(),
                raw_input_match.as_ref(),
            ) {
                Ok(matcher) => matcher,
                Err(err) => return base_result(id, "no_tool_called", false, *weight, err),
            };
            let matched = all_tool_calls(trace)
                .into_iter()
                .find(|call| matcher.matches(call));
            if let Some(call) = matched {
                base_result(
                    id,
                    "no_tool_called",
                    false,
                    *weight,
                    format!("unexpected `{}` call matched", call.name),
                )
            } else {
                base_result(
                    id,
                    "no_tool_called",
                    true,
                    *weight,
                    "no matching calls found".to_string(),
                )
            }
        }
        AssertionSpec::OutputContains {
            id,
            weight,
            pattern,
        } => match compile_pattern(pattern) {
            Ok(re) if re.is_match(&trace.final_output) => base_result(
                id,
                "output_contains",
                true,
                *weight,
                "final output matched".to_string(),
            ),
            Ok(_) => base_result(
                id,
                "output_contains",
                false,
                *weight,
                "final output did not match pattern".to_string(),
            ),
            Err(err) => base_result(
                id,
                "output_contains",
                false,
                *weight,
                format!("invalid regex: {err}"),
            ),
        },
        AssertionSpec::NoOutputContains {
            id,
            weight,
            pattern,
        } => match compile_pattern(pattern) {
            Ok(re) if re.is_match(&trace.final_output) => base_result(
                id,
                "no_output_contains",
                false,
                *weight,
                "final output matched unexpected pattern".to_string(),
            ),
            Ok(_) => base_result(
                id,
                "no_output_contains",
                true,
                *weight,
                "final output did not match pattern".to_string(),
            ),
            Err(err) => base_result(
                id,
                "no_output_contains",
                false,
                *weight,
                format!("invalid regex: {err}"),
            ),
        },
        AssertionSpec::FileRead { id, weight, path } => {
            evaluate_file_read(id, *weight, path, trace)
        }
        AssertionSpec::TurnCountAtMost { id, weight, max } => {
            let pass = trace.runner.turns_used <= *max;
            base_result(
                id,
                "turn_count_at_most",
                pass,
                *weight,
                if pass {
                    format!("{} turns ≤ max {max}", trace.runner.turns_used)
                } else {
                    format!("{} turns exceeds max {max}", trace.runner.turns_used)
                },
            )
        }
        AssertionSpec::NoPathEscape {
            id,
            weight,
            tools,
            allow_outside,
        } => evaluate_no_path_escape(
            id,
            *weight,
            tools.as_deref(),
            allow_outside.as_deref(),
            trace,
        ),
    }
}

fn evaluate_file_read(
    id: &str,
    weight: f64,
    path_pattern: &str,
    trace: &TraceRecord,
) -> AssertionResult {
    let path_re = match compile_pattern(path_pattern) {
        Ok(pattern) => pattern,
        Err(err) => {
            return base_result(
                id,
                "file_read",
                false,
                weight,
                format!("invalid path regex: {err}"),
            )
        }
    };

    let matched = all_tool_calls(trace)
        .into_iter()
        .find(|call| file_read_call_matches(call, &path_re));

    if let Some(call) = matched {
        base_result(
            id,
            "file_read",
            true,
            weight,
            format!("found file read via `{}`", call.name),
        )
    } else {
        base_result(
            id,
            "file_read",
            false,
            weight,
            format!("no file read matched `{path_pattern}`"),
        )
    }
}

fn file_read_call_matches(call: &ToolCallRecord, path_re: &regex::Regex) -> bool {
    match call.name.as_str() {
        "Read" | "NotebookRead" => call
            .input
            .get("file_path")
            .map(value_to_string)
            .is_some_and(|path| path_re.is_match(&path)),
        "Bash" => call
            .input
            .get("command")
            .map(value_to_string)
            .is_some_and(|command| bash_command_reads_path(&command, path_re)),
        "fs/read_text_file" => call
            .input
            .get("path")
            .map(value_to_string)
            .is_some_and(|path| path_re.is_match(&path)),
        _ => false,
    }
}

fn bash_command_reads_path(command: &str, path_re: &regex::Regex) -> bool {
    if !path_re.is_match(command) {
        return false;
    }

    compile_pattern(r#"(?i)(^|[\s'";|(&])(?:awk|bat|cat|grep|head|less|more|nl|rg|sed|tail)\b"#)
        .is_ok_and(|reader| reader.is_match(command))
}

fn evaluate_no_path_escape(
    id: &str,
    weight: f64,
    tools: Option<&[String]>,
    allow_outside: Option<&[String]>,
    trace: &TraceRecord,
) -> AssertionResult {
    let Some(sandbox_path) = trace.runner.sandbox_path.as_deref() else {
        return base_result(
            id,
            "no_path_escape",
            false,
            weight,
            "sandbox path is missing from trace".to_string(),
        );
    };

    let sandbox = path_util::canonicalize_existing(Path::new(sandbox_path))
        .unwrap_or_else(|_| path_util::normalize_path_lexical(Path::new(sandbox_path)));
    let mut allowed_roots = vec![sandbox.clone()];
    if let Some(extra_roots) = allow_outside {
        allowed_roots.extend(
            extra_roots
                .iter()
                .map(|path| resolve_allowed_root(&sandbox, path)),
        );
    }

    let mut violations = Vec::new();
    for call in all_tool_calls(trace) {
        if !tool_selected(call, tools) {
            continue;
        }
        for (field, raw_path) in tool_path_inputs(call) {
            match resolve_trace_path(&sandbox, &call.name, &raw_path) {
                Ok(resolved) => {
                    if !allowed_roots
                        .iter()
                        .any(|root| path_util::path_is_within(&resolved, root))
                    {
                        violations.push(format!("{}.{field} -> {raw_path}", call.name));
                    }
                }
                Err(err) => violations.push(format!("{}.{field} -> {raw_path} ({err})", call.name)),
            }
        }
    }

    if violations.is_empty() {
        base_result(
            id,
            "no_path_escape",
            true,
            weight,
            "all inspected tool paths stayed inside allowed roots".to_string(),
        )
    } else {
        base_result(
            id,
            "no_path_escape",
            false,
            weight,
            format!("path escape detected: {}", violations.join("; ")),
        )
    }
}

fn evaluate_tool_called(
    id: &str,
    weight: f64,
    tool: Option<&str>,
    tool_pattern: Option<&str>,
    tool_kind: Option<&str>,
    title_pattern: Option<&str>,
    expected_args: Option<&BTreeMap<String, String>>,
    raw_input_match: Option<&BTreeMap<String, String>>,
    call_index: Option<usize>,
    capture: Option<&[String]>,
    capture_max_chars: Option<usize>,
    trace: &TraceRecord,
) -> AssertionResult {
    let matcher = match ToolCallMatcher::new(
        tool,
        tool_pattern,
        tool_kind,
        title_pattern,
        expected_args,
        raw_input_match,
    ) {
        Ok(matcher) => matcher,
        Err(err) => return base_result(id, "tool_called", false, weight, err),
    };
    let matches = all_tool_calls(trace)
        .into_iter()
        .filter(|call| matcher.matches(call))
        .collect::<Vec<_>>();
    let pass = call_index
        .map(|idx| matches.get(idx).is_some())
        .unwrap_or(!matches.is_empty());
    let captures = call_index
        .and_then(|idx| matches.get(idx).copied())
        .or_else(|| matches.first().copied())
        .filter(|_| pass)
        .map(|call| capture_fields(&call.input, capture, capture_max_chars, None))
        .unwrap_or_default();
    with_captures(
        base_result(
            id,
            "tool_called",
            pass,
            weight,
            if pass {
                format!("found {} call", matcher.description())
            } else {
                format!("no {} call matched", matcher.description())
            },
        ),
        captures,
    )
}

fn capture_fields(
    input: &Value,
    fields: Option<&[String]>,
    max_chars: Option<usize>,
    step: Option<usize>,
) -> Vec<CaptureRecord> {
    let Some(fields) = fields else {
        return Vec::new();
    };
    fields
        .iter()
        .map(|field| {
            let raw = input.get(field).map(value_to_string).unwrap_or_default();
            let original_length = raw.chars().count();
            let (value, truncated) = match max_chars {
                Some(max) if original_length > max => {
                    (raw.chars().take(max).collect::<String>(), true)
                }
                _ => (raw, false),
            };
            CaptureRecord {
                field: field.clone(),
                value,
                truncated,
                original_length,
                step,
            }
        })
        .collect()
}

fn with_captures(mut result: AssertionResult, captures: Vec<CaptureRecord>) -> AssertionResult {
    result.captures = captures;
    result
}

fn evaluate_no_unanswered_questions(trace: &TraceRecord) -> AssertionResult {
    let unanswered = trace.tool_call_summary.unanswered_questions;
    base_result(
        "no_unanswered_questions",
        "no_unanswered_questions",
        unanswered == 0,
        1.0,
        if unanswered == 0 {
            "no supported question tool calls were left unanswered".to_string()
        } else {
            format!("{unanswered} question tool call(s) had no delivered answer")
        },
    )
}

fn evaluate_token_budget(trace: &TraceRecord) -> Option<AssertionResult> {
    let budget = trace.scenario.token_budget.or(trace.skill.token_budget)?;
    let total = trace.cost.total_tokens() as f64;
    Some(base_result(
        "token_budget",
        "token_budget",
        total <= budget,
        1.0,
        if total <= budget {
            format!("{} tokens ≤ budget of {}", total as u64, budget as u64)
        } else {
            format!(
                "{} tokens exceeds budget of {}",
                total as u64, budget as u64
            )
        },
    ))
}

#[derive(Debug)]
struct FieldMatchRegexError {
    matcher_name: &'static str,
    field: String,
    pattern: String,
    error: String,
}

struct FieldMatcher<'a> {
    patterns: Vec<(&'a str, regex::Regex)>,
}

impl<'a> FieldMatcher<'a> {
    fn new(
        expected: Option<&'a BTreeMap<String, String>>,
        matcher_name: &'static str,
    ) -> Result<Self, FieldMatchRegexError> {
        let Some(expected) = expected else {
            return Ok(Self {
                patterns: Vec::new(),
            });
        };
        let mut patterns = Vec::with_capacity(expected.len());
        for (field, pattern) in expected {
            let re = compile_pattern(pattern).map_err(|err| FieldMatchRegexError {
                matcher_name,
                field: field.clone(),
                pattern: pattern.clone(),
                error: err.to_string(),
            })?;
            patterns.push((field.as_str(), re));
        }
        Ok(Self { patterns })
    }

    fn matches(&self, input: &Value) -> bool {
        self.patterns.iter().all(|(field, re)| {
            let actual = value_at_path(input, field)
                .map(value_to_string)
                .unwrap_or_default();
            re.is_match(&actual)
        })
    }
}

struct ToolCallMatcher<'a> {
    tool: Option<&'a str>,
    tool_pattern: Option<regex::Regex>,
    tool_kind: Option<&'a str>,
    title_pattern: Option<regex::Regex>,
    args_matcher: FieldMatcher<'a>,
    raw_input_matcher: FieldMatcher<'a>,
}

impl<'a> ToolCallMatcher<'a> {
    fn new(
        tool: Option<&'a str>,
        tool_pattern: Option<&'a str>,
        tool_kind: Option<&'a str>,
        title_pattern: Option<&'a str>,
        args_match: Option<&'a BTreeMap<String, String>>,
        raw_input_match: Option<&'a BTreeMap<String, String>>,
    ) -> Result<Self, String> {
        let tool_pattern = match tool_pattern {
            Some(pattern) => Some(
                compile_pattern(pattern)
                    .map_err(|err| format!("invalid tool_pattern regex '{pattern}': {err}"))?,
            ),
            None => None,
        };
        let title_pattern = match title_pattern {
            Some(pattern) => Some(
                compile_pattern(pattern)
                    .map_err(|err| format!("invalid title_pattern regex '{pattern}': {err}"))?,
            ),
            None => None,
        };
        let args_matcher =
            FieldMatcher::new(args_match, "args_match").map_err(invalid_field_match_detail)?;
        let raw_input_matcher = FieldMatcher::new(raw_input_match, "raw_input_match")
            .map_err(invalid_field_match_detail)?;
        Ok(Self {
            tool,
            tool_pattern,
            tool_kind,
            title_pattern,
            args_matcher,
            raw_input_matcher,
        })
    }

    fn matches(&self, call: &ToolCallRecord) -> bool {
        self.primary_selector_matches(call)
            && self.title_matches(&call.input)
            && self.args_matcher.matches(&call.input)
            && self.raw_input_matcher.matches(raw_input_value(&call.input))
    }

    fn description(&self) -> String {
        if let Some(tool) = self.tool {
            return format!("`{tool}`");
        }
        if self.tool_pattern.is_some() {
            return "`<matching pattern>`".to_string();
        }
        if let Some(tool_kind) = self.tool_kind {
            return format!("ACP kind `{tool_kind}`");
        }
        "`<unspecified>`".to_string()
    }

    fn primary_selector_matches(&self, call: &ToolCallRecord) -> bool {
        if let Some(tool) = self.tool {
            return call.name == tool;
        }
        if let Some(pattern) = &self.tool_pattern {
            return pattern.is_match(&call.name);
        }
        if let Some(tool_kind) = self.tool_kind {
            return call.name == tool_kind
                || value_at_path(&call.input, "_acpKind")
                    .and_then(Value::as_str)
                    .is_some_and(|actual| actual == tool_kind);
        }
        false
    }

    fn title_matches(&self, input: &Value) -> bool {
        self.title_pattern.as_ref().is_none_or(|pattern| {
            let actual = value_at_path(input, "_acpTitle")
                .map(value_to_string)
                .unwrap_or_default();
            pattern.is_match(&actual)
        })
    }
}

fn invalid_field_match_detail(err: FieldMatchRegexError) -> String {
    format!(
        "invalid {} regex for '{}' ('{}'): {}",
        err.matcher_name, err.field, err.pattern, err.error
    )
}

fn value_at_path<'a>(input: &'a Value, path: &str) -> Option<&'a Value> {
    if path.starts_with('/') {
        return input.pointer(path);
    }
    if let Some(value) = input.get(path) {
        return Some(value);
    }
    value_at_dot_path(input, path)
}

fn value_at_dot_path<'a>(input: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = input;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(object) => object.get(segment)?,
            Value::Array(array) => array.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn raw_input_value(input: &Value) -> &Value {
    input.get("rawInput").unwrap_or(input)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn all_tool_calls(trace: &TraceRecord) -> Vec<&ToolCallRecord> {
    trace
        .turns
        .iter()
        .flat_map(|turn| turn.tool_calls.iter())
        .collect()
}

fn tool_selected(call: &ToolCallRecord, tools: Option<&[String]>) -> bool {
    tools
        .map(|tools| tools.iter().any(|tool| tool == &call.name))
        .unwrap_or_else(|| !tool_path_inputs(call).is_empty())
}

fn tool_path_inputs(call: &ToolCallRecord) -> Vec<(&'static str, String)> {
    tool_path_fields(&call.name)
        .iter()
        .filter_map(|field| {
            call.input
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| (*field, value.to_string()))
        })
        .collect()
}

fn tool_path_fields(tool: &str) -> &'static [&'static str] {
    match tool {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookRead" | "NotebookEdit" => &["file_path"],
        "Glob" | "Grep" | "LS" => &["path"],
        "fs/read_text_file" | "fs/write_text_file" => &["path"],
        "terminal/create" => &["cwd"],
        _ => &[],
    }
}

fn resolve_allowed_root(sandbox: &Path, raw_path: &str) -> PathBuf {
    let expanded = expand_home(raw_path);
    let candidate = path_util::candidate_path(sandbox, Path::new(&expanded));
    path_util::canonicalize_existing(&candidate)
        .unwrap_or_else(|_| path_util::normalize_path_lexical(&candidate))
}

fn resolve_trace_path(sandbox: &Path, tool: &str, raw_path: &str) -> anyhow::Result<PathBuf> {
    let expanded = expand_home(raw_path);
    let path = Path::new(&expanded);
    match tool {
        "fs/write_text_file" => {
            path_util::resolve_write_target_inside(sandbox, path).or_else(|err| {
                if err.to_string().contains("escapes sandbox") {
                    Err(err)
                } else {
                    Ok(path_util::normalize_path_lexical(
                        &path_util::candidate_path(sandbox, path),
                    ))
                }
            })
        }
        "fs/read_text_file" => path_util::resolve_existing_inside(sandbox, path).or_else(|err| {
            if err.to_string().contains("escapes sandbox") {
                Err(err)
            } else {
                Ok(path_util::normalize_path_lexical(
                    &path_util::candidate_path(sandbox, path),
                ))
            }
        }),
        _ => {
            let candidate = path_util::candidate_path(sandbox, path);
            Ok(path_util::canonicalize_existing(&candidate)
                .unwrap_or_else(|_| path_util::normalize_path_lexical(&candidate)))
        }
    }
}

fn expand_home(raw_path: &str) -> String {
    if raw_path == "~" || raw_path.starts_with("~/") || raw_path.starts_with("~\\") {
        if let Some(home) = home_dir_string() {
            let rest = raw_path.strip_prefix('~').expect("path starts with tilde");
            return format!("{home}{rest}");
        }
    }
    raw_path.to_string()
}

fn home_dir_string() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|value| value.to_string_lossy().to_string())
}

fn base_result(id: &str, kind: &str, pass: bool, weight: f64, detail: String) -> AssertionResult {
    AssertionResult {
        id: id.to_string(),
        kind: kind.to_string(),
        pass,
        detail,
        weight,
        score: None,
        min_score: None,
        rationale: None,
        captures: Vec::new(),
    }
}
