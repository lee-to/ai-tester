use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scenario::AssertionSpec;
use crate::trace::{ToolCallRecord, TraceRecord};
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
            args_match,
            call_index,
            ..
        } => evaluate_tool_called(id, *weight, tool, args_match.as_ref(), *call_index, trace),
        AssertionSpec::ToolCallSequence {
            id,
            weight,
            sequence,
            ..
        } => {
            let mut next_index = 0usize;
            let calls = all_tool_calls(trace);
            for step in sequence {
                let found = calls[next_index..].iter().position(|call| {
                    call.name == step.tool
                        && args_match(step.args_match.as_ref(), &call.input).unwrap_or(false)
                });
                let Some(offset) = found else {
                    return base_result(
                        id,
                        "tool_call_sequence",
                        false,
                        *weight,
                        format!("missing sequence step for tool `{}`", step.tool),
                    );
                };
                next_index += offset + 1;
            }
            base_result(
                id,
                "tool_call_sequence",
                true,
                *weight,
                "sequence matched".to_string(),
            )
        }
        AssertionSpec::NoToolCalled {
            id,
            weight,
            tool,
            tool_pattern,
            args_match: expected_args,
        } => {
            let pattern = tool_pattern.as_ref().and_then(|p| compile_pattern(p).ok());
            let matched = all_tool_calls(trace).into_iter().find(|call| {
                let tool_ok = tool.as_ref().is_some_and(|t| call.name == *t)
                    || pattern.as_ref().is_some_and(|re| re.is_match(&call.name));
                tool_ok && args_match(expected_args.as_ref(), &call.input).unwrap_or(false)
            });
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
        AssertionSpec::NoPathEscape { id, weight, .. } => base_result(
            id,
            "no_path_escape",
            true,
            *weight,
            "path escape checks require a real sandbox path".to_string(),
        ),
    }
}

fn evaluate_tool_called(
    id: &str,
    weight: f64,
    tool: &str,
    expected_args: Option<&BTreeMap<String, String>>,
    call_index: Option<usize>,
    trace: &TraceRecord,
) -> AssertionResult {
    let matches = all_tool_calls(trace)
        .into_iter()
        .filter(|call| call.name == tool && args_match(expected_args, &call.input).unwrap_or(false))
        .collect::<Vec<_>>();
    let pass = call_index
        .map(|idx| matches.get(idx).is_some())
        .unwrap_or(!matches.is_empty());
    base_result(
        id,
        "tool_called",
        pass,
        weight,
        if pass {
            format!("found `{tool}` call")
        } else {
            format!("no `{tool}` call matched")
        },
    )
}

fn evaluate_no_unanswered_questions(trace: &TraceRecord) -> AssertionResult {
    let unanswered = trace.tool_call_summary.unanswered_questions;
    base_result(
        "no_unanswered_questions",
        "no_unanswered_questions",
        unanswered == 0,
        1.0,
        if unanswered == 0 {
            "all AskUserQuestion calls had matching user_responses entries".to_string()
        } else {
            format!("{unanswered} AskUserQuestion call(s) had no matching user_responses")
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

fn args_match(expected: Option<&BTreeMap<String, String>>, input: &Value) -> anyhow::Result<bool> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    for (field, pattern) in expected {
        let actual = input.get(field).map(value_to_string).unwrap_or_default();
        let re = compile_pattern(pattern)?;
        if !re.is_match(&actual) {
            return Ok(false);
        }
    }
    Ok(true)
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
