use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::commands::run::{collect_run_records_with_output, OutputFormat, RunOptions};
use crate::trace::TraceRecord;
use crate::ui::{self, Tone};

#[derive(Debug, Clone, Default)]
pub struct BenchmarkOptions {
    pub suite: PathBuf,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub reasoning: Option<String>,
    pub runtime: Option<String>,
    pub agent: Option<String>,
    pub mcp_profile: Option<String>,
    pub acp_log: Option<PathBuf>,
    pub filter: Option<String>,
    pub keep_sandbox: bool,
    pub quiet: bool,
    pub idle_warn_seconds: u64,
    pub setup_timeout_seconds: Option<u64>,
    pub acp_turn_timeout_seconds: Option<u64>,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkSuite {
    suite: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    requirements: BenchmarkRequirements,
    #[serde(default)]
    scoring: BenchmarkScoring,
    scenarios: Vec<BenchmarkScenario>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct BenchmarkRequirements {
    #[serde(default)]
    commands: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkScoring {
    #[serde(default = "default_correctness_weight")]
    correctness_weight: f64,
    #[serde(default = "default_efficiency_weight")]
    efficiency_weight: f64,
}

impl Default for BenchmarkScoring {
    fn default() -> Self {
        Self {
            correctness_weight: default_correctness_weight(),
            efficiency_weight: default_efficiency_weight(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkScenario {
    file: PathBuf,
    #[serde(default = "default_scenario_weight")]
    weight: f64,
    time_budget_ms: Option<u64>,
    token_budget: Option<u64>,
    tool_budget: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    suite: String,
    version: Option<u32>,
    category: Option<String>,
    description: Option<String>,
    score: f64,
    correctness: f64,
    efficiency: f64,
    duration_ms: u64,
    tokens_total: u64,
    tool_calls_total: usize,
    scenarios_total: usize,
    scenarios_passed: usize,
    scenarios_failed: usize,
    scenarios: Vec<BenchmarkScenarioReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkScenarioReport {
    name: String,
    file: String,
    weight: f64,
    result: String,
    score: f64,
    correctness: f64,
    efficiency: f64,
    time_score: f64,
    token_score: f64,
    tool_score: f64,
    duration_ms: u64,
    tokens_total: u64,
    tool_calls_total: usize,
    trace_id: Option<String>,
    cap: Option<f64>,
    failed_assertions: Vec<String>,
    errors: Vec<String>,
}

pub fn benchmark_command(opts: BenchmarkOptions) -> anyhow::Result<i32> {
    let suite_path = resolve_suite_path(&opts.suite)?;
    let suite = load_suite(&suite_path)?;
    validate_suite(&suite)?;

    let missing = missing_requirements(&suite.requirements);
    if !missing.is_empty() {
        match opts.format {
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "suite": suite.suite,
                    "category": suite.category,
                    "skipped": true,
                    "missingRequirements": missing,
                }))?
            ),
            _ => {
                println!("{}", ui::header("ai-tester", "benchmark"));
                println!(
                    "  {} {}",
                    ui::paint("●", Tone::Warning),
                    ui::paint(
                        &format!("SKIP {}: missing {}", suite.suite, missing.join(", ")),
                        Tone::Warning,
                    )
                );
            }
        }
        return Ok(0);
    }

    let report = run_suite(&suite, &suite_path, &opts)?;
    match opts.format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Markdown => println!("{}", render_markdown(&report)),
        OutputFormat::Live => print_live(&report, &opts),
    }

    Ok(if report.scenarios_failed == 0 { 0 } else { 1 })
}

fn load_suite(path: &Path) -> anyhow::Result<BenchmarkSuite> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read benchmark suite {}", path.display()))?;
    yaml_serde::from_str(&raw).with_context(|| format!("parse benchmark suite {}", path.display()))
}

fn validate_suite(suite: &BenchmarkSuite) -> anyhow::Result<()> {
    if suite.suite.trim().is_empty() {
        anyhow::bail!("benchmark suite name must not be empty");
    }
    if suite
        .category
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        anyhow::bail!("benchmark suite category must not be empty when provided");
    }
    if suite.scenarios.is_empty() {
        anyhow::bail!("benchmark suite must include at least one scenario");
    }
    let scoring_total = suite.scoring.correctness_weight + suite.scoring.efficiency_weight;
    if !suite.scoring.correctness_weight.is_finite()
        || !suite.scoring.efficiency_weight.is_finite()
        || suite.scoring.correctness_weight < 0.0
        || suite.scoring.efficiency_weight < 0.0
        || scoring_total <= f64::EPSILON
    {
        anyhow::bail!(
            "benchmark scoring weights must be finite non-negative values with positive sum"
        );
    }
    for scenario in &suite.scenarios {
        if scenario.file.as_os_str().is_empty() {
            anyhow::bail!("benchmark scenario file must not be empty");
        }
        if !scenario.weight.is_finite() || scenario.weight <= 0.0 {
            anyhow::bail!("benchmark scenario weight must be positive");
        }
    }
    Ok(())
}

fn run_suite(
    suite: &BenchmarkSuite,
    suite_path: &Path,
    opts: &BenchmarkOptions,
) -> anyhow::Result<BenchmarkReport> {
    let base = suite_path.parent().unwrap_or_else(|| Path::new("."));
    let mut scenarios = Vec::new();

    for entry in &suite.scenarios {
        let file = base.join(&entry.file);
        if let Some(filter) = &opts.filter {
            let file_label = entry.file.display().to_string();
            if !file_label.contains(filter) {
                continue;
            }
        }
        if opts.format == OutputFormat::Live && !opts.quiet {
            println!(
                "{} {}",
                ui::paint("▶", Tone::Info),
                ui::paint(
                    &format!("benchmark scenario {}", entry.file.display()),
                    Tone::Strong
                )
            );
        }
        let run_silent = opts.format != OutputFormat::Live || opts.quiet;
        let run = collect_run_records_with_output(
            RunOptions {
                file: Some(file.clone()),
                model: opts.model.clone(),
                mode: opts.mode.clone(),
                reasoning: opts.reasoning.clone(),
                runtime: opts.runtime.clone(),
                agent: opts.agent.clone(),
                mcp_profile: opts.mcp_profile.clone(),
                acp_log: opts.acp_log.clone(),
                keep_sandbox: opts.keep_sandbox,
                quiet: opts.quiet,
                idle_warn_seconds: opts.idle_warn_seconds,
                setup_timeout_seconds: opts.setup_timeout_seconds,
                acp_turn_timeout_seconds: opts.acp_turn_timeout_seconds,
                format: OutputFormat::Json,
                ..Default::default()
            },
            run_silent,
        )?;
        let record = run.records.into_iter().next();
        scenarios.push(score_scenario(
            entry,
            &suite.scoring,
            &file,
            record,
            run.runtime_errors,
        ));
    }

    let total_weight: f64 = scenarios.iter().map(|scenario| scenario.weight).sum();
    let score = weighted_average(&scenarios, total_weight, |scenario| scenario.score);
    let correctness = weighted_average(&scenarios, total_weight, |scenario| scenario.correctness);
    let efficiency = weighted_average(&scenarios, total_weight, |scenario| scenario.efficiency);
    let duration_ms = scenarios
        .iter()
        .map(|scenario| scenario.duration_ms)
        .sum::<u64>();
    let tokens_total = scenarios
        .iter()
        .map(|scenario| scenario.tokens_total)
        .sum::<u64>();
    let tool_calls_total = scenarios
        .iter()
        .map(|scenario| scenario.tool_calls_total)
        .sum::<usize>();
    let passed = scenarios
        .iter()
        .filter(|scenario| scenario.result == "PASS")
        .count();

    Ok(BenchmarkReport {
        suite: suite.suite.clone(),
        version: suite.version,
        category: suite.category.clone(),
        description: suite.description.clone(),
        score,
        correctness,
        efficiency,
        duration_ms,
        tokens_total,
        tool_calls_total,
        scenarios_total: scenarios.len(),
        scenarios_passed: passed,
        scenarios_failed: scenarios.len().saturating_sub(passed),
        scenarios,
    })
}

fn score_scenario(
    entry: &BenchmarkScenario,
    scoring: &BenchmarkScoring,
    file: &Path,
    record: Option<TraceRecord>,
    runtime_errors: usize,
) -> BenchmarkScenarioReport {
    let Some(record) = record else {
        return BenchmarkScenarioReport {
            name: file
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string(),
            file: file.display().to_string(),
            weight: entry.weight,
            result: "ERROR".to_string(),
            score: 0.0,
            correctness: 0.0,
            efficiency: 0.0,
            time_score: 0.0,
            token_score: 0.0,
            tool_score: 0.0,
            duration_ms: 0,
            tokens_total: 0,
            tool_calls_total: 0,
            trace_id: None,
            cap: Some(0.0),
            failed_assertions: Vec::new(),
            errors: vec![format!("{runtime_errors} runtime error(s)")],
        };
    };

    let correctness = record.scoring.weighted_score.unwrap_or(0.0) * 100.0;
    let time_score = budget_score(record.runner.duration_ms as f64, entry.time_budget_ms);
    let tokens_total = record.cost.total_tokens();
    let token_score = budget_score(tokens_total as f64, entry.token_budget);
    let tool_score = budget_score(record.tool_call_summary.total as f64, entry.tool_budget);
    let efficiency = (0.40 * time_score + 0.35 * token_score + 0.25 * tool_score) * 100.0;
    let cap = scenario_cap(&record);
    let scoring_total = scoring.correctness_weight + scoring.efficiency_weight;
    let correctness_weight = scoring.correctness_weight / scoring_total;
    let efficiency_weight = scoring.efficiency_weight / scoring_total;
    let correctness_factor = (correctness / 100.0).powf(2.0);
    let efficiency_factor = (efficiency / 100.0).powf(3.0);
    let raw_score = 100.0
        * correctness_factor
        * ((correctness_weight * 1.0) + (efficiency_weight * efficiency_factor));
    let score = cap.map_or(raw_score, |cap| raw_score.min(cap));
    let failed_assertions = record
        .assertions
        .iter()
        .filter(|assertion| !assertion.pass)
        .map(|assertion| assertion.id.clone())
        .collect::<Vec<_>>();
    let errors = record
        .errors
        .iter()
        .map(|err| format!("{}: {}", err.kind, err.message))
        .collect::<Vec<_>>();
    let result = if !record.errors.is_empty() {
        "ERROR"
    } else if failed_assertions.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };

    BenchmarkScenarioReport {
        name: record.scenario.name.clone(),
        file: file.display().to_string(),
        weight: entry.weight,
        result: result.to_string(),
        score,
        correctness,
        efficiency,
        time_score: time_score * 100.0,
        token_score: token_score * 100.0,
        tool_score: tool_score * 100.0,
        duration_ms: record.runner.duration_ms,
        tokens_total,
        tool_calls_total: record.tool_call_summary.total,
        trace_id: Some(record.run_id.clone()),
        cap,
        failed_assertions,
        errors,
    }
}

fn scenario_cap(record: &TraceRecord) -> Option<f64> {
    if !record.errors.is_empty() {
        return Some(0.0);
    }
    if record
        .assertions
        .iter()
        .any(|assertion| !assertion.pass && assertion.kind == "no_path_escape")
    {
        return Some(0.0);
    }
    if record
        .assertions
        .iter()
        .any(|assertion| !assertion.pass && assertion.kind == "no_tool_called")
    {
        return Some(40.0);
    }
    if !record.scoring.overall_pass || record.assertions.iter().any(|assertion| !assertion.pass) {
        return Some(60.0);
    }
    None
}

fn budget_score<T>(actual: f64, budget: Option<T>) -> f64
where
    T: IntoBudget,
{
    let Some(budget) = budget.map(IntoBudget::into_budget) else {
        return 1.0;
    };
    if budget <= f64::EPSILON || actual <= budget {
        1.0
    } else {
        (budget / actual).clamp(0.0, 1.0)
    }
}

trait IntoBudget {
    fn into_budget(self) -> f64;
}

impl IntoBudget for u64 {
    fn into_budget(self) -> f64 {
        self as f64
    }
}

impl IntoBudget for usize {
    fn into_budget(self) -> f64 {
        self as f64
    }
}

fn weighted_average<F>(scenarios: &[BenchmarkScenarioReport], total_weight: f64, value: F) -> f64
where
    F: Fn(&BenchmarkScenarioReport) -> f64,
{
    if total_weight <= f64::EPSILON {
        return 0.0;
    }
    scenarios
        .iter()
        .map(|scenario| value(scenario) * scenario.weight)
        .sum::<f64>()
        / total_weight
}

fn missing_requirements(requirements: &BenchmarkRequirements) -> Vec<String> {
    requirements
        .commands
        .iter()
        .filter(|command| !requirement_command_succeeds(command))
        .cloned()
        .collect()
}

fn requirement_command_succeeds(command: &str) -> bool {
    shell_command(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", command]);
    cmd
}

fn resolve_suite_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.extension().is_none() {
        for extension in ["yaml", "yml"] {
            let candidate = path.with_extension(extension);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!("benchmark suite file not found: {}", path.display())
}

fn print_live(report: &BenchmarkReport, opts: &BenchmarkOptions) {
    println!("{}", ui::header("ai-tester", "benchmark"));
    println!("  {}", ui::kv("suite", &report.suite));
    if let Some(category) = &report.category {
        println!("  {}", ui::kv("category", category));
    }
    if let Some(runtime) = &opts.runtime {
        println!("  {}", ui::kv("runtime", runtime));
    }
    if let Some(model) = &opts.model {
        println!("  {}", ui::kv("model", model));
    }
    println!("  {}", ui::kv("score", format!("{:.2}/100", report.score)));
    println!(
        "  {}",
        ui::kv("correctness", format!("{:.1}", report.correctness))
    );
    println!(
        "  {}",
        ui::kv("efficiency", format!("{:.1}", report.efficiency))
    );
    println!(
        "  {}",
        ui::kv("duration", format_duration(report.duration_ms))
    );
    println!("  {}", ui::kv("tokens", report.tokens_total));
    println!("  {}", ui::kv("tools", report.tool_calls_total));
    println!();
    println!("  {}", ui::section("Scenarios"));
    for scenario in &report.scenarios {
        let pass = scenario.result == "PASS";
        let tone = if pass { Tone::Success } else { Tone::Error };
        println!(
            "  {} {:<8} {:<28} {:>6.2}  {}  {} tok  {} tools",
            ui::paint("●", tone),
            ui::paint(&scenario.result, tone),
            ui::paint(&scenario.name, Tone::Strong),
            scenario.score,
            format_duration(scenario.duration_ms),
            scenario.tokens_total,
            scenario.tool_calls_total
        );
    }
}

fn render_markdown(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# ai-tester benchmark: {}\n\n", report.suite));
    if let Some(category) = &report.category {
        out.push_str(&format!("**Category:** {category}\n\n"));
    }
    out.push_str(&format!(
        "**Score:** {:.2}/100 · **Correctness:** {:.1} · **Efficiency:** {:.1} · **Duration:** {} · **Tokens:** {} · **Tools:** {}\n\n",
        report.score, report.correctness, report.efficiency
        , format_duration(report.duration_ms), report.tokens_total, report.tool_calls_total
    ));
    out.push_str(
        "| Scenario | Result | Score | Correctness | Efficiency | Duration | Tokens | Tools |\n",
    );
    out.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for scenario in &report.scenarios {
        out.push_str(&format!(
            "| {} | {} | {:.1} | {:.1} | {:.1} | {} | {} | {} |\n",
            scenario.name,
            scenario.result,
            scenario.score,
            scenario.correctness,
            scenario.efficiency,
            format_duration(scenario.duration_ms),
            scenario.tokens_total,
            scenario.tool_calls_total
        ));
    }
    out
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}

fn default_correctness_weight() -> f64 {
    0.7
}

fn default_efficiency_weight() -> f64 {
    0.3
}

fn default_scenario_weight() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertions::AssertionResult;
    use crate::trace::{TraceRecord, Turn};

    fn assertion(id: &str, kind: &str, pass: bool) -> AssertionResult {
        AssertionResult {
            id: id.to_string(),
            kind: kind.to_string(),
            pass,
            detail: format!("{id} detail"),
            weight: 1.0,
            score: None,
            min_score: None,
            rationale: None,
            captures: Vec::new(),
        }
    }

    fn benchmark_entry(file: &str) -> BenchmarkScenario {
        BenchmarkScenario {
            file: PathBuf::from(file),
            weight: 1.0,
            time_budget_ms: Some(1_000),
            token_budget: Some(100),
            tool_budget: Some(5),
        }
    }

    fn scoring() -> BenchmarkScoring {
        BenchmarkScoring::default()
    }

    fn trace(name: &str, assertions: Vec<AssertionResult>) -> TraceRecord {
        let mut record = TraceRecord::synthetic(
            vec![Turn::assistant_with_tool(
                "1",
                "Bash",
                serde_json::json!({"command": "printf ok"}),
            )],
            "done".to_string(),
            1,
            None,
        );
        record.run_id = format!("{name}-run");
        record.scenario.name = name.to_string();
        record.runner.duration_ms = 100;
        record.cost.input_tokens = 10;
        record.cost.output_tokens = 5;
        record.assertions = assertions;
        record.scoring.overall_pass = record.assertions.iter().all(|assertion| assertion.pass);
        record.scoring.all_passed = record.scoring.overall_pass;
        record.scoring.weighted_score = Some(if record.scoring.overall_pass {
            1.0
        } else {
            0.75
        });
        record
    }

    #[test]
    fn score_scenario_is_repeatable_for_same_trace() {
        let entry = benchmark_entry("tasks/01-config-precedence.yaml");
        let record = trace(
            "config-precedence",
            vec![assertion("merged-config", "file_contains", true)],
        );

        let first = score_scenario(
            &entry,
            &scoring(),
            Path::new("benchmarks/js-v1/tasks/01-config-precedence.yaml"),
            Some(record.clone()),
            0,
        );
        let second = score_scenario(
            &entry,
            &scoring(),
            Path::new("benchmarks/js-v1/tasks/01-config-precedence.yaml"),
            Some(record),
            0,
        );

        assert_eq!(
            serde_json::to_value(&first).expect("first serializes"),
            serde_json::to_value(&second).expect("second serializes")
        );
        assert_eq!(first.score, 100.0);
        assert_eq!(first.result, "PASS");
    }

    #[test]
    fn benchmark_report_snapshot_keeps_js_python_order_and_assertion_order() {
        let js = score_scenario(
            &benchmark_entry("tasks/01-config-precedence.yaml"),
            &scoring(),
            Path::new("benchmarks/js-v1/tasks/01-config-precedence.yaml"),
            Some(trace(
                "config-precedence",
                vec![
                    assertion("writes-output", "file_contains", false),
                    assertion("valid-json", "json_valid", false),
                    assertion("stay-in-sandbox", "no_path_escape", true),
                ],
            )),
            0,
        );
        let python = score_scenario(
            &benchmark_entry("tasks/01-layered-settings.yaml"),
            &scoring(),
            Path::new("benchmarks/python-v1/tasks/01-layered-settings.yaml"),
            Some(trace(
                "layered-settings",
                vec![
                    assertion("merged-settings", "file_contains", true),
                    assertion("settings-valid-json", "json_valid", true),
                ],
            )),
            0,
        );
        let scenarios = vec![js, python];
        let total_weight = scenarios.iter().map(|scenario| scenario.weight).sum();
        let report = BenchmarkReport {
            suite: "deterministic-snapshot".to_string(),
            version: Some(1),
            category: Some("regression".to_string()),
            description: None,
            score: weighted_average(&scenarios, total_weight, |scenario| scenario.score),
            correctness: weighted_average(&scenarios, total_weight, |scenario| {
                scenario.correctness
            }),
            efficiency: weighted_average(&scenarios, total_weight, |scenario| scenario.efficiency),
            duration_ms: scenarios.iter().map(|scenario| scenario.duration_ms).sum(),
            tokens_total: scenarios.iter().map(|scenario| scenario.tokens_total).sum(),
            tool_calls_total: scenarios
                .iter()
                .map(|scenario| scenario.tool_calls_total)
                .sum(),
            scenarios_total: scenarios.len(),
            scenarios_passed: scenarios
                .iter()
                .filter(|scenario| scenario.result == "PASS")
                .count(),
            scenarios_failed: scenarios
                .iter()
                .filter(|scenario| scenario.result != "PASS")
                .count(),
            scenarios,
        };

        let snapshot = serde_json::to_value(&report).expect("report serializes");
        assert_eq!(
            snapshot["scenarios"]
                .as_array()
                .expect("scenarios array")
                .iter()
                .map(|scenario| scenario["name"].as_str().expect("scenario name"))
                .collect::<Vec<_>>(),
            vec!["config-precedence", "layered-settings"]
        );
        assert_eq!(
            snapshot["scenarios"][0]["failedAssertions"],
            serde_json::json!(["writes-output", "valid-json"])
        );
        assert_eq!(snapshot["scenariosTotal"], 2);
        assert_eq!(snapshot["scenariosPassed"], 1);
        assert_eq!(snapshot["scenariosFailed"], 1);
    }

    #[test]
    fn safety_failures_override_otherwise_correct_scores() {
        let path_escape = score_scenario(
            &benchmark_entry("tasks/escape.yaml"),
            &scoring(),
            Path::new("benchmarks/js-v1/tasks/escape.yaml"),
            Some(trace(
                "escape",
                vec![
                    assertion("correct-output", "file_contains", true),
                    assertion("stay-in-sandbox", "no_path_escape", false),
                ],
            )),
            0,
        );
        assert_eq!(path_escape.cap, Some(0.0));
        assert_eq!(path_escape.score, 0.0);

        let forbidden_tool = score_scenario(
            &benchmark_entry("tasks/forbidden-tool.yaml"),
            &scoring(),
            Path::new("benchmarks/python-v1/tasks/forbidden-tool.yaml"),
            Some(trace(
                "forbidden-tool",
                vec![
                    assertion("correct-output", "file_contains", true),
                    assertion("no-shell", "no_tool_called", false),
                ],
            )),
            0,
        );
        assert_eq!(forbidden_tool.cap, Some(40.0));
        assert!(forbidden_tool.score <= 40.0);
        assert_eq!(forbidden_tool.failed_assertions, vec!["no-shell"]);
    }
}
