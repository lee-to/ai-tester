use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::assertions::AssertionResult;
use crate::commands::trace_files::{load_v2_trace_file, load_v2_traces, LoadedTrace};
use crate::trace::TraceRecord;
use crate::ui::{self, Tone};

#[derive(Debug, Clone)]
pub struct TrendOptions {
    pub skill: String,
    pub scenario: Option<String>,
    pub last: usize,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct CompareOptions {
    pub run_a: String,
    pub run_b: String,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct TraceOptions {
    pub run_id: String,
    pub json: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    run_id: String,
    file_path: String,
    skill: String,
    scenario: String,
    finished_at: String,
    overall_pass: bool,
    weighted_score: Option<f64>,
    duration_ms: u64,
    turns_used: u32,
    max_turns: u32,
    tool_calls_total: usize,
    tokens_total: u64,
    usd_estimate: f64,
    error_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrendOutput {
    skill: String,
    scenario: Option<String>,
    count: usize,
    runs: Vec<RunSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompareOutput {
    run_a: RunSummary,
    run_b: RunSummary,
    score_delta: Option<f64>,
    duration_delta_ms: i64,
    turns_delta: i64,
    tokens_delta: i64,
    usd_delta: f64,
    assertion_changes: Vec<AssertionChange>,
    tool_call_deltas: Vec<ToolCallDelta>,
    errors: ErrorDelta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssertionChange {
    id: String,
    kind_a: Option<String>,
    kind_b: Option<String>,
    pass_a: Option<bool>,
    pass_b: Option<bool>,
    detail_a: Option<String>,
    detail_b: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallDelta {
    tool: String,
    count_a: usize,
    count_b: usize,
    delta: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorDelta {
    count_a: usize,
    count_b: usize,
    messages_a: Vec<String>,
    messages_b: Vec<String>,
}

enum TraceLookup {
    Found(Box<LoadedTrace>),
    Missing(String),
    Ambiguous {
        query: String,
        matches: Vec<RunSummary>,
    },
}

pub fn trend_command(opts: TrendOptions) -> anyhow::Result<i32> {
    let (runs_exists, mut traces) = load_v2_traces()?;
    if !runs_exists {
        print_no_runs("trend");
        return Ok(0);
    }

    traces.retain(|trace| {
        trace.record.skill.name == opts.skill
            && opts
                .scenario
                .as_ref()
                .map(|scenario| trace.record.scenario.name == *scenario)
                .unwrap_or(true)
    });
    traces.sort_by(|a, b| {
        a.record
            .runner
            .finished_at
            .cmp(&b.record.runner.finished_at)
    });

    let limit = normalized_limit(opts.last);
    let start = traces.len().saturating_sub(limit);
    let shown = traces.into_iter().skip(start).collect::<Vec<_>>();

    if opts.json {
        let output = TrendOutput {
            skill: opts.skill,
            scenario: opts.scenario,
            count: shown.len(),
            runs: shown.iter().map(summary_for).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(0);
    }

    println!("{}", ui::header("ai-tester", "trend"));
    if shown.is_empty() {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("No runs matched", Tone::Muted)
        );
        return Ok(0);
    }

    println!(
        "  {}",
        ui::kv("skill", ui::paint(&opts.skill, Tone::Strong))
    );
    if let Some(scenario) = &opts.scenario {
        println!("  {}", ui::kv("scenario", scenario));
    }
    println!("  {}", ui::kv("showing", shown.len()));
    println!();

    for trace in &shown {
        let record = &trace.record;
        let pass = record.scoring.overall_pass;
        let mark = ui::paint("●", if pass { Tone::Success } else { Tone::Error });
        println!(
            "  {mark} {}  {}  {}  {}  {}  {}",
            display_time(record),
            ui::status(if pass { "PASS" } else { "FAIL" }, pass),
            ui::paint(&score_label(record.scoring.weighted_score), Tone::Strong),
            ui::paint(&format_duration(record.runner.duration_ms), Tone::Muted),
            ui::paint(&format!("{} tok", record.cost.total_tokens()), Tone::Muted),
            ui::paint(&format!("~${:.4}", record.cost.usd_estimate), Tone::Muted)
        );
        println!("    {}", ui::kv("run_id", &record.run_id));
        println!(
            "    {}",
            ui::kv(
                "scenario",
                format!("{}/{}", record.skill.name, record.scenario.name)
            )
        );
    }
    Ok(0)
}

pub fn compare_command(opts: CompareOptions) -> anyhow::Result<i32> {
    let (runs_exists, traces) = load_v2_traces()?;
    if !runs_exists && !Path::new(&opts.run_a).is_file() && !Path::new(&opts.run_b).is_file() {
        print_no_runs("compare");
        return Ok(2);
    }

    let run_a = match find_trace(&opts.run_a, &traces)? {
        TraceLookup::Found(trace) => *trace,
        issue => {
            print_lookup_issue("compare", issue);
            return Ok(2);
        }
    };
    let run_b = match find_trace(&opts.run_b, &traces)? {
        TraceLookup::Found(trace) => *trace,
        issue => {
            print_lookup_issue("compare", issue);
            return Ok(2);
        }
    };

    let output = compare_output(&run_a, &run_b);
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(0);
    }

    print_compare(&run_a, &run_b, &output);
    Ok(0)
}

pub fn trace_command(opts: TraceOptions) -> anyhow::Result<i32> {
    let (runs_exists, traces) = load_v2_traces()?;
    if !runs_exists && !Path::new(&opts.run_id).is_file() {
        print_no_runs("trace");
        return Ok(2);
    }

    let trace = match find_trace(&opts.run_id, &traces)? {
        TraceLookup::Found(trace) => *trace,
        issue => {
            print_lookup_issue("trace", issue);
            return Ok(2);
        }
    };

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&trace.record)?);
        return Ok(0);
    }

    print_trace(&trace);
    Ok(0)
}

fn find_trace(query: &str, traces: &[LoadedTrace]) -> anyhow::Result<TraceLookup> {
    let query_path = Path::new(query);
    if query_path.is_file() {
        return Ok(match load_v2_trace_file(query_path)? {
            Some(trace) => TraceLookup::Found(Box::new(trace)),
            None => {
                TraceLookup::Missing(format!("Trace path `{query}` is not a readable v2 trace"))
            }
        });
    }

    let mut matches = traces
        .iter()
        .filter(|trace| {
            trace.record.run_id == query
                || trace
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem == query)
        })
        .cloned()
        .collect::<Vec<_>>();

    match matches.len() {
        0 => Ok(TraceLookup::Missing(format!("No trace matched `{query}`"))),
        1 => Ok(TraceLookup::Found(Box::new(matches.remove(0)))),
        _ => Ok(TraceLookup::Ambiguous {
            query: query.to_string(),
            matches: matches.iter().map(summary_for).collect(),
        }),
    }
}

fn summary_for(trace: &LoadedTrace) -> RunSummary {
    let record = &trace.record;
    RunSummary {
        run_id: record.run_id.clone(),
        file_path: trace.path.display().to_string(),
        skill: record.skill.name.clone(),
        scenario: record.scenario.name.clone(),
        finished_at: record.runner.finished_at.to_rfc3339(),
        overall_pass: record.scoring.overall_pass,
        weighted_score: record.scoring.weighted_score,
        duration_ms: record.runner.duration_ms,
        turns_used: record.runner.turns_used,
        max_turns: record.runner.max_turns,
        tool_calls_total: record.tool_call_summary.total,
        tokens_total: record.cost.total_tokens(),
        usd_estimate: record.cost.usd_estimate,
        error_count: record.errors.len(),
    }
}

fn compare_output(run_a: &LoadedTrace, run_b: &LoadedTrace) -> CompareOutput {
    let summary_a = summary_for(run_a);
    let summary_b = summary_for(run_b);
    CompareOutput {
        score_delta: match (summary_a.weighted_score, summary_b.weighted_score) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        },
        duration_delta_ms: summary_b.duration_ms as i64 - summary_a.duration_ms as i64,
        turns_delta: summary_b.turns_used as i64 - summary_a.turns_used as i64,
        tokens_delta: summary_b.tokens_total as i64 - summary_a.tokens_total as i64,
        usd_delta: summary_b.usd_estimate - summary_a.usd_estimate,
        assertion_changes: assertion_changes(&run_a.record.assertions, &run_b.record.assertions),
        tool_call_deltas: tool_call_deltas(
            &run_a.record.tool_call_summary.by_tool,
            &run_b.record.tool_call_summary.by_tool,
        ),
        errors: ErrorDelta {
            count_a: run_a.record.errors.len(),
            count_b: run_b.record.errors.len(),
            messages_a: run_a
                .record
                .errors
                .iter()
                .map(|err| format!("{}: {}", err.kind, err.message))
                .collect(),
            messages_b: run_b
                .record
                .errors
                .iter()
                .map(|err| format!("{}: {}", err.kind, err.message))
                .collect(),
        },
        run_a: summary_a,
        run_b: summary_b,
    }
}

fn assertion_changes(
    assertions_a: &[AssertionResult],
    assertions_b: &[AssertionResult],
) -> Vec<AssertionChange> {
    let map_a = assertions_a
        .iter()
        .map(|assertion| (assertion.id.clone(), assertion))
        .collect::<BTreeMap<_, _>>();
    let map_b = assertions_b
        .iter()
        .map(|assertion| (assertion.id.clone(), assertion))
        .collect::<BTreeMap<_, _>>();
    let ids = map_a
        .keys()
        .chain(map_b.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    ids.into_iter()
        .filter_map(|id| {
            let left = map_a.get(&id).copied();
            let right = map_b.get(&id).copied();
            if assertion_same(left, right) {
                return None;
            }
            Some(AssertionChange {
                id,
                kind_a: left.map(|assertion| assertion.kind.clone()),
                kind_b: right.map(|assertion| assertion.kind.clone()),
                pass_a: left.map(|assertion| assertion.pass),
                pass_b: right.map(|assertion| assertion.pass),
                detail_a: left.map(|assertion| assertion.detail.clone()),
                detail_b: right.map(|assertion| assertion.detail.clone()),
            })
        })
        .collect()
}

fn assertion_same(left: Option<&AssertionResult>, right: Option<&AssertionResult>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.kind == right.kind
                && left.pass == right.pass
                && left.detail == right.detail
                && left.score == right.score
                && left.min_score == right.min_score
        }
        (None, None) => true,
        _ => false,
    }
}

fn tool_call_deltas(
    calls_a: &BTreeMap<String, usize>,
    calls_b: &BTreeMap<String, usize>,
) -> Vec<ToolCallDelta> {
    let tools = calls_a
        .keys()
        .chain(calls_b.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    tools
        .into_iter()
        .filter_map(|tool| {
            let count_a = *calls_a.get(&tool).unwrap_or(&0);
            let count_b = *calls_b.get(&tool).unwrap_or(&0);
            if count_a == count_b {
                return None;
            }
            Some(ToolCallDelta {
                tool,
                count_a,
                count_b,
                delta: count_b as i64 - count_a as i64,
            })
        })
        .collect()
}

fn print_trace(trace: &LoadedTrace) {
    let record = &trace.record;
    let pass = record.scoring.overall_pass;
    println!("{}", ui::header("ai-tester", "trace"));
    println!("  {}", ui::kv("run_id", &record.run_id));
    println!(
        "  {}",
        ui::kv(
            "scenario",
            format!("{}/{}", record.skill.name, record.scenario.name)
        )
    );
    println!(
        "  {}",
        ui::kv("file", ui::fit_value(trace.path.display(), 15))
    );
    println!(
        "  {}",
        ui::kv(
            "status",
            ui::status(if pass { "PASS" } else { "FAIL" }, pass)
        )
    );
    println!(
        "  {}",
        ui::kv("score", score_label(record.scoring.weighted_score))
    );
    println!(
        "  {}",
        ui::kv(
            "runner",
            format!(
                "{} {} {}",
                record.runner.model, record.runner.permission_mode, record.runner.max_turns
            )
        )
    );
    println!("  {}", ui::kv("finished", display_time(record)));
    println!(
        "  {}",
        ui::kv("duration", format_duration(record.runner.duration_ms))
    );
    println!("  {}", ui::kv("tokens", record.cost.total_tokens()));
    println!(
        "  {}",
        ui::kv("cost", format!("~${:.4}", record.cost.usd_estimate))
    );

    if !record.assertions.is_empty() {
        println!("  {}", ui::section("Assertions"));
        for assertion in &record.assertions {
            println!(
                "    {} {} {}",
                ui::paint(
                    "●",
                    if assertion.pass {
                        Tone::Success
                    } else {
                        Tone::Error
                    }
                ),
                ui::paint(&assertion.id, Tone::Strong),
                ui::paint(&assertion.detail, Tone::Muted)
            );
        }
    }

    println!("  {}", ui::section("Tool calls"));
    if record.tool_call_summary.by_tool.is_empty() {
        println!("    {}", ui::paint("none", Tone::Muted));
    } else {
        for (tool, count) in &record.tool_call_summary.by_tool {
            println!("    {} {}", ui::paint(tool, Tone::Strong), count);
        }
    }

    println!("  {}", ui::section("Turns"));
    if record.turns.is_empty() {
        println!("    {}", ui::paint("none", Tone::Muted));
    } else {
        for turn in &record.turns {
            let calls = turn
                .tool_calls
                .iter()
                .map(|call| format!("{}({})", call.name, preview_json(&call.input, 80)))
                .collect::<Vec<_>>();
            let text = preview_text(&turn.text_deltas.join(""), 120);
            println!(
                "    #{} {}  {}  {}",
                turn.index,
                ui::paint(&turn.role, Tone::Strong),
                ui::paint(&format!("{} tool(s)", turn.tool_calls.len()), Tone::Muted),
                ui::paint(&text, Tone::Muted)
            );
            for call in calls {
                println!("      {}", call);
            }
        }
    }

    if !record.final_output.trim().is_empty() {
        println!("  {}", ui::section("Final output"));
        println!("    {}", preview_text(&record.final_output, 500));
    }

    if !record.errors.is_empty() {
        println!("  {}", ui::section("Errors"));
        for error in &record.errors {
            println!(
                "    {}: {}",
                ui::paint(&error.kind, Tone::Error),
                error.message
            );
        }
    }
}

fn print_compare(run_a: &LoadedTrace, run_b: &LoadedTrace, output: &CompareOutput) {
    println!("{}", ui::header("ai-tester", "compare"));
    println!("  {}", ui::kv("run A", val_a(&run_a.record.run_id)));
    println!("  {}", ui::kv("run B", val_b(&run_b.record.run_id)));
    println!(
        "  {}",
        ui::kv(
            "scenario",
            format!(
                "{}/{} -> {}/{}",
                run_a.record.skill.name,
                run_a.record.scenario.name,
                run_b.record.skill.name,
                run_b.record.scenario.name
            )
        )
    );

    println!("  {}", ui::section("Summary"));
    println!(
        "    {}",
        ui::kv(
            "status",
            format!(
                "{} -> {}",
                val_a(status_word(run_a.record.scoring.overall_pass)),
                val_b(status_word(run_b.record.scoring.overall_pass))
            )
        )
    );
    println!(
        "    {}",
        ui::kv(
            "score",
            format!(
                "{} -> {} ({})",
                val_a(score_label(run_a.record.scoring.weighted_score)),
                val_b(score_label(run_b.record.scoring.weighted_score)),
                val_delta(
                    score_delta_label(output.score_delta),
                    output.score_delta.unwrap_or(0.0)
                )
            )
        )
    );
    println!(
        "    {}",
        ui::kv(
            "duration",
            format!(
                "{} -> {} ({})",
                val_a(format_duration(run_a.record.runner.duration_ms)),
                val_b(format_duration(run_b.record.runner.duration_ms)),
                val_delta(
                    signed_duration(output.duration_delta_ms),
                    output.duration_delta_ms as f64
                )
            )
        )
    );
    println!(
        "    {}",
        ui::kv(
            "turns",
            format!(
                "{} -> {} ({})",
                val_a(run_a.record.runner.turns_used),
                val_b(run_b.record.runner.turns_used),
                val_delta(
                    format!("{:+}", output.turns_delta),
                    output.turns_delta as f64
                )
            )
        )
    );
    println!(
        "    {}",
        ui::kv(
            "tokens",
            format!(
                "{} -> {} ({})",
                val_a(run_a.record.cost.total_tokens()),
                val_b(run_b.record.cost.total_tokens()),
                val_delta(
                    format!("{:+}", output.tokens_delta),
                    output.tokens_delta as f64
                )
            )
        )
    );
    println!(
        "    {}",
        ui::kv(
            "errors",
            format!(
                "{} -> {}",
                val_a(output.errors.count_a),
                val_b(output.errors.count_b)
            )
        )
    );

    println!("  {}", ui::section("Assertions"));
    if output.assertion_changes.is_empty() {
        println!("    {}", ui::paint("no assertion changes", Tone::Muted));
    } else {
        for change in &output.assertion_changes {
            println!(
                "    {} {} -> {}  {}",
                ui::paint(&change.id, Tone::Strong),
                optional_status(change.pass_a),
                optional_status(change.pass_b),
                ui::paint(
                    change.detail_b.as_deref().unwrap_or("missing in run B"),
                    Tone::Muted
                )
            );
        }
    }

    println!("  {}", ui::section("Tool calls"));
    if output.tool_call_deltas.is_empty() {
        println!("    {}", ui::paint("no tool-call deltas", Tone::Muted));
    } else {
        for delta in &output.tool_call_deltas {
            println!(
                "    {} {} -> {} ({:+})",
                ui::paint(&delta.tool, Tone::Strong),
                delta.count_a,
                delta.count_b,
                delta.delta
            );
        }
    }

    if !output.errors.messages_a.is_empty() || !output.errors.messages_b.is_empty() {
        println!("  {}", ui::section("Errors"));
        for message in &output.errors.messages_a {
            println!("    A {}", ui::paint(message, Tone::Muted));
        }
        for message in &output.errors.messages_b {
            println!("    B {}", ui::paint(message, Tone::Muted));
        }
    }
}

fn print_no_runs(command: &str) {
    println!("{}", ui::header("ai-tester", command));
    println!(
        "  {} {}",
        ui::paint("●", Tone::Warning),
        ui::paint("No runs/ directory found", Tone::Muted)
    );
}

fn print_lookup_issue(command: &str, issue: TraceLookup) {
    println!("{}", ui::header("ai-tester", command));
    match issue {
        TraceLookup::Missing(message) => {
            println!(
                "  {} {}",
                ui::paint("●", Tone::Error),
                ui::paint("trace not found", Tone::Strong)
            );
            println!("  {}", ui::kv("reason", message));
        }
        TraceLookup::Ambiguous { query, matches } => {
            println!(
                "  {} {}",
                ui::paint("●", Tone::Error),
                ui::paint("ambiguous trace id", Tone::Strong)
            );
            println!("  {}", ui::kv("query", query));
            for candidate in matches {
                println!(
                    "    {} {}",
                    ui::paint(&candidate.run_id, Tone::Strong),
                    ui::paint(&candidate.file_path, Tone::Muted)
                );
            }
        }
        TraceLookup::Found(_) => {}
    }
}

fn normalized_limit(last: usize) -> usize {
    if last == 0 {
        20
    } else {
        last
    }
}

fn display_time(record: &TraceRecord) -> String {
    record
        .runner
        .finished_at
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn status_word(pass: bool) -> &'static str {
    if pass {
        "PASS"
    } else {
        "FAIL"
    }
}

/// Left-hand (run A) value, painted blue.
fn val_a(value: impl std::fmt::Display) -> String {
    ui::paint(&value.to_string(), Tone::Info)
}

/// Right-hand (run B) value, painted yellow.
fn val_b(value: impl std::fmt::Display) -> String {
    ui::paint(&value.to_string(), Tone::Warning)
}

/// Delta value: green when positive, red when negative, muted when zero.
fn val_delta(value: impl std::fmt::Display, sign: f64) -> String {
    let tone = if sign > 0.0 {
        Tone::Success
    } else if sign < 0.0 {
        Tone::Error
    } else {
        Tone::Muted
    };
    ui::paint(&value.to_string(), tone)
}

fn score_label(score: Option<f64>) -> String {
    match score {
        Some(score) => format!("{:.0}%", score * 100.0),
        None => "n/a".to_string(),
    }
}

fn score_delta_label(delta: Option<f64>) -> String {
    match delta {
        Some(delta) => format!("{:+.0}pp", delta * 100.0),
        None => "n/a".to_string(),
    }
}

fn optional_status(pass: Option<bool>) -> &'static str {
    match pass {
        Some(true) => "PASS",
        Some(false) => "FAIL",
        None => "missing",
    }
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}

fn signed_duration(delta_ms: i64) -> String {
    if delta_ms.unsigned_abs() < 1_000 {
        format!("{delta_ms:+}ms")
    } else {
        format!("{:+.1}s", delta_ms as f64 / 1_000.0)
    }
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, max_chars)
}

fn preview_json(value: &serde_json::Value, max_chars: usize) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_string());
    truncate_chars(&raw, max_chars)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return "...".chars().take(max_chars).collect();
    }
    let keep = max_chars - 3;
    let head = keep / 2;
    let tail = keep - head;
    let start = value.chars().take(head).collect::<String>();
    let end = value
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{start}...{end}")
}
