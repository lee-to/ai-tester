use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assertions::AssertionResult;
use crate::skill::allowed_tools::ParsedTool;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub result_content: Option<String>,
    pub result_is_error: bool,
    pub answered: Option<AnsweredQuestion>,
}

impl ToolCallRecord {
    pub fn new(id: &str, name: &str, input: Value) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            input,
            result_content: None,
            result_is_error: false,
            answered: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredQuestion {
    pub matched_entry_index: i32,
    pub chosen_label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub index: usize,
    pub role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_deltas: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
    pub usage: Option<TurnUsage>,
}

impl Turn {
    pub fn assistant_with_tool(id: &str, tool: &str, input: Value) -> Self {
        Self {
            index: 0,
            role: "assistant".to_string(),
            text_deltas: Vec::new(),
            tool_calls: vec![ToolCallRecord::new(id, tool, input)],
            usage: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRecord {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub run_id: String,
    pub skill: TraceSkill,
    pub scenario: TraceScenario,
    pub runner: TraceRunner,
    pub turns: Vec<Turn>,
    pub final_output: String,
    pub tool_call_summary: ToolCallSummary,
    pub assertions: Vec<AssertionResult>,
    pub scoring: TraceScoring,
    pub cost: TraceCost,
    pub errors: Vec<TraceError>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl TraceRecord {
    pub fn synthetic(
        mut turns: Vec<Turn>,
        final_output: String,
        turns_used: u32,
        token_budget: Option<f64>,
    ) -> Self {
        for (idx, turn) in turns.iter_mut().enumerate() {
            turn.index = idx;
        }
        let summary = ToolCallSummary::from_turns(&turns, 0);
        Self {
            schema_version: "2.0.0".to_string(),
            run_id: "synthetic".to_string(),
            skill: TraceSkill {
                name: "synthetic".to_string(),
                path: String::new(),
                version: None,
                source_hash: "0".repeat(64),
                source_hash_short: "00000000".to_string(),
                body_hash: "0".repeat(64),
                allowed_tools_parsed: Vec::new(),
                allowed_tools_raw: Vec::new(),
                token_budget: None,
            },
            scenario: TraceScenario {
                name: "synthetic".to_string(),
                path: String::new(),
                argument: None,
                token_budget,
            },
            runner: TraceRunner {
                runtime: "claude".to_string(),
                model: crate::config::DEFAULT_MODEL.to_string(),
                mode: None,
                reasoning: None,
                permission_mode: "bypassPermissions".to_string(),
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 0,
                max_turns: crate::config::INTERNAL_MAX_TURNS,
                max_turns_user_set: false,
                turns_used,
                hit_max_turns: false,
                stopped_reason: "end_turn".to_string(),
                session_id: None,
                sandbox_path: None,
            },
            turns,
            final_output,
            tool_call_summary: summary,
            assertions: Vec::new(),
            scoring: TraceScoring {
                all_passed: true,
                overall_pass: true,
                weighted_score: None,
                pass_threshold: None,
            },
            cost: TraceCost {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                usd_estimate: 0.0,
                source: "unknown".to_string(),
            },
            errors: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSkill {
    pub name: String,
    pub path: String,
    pub version: Option<String>,
    pub source_hash: String,
    pub source_hash_short: String,
    pub body_hash: String,
    pub allowed_tools_parsed: Vec<ParsedTool>,
    pub allowed_tools_raw: Vec<String>,
    pub token_budget: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceScenario {
    pub name: String,
    pub path: String,
    pub argument: Option<String>,
    pub token_budget: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRunner {
    #[serde(default)]
    pub runtime: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub permission_mode: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub max_turns: u32,
    pub max_turns_user_set: bool,
    pub turns_used: u32,
    pub hit_max_turns: bool,
    #[serde(default)]
    pub stopped_reason: String,
    pub session_id: Option<String>,
    pub sandbox_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallSummary {
    pub total: usize,
    #[serde(rename = "byTool")]
    pub by_tool: BTreeMap<String, usize>,
    pub unanswered_questions: usize,
}

impl ToolCallSummary {
    pub fn from_turns(turns: &[Turn], unanswered_questions: usize) -> Self {
        let mut by_tool = BTreeMap::new();
        let mut total = 0;
        for call in turns.iter().flat_map(|turn| &turn.tool_calls) {
            total += 1;
            *by_tool.entry(call.name.clone()).or_insert(0) += 1;
        }
        Self {
            total,
            by_tool,
            unanswered_questions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceScoring {
    pub all_passed: bool,
    pub overall_pass: bool,
    pub weighted_score: Option<f64>,
    pub pass_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub usd_estimate: f64,
    pub source: String,
}

impl TraceCost {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceError {
    pub kind: String,
    pub message: String,
}

pub fn write_trace(runs_dir: impl AsRef<Path>, record: &TraceRecord) -> anyhow::Result<PathBuf> {
    let runs_dir = runs_dir
        .as_ref()
        .join(sanitize_path_segment(&record.skill.name));
    fs::create_dir_all(&runs_dir)?;
    let path = runs_dir.join(format!("{}.json", sanitize_path_segment(&record.run_id)));
    fs::write(&path, serde_json::to_string_pretty(record)? + "\n")?;
    Ok(path)
}

pub fn sanitize_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}
