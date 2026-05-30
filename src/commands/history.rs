use serde::Serialize;

use crate::commands::trace_files::{load_v2_traces, LoadedTrace};
use crate::ui::{self, Tone};

#[derive(Debug, Clone)]
pub struct HistoryOptions {
    pub skill: Option<String>,
    pub scenario: Option<String>,
    pub last: usize,
    pub json: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    run_id: String,
    file_path: String,
    skill: String,
    scenario: String,
    finished_at: String,
    overall_pass: bool,
    duration_ms: u64,
    turns_used: u32,
    tool_calls_total: usize,
    tokens_total: u64,
    usd_estimate: f64,
    token_budget: Option<f64>,
    budget_exceeded: bool,
    error_count: usize,
}

pub fn history_command(opts: HistoryOptions) -> anyhow::Result<i32> {
    let (runs_exists, traces) = load_v2_traces()?;
    if !runs_exists {
        println!("{}", ui::header("ai-tester", "history"));
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("No runs/ directory found", Tone::Muted)
        );
        return Ok(0);
    }

    let mut entries = Vec::new();
    for trace in traces {
        if opts
            .skill
            .as_ref()
            .is_some_and(|skill| *skill != trace.record.skill.name)
        {
            continue;
        }
        if opts
            .scenario
            .as_ref()
            .is_some_and(|scenario| *scenario != trace.record.scenario.name)
        {
            continue;
        }
        entries.push(to_entry(trace));
    }

    entries.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
    let limit = if opts.last == 0 { 20 } else { opts.last };
    let shown = entries.into_iter().take(limit).collect::<Vec<_>>();

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(0);
    }

    if shown.is_empty() {
        println!("{}", ui::header("ai-tester", "history"));
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("No runs matched", Tone::Muted)
        );
        return Ok(0);
    }

    println!("{}", ui::header("ai-tester", "history"));
    println!("  {}", ui::kv("showing", shown.len()));
    println!();
    for entry in &shown {
        let mark = ui::paint(
            "●",
            if entry.overall_pass {
                Tone::Success
            } else {
                Tone::Error
            },
        );
        println!(
            "  {mark} {}  {}  {}  {}  {}  {}",
            entry.finished_at,
            ui::paint(&format!("{}/{}", entry.skill, entry.scenario), Tone::Strong),
            ui::paint(&format!("{}t", entry.turns_used), Tone::Muted),
            ui::paint(&format!("{} calls", entry.tool_calls_total), Tone::Muted),
            ui::paint(&format!("{} tok", entry.tokens_total), Tone::Muted),
            ui::paint(&format!("~${:.4}", entry.usd_estimate), Tone::Muted)
        );
        println!("    {}", ui::kv("run_id", &entry.run_id));
    }
    Ok(0)
}

fn to_entry(trace: LoadedTrace) -> HistoryEntry {
    let record = trace.record;
    let tokens_total = record.cost.total_tokens();
    let token_budget = record.scenario.token_budget.or(record.skill.token_budget);
    HistoryEntry {
        run_id: record.run_id,
        file_path: trace.path.display().to_string(),
        skill: record.skill.name,
        scenario: record.scenario.name,
        finished_at: record
            .runner
            .finished_at
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        overall_pass: record.scoring.overall_pass,
        duration_ms: record.runner.duration_ms,
        turns_used: record.runner.turns_used,
        tool_calls_total: record.tool_call_summary.total,
        tokens_total,
        usd_estimate: record.cost.usd_estimate,
        token_budget,
        budget_exceeded: token_budget.is_some_and(|budget| tokens_total as f64 > budget),
        error_count: record.errors.len(),
    }
}
