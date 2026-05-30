use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;

use crate::assertions::{compute_weighted_score, evaluate_assertions, AssertionResult};
use crate::config::load_project_config;
use crate::sandbox::{create_sandbox, SandboxOptions, SkillInstall};
use crate::scenario::{load_scenario_file, LoadedScenario, Scenario};
use crate::skill::{load_skill, sha256_hex, SkillRecord};
use crate::trace::{
    write_trace, ToolCallSummary, TraceRecord, TraceRunner, TraceScenario, TraceScoring, TraceSkill,
};
use crate::ui::{self, Tone};

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub skill: Option<String>,
    pub scenario: Option<String>,
    pub file: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub model: Option<String>,
    pub runtime: Option<String>,
    pub filter: Option<String>,
    pub dry_run: bool,
    pub keep_sandbox: bool,
    pub quiet: bool,
    pub idle_warn_seconds: u64,
}

pub fn run_command(opts: RunOptions) -> anyhow::Result<i32> {
    if opts.dry_run {
        return run_dry_run(opts);
    }
    run_live(opts)
}

fn run_dry_run(opts: RunOptions) -> anyhow::Result<i32> {
    println!("{}", ui::header("ai-tester", "dry run"));
    println!();

    let mut total = 0usize;
    let mut invalid = 0usize;

    match discover_scenarios(&opts) {
        Ok(scenarios) => {
            for loaded in scenarios {
                let scenario = prepare_scenario(&loaded, &opts)?;
                print_scenario_dry_run(&loaded, &scenario);
                total += 1;
            }
        }
        Err(err) => {
            println!(
                "{} scenario discovery failed: {err}",
                ui::paint("x", Tone::Error)
            );
            invalid += 1;
        }
    }

    println!();
    println!("{}", ui::section("Summary"));
    println!("  {}", ui::kv("scenarios", total));
    println!("  {}", ui::kv("invalid", invalid));
    println!();
    if invalid > 0 {
        println!(
            "{} {}",
            ui::status("FAIL", false),
            ui::paint("some scenarios failed to load", Tone::Muted)
        );
        Ok(1)
    } else {
        println!(
            "{} {}",
            ui::status("OK", true),
            ui::paint(
                "all scenarios parsed; no sandbox or runtime calls",
                Tone::Muted
            )
        );
        Ok(0)
    }
}

fn run_live(opts: RunOptions) -> anyhow::Result<i32> {
    let runs_dir = load_project_config(std::env::current_dir()?)?.runs_dir;
    let scenarios = discover_scenarios(&opts)?;
    if scenarios.is_empty() {
        println!("{}", ui::header("ai-tester", "behavioral run"));
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("No scenarios matched", Tone::Muted)
        );
        return Ok(0);
    }
    let total = scenarios.len();

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut runtime_errors = 0usize;

    print_run_banner(total, &opts);

    for (idx, loaded) in scenarios.into_iter().enumerate() {
        let scenario = prepare_scenario(&loaded, &opts)?;

        let skill = resolve_skill(&loaded, &scenario)?;
        let allowed_tools = scenario
            .runner
            .allowed_tools_override
            .clone()
            .unwrap_or_else(|| skill.allowed_tools_raw.clone());
        let runtime_name = scenario.runner.runtime.clone();
        if !crate::runtime::runtime_ready(&runtime_name) {
            println!(
                "{} {}",
                ui::paint(&format!("[{}/{}]", idx + 1, total), Tone::Accent),
                ui::paint(&scenario.scenario, Tone::Strong)
            );
            println!("  {}{}", ui::label("result"), ui::status("ERROR", false));
            println!(
                "  {} `{runtime_name}` runtime is not ready",
                ui::label("reason")
            );
            println!();
            runtime_errors += 1;
            continue;
        }

        print_scenario_start(idx + 1, total, &loaded, &scenario, &skill);

        let started_at = Utc::now();
        let start = Instant::now();
        let sandbox = create_sandbox(
            &scenario.scenario,
            &scenario.fixtures,
            SandboxOptions {
                keep: opts.keep_sandbox,
                skill: skill.install.as_ref().map(|install| SkillInstall {
                    name: install.name.clone(),
                    dir_path: install.dir_path.clone(),
                }),
            },
        )?;
        if !opts.quiet {
            println!(
                "  {}{}",
                ui::label("sandbox"),
                ui::fit_value(sandbox.path.display(), 15)
            );
            if runtime_name == "codex" {
                println!(
                    "  {}{}",
                    ui::label("progress"),
                    ui::paint("Codex event stream", Tone::Info)
                );
            } else {
                println!(
                    "  {}{}",
                    ui::label("progress"),
                    ui::paint("waiting for runtime result", Tone::Info)
                );
            }
        }

        let user_messages = build_user_message_chain(&scenario, &skill.name);
        let runtime_result = match crate::runtime::run_runtime(crate::runtime::RuntimeRunRequest {
            runtime: runtime_name,
            skill_body: skill.body.clone(),
            scenario: scenario.clone(),
            cwd: sandbox.path.clone(),
            user_messages,
            user_responses: scenario.user_responses.clone(),
            allowed_tools,
            skill_install_rel_path: sandbox
                .skill_install_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            progress: !opts.quiet,
        }) {
            Ok(result) => result,
            Err(err) => {
                println!("  {}{}", ui::label("result"), ui::status("ERROR", false));
                println!("  {}{err}", ui::label("reason"));
                let _ = sandbox.cleanup();
                runtime_errors += 1;
                continue;
            }
        };
        let finished_at = Utc::now();

        let mut record = build_trace_record(TraceBuildInput {
            skill: &skill,
            scenario: &scenario,
            scenario_path: &loaded.file_path,
            runtime_result,
            started_at,
            finished_at,
            duration_ms: start.elapsed().as_millis() as u64,
            sandbox_path: Some(sandbox.path.display().to_string()),
        });

        if record.errors.is_empty() {
            let mut assertions = evaluate_assertions(&scenario.assertions, &record);
            if record.runner.hit_max_turns && record.runner.max_turns_user_set {
                assertions.push(turn_budget_assertion(&record));
            }
            let all_passed = assertions.iter().all(|result| result.pass);
            let weighted = compute_weighted_score(&assertions);
            record.assertions = assertions;
            record.scoring.all_passed = all_passed;
            record.scoring.overall_pass = all_passed;
            record.scoring.weighted_score = Some(weighted);
        }

        let trace_path = write_trace(&runs_dir, &record)?;
        print_scenario_result(&record, &trace_path, !opts.quiet);
        sandbox.cleanup()?;
        println!();

        if record.errors.is_empty() && record.scoring.overall_pass {
            passed += 1;
        } else if record.errors.is_empty() {
            failed += 1;
        } else {
            runtime_errors += 1;
        }
    }

    print_run_summary(passed, failed, runtime_errors);

    if failed == 0 && runtime_errors == 0 {
        println!(
            "{} {}",
            ui::paint("●", Tone::Success),
            ui::status("PASS", true)
        );
        Ok(0)
    } else if runtime_errors == 0 {
        println!(
            "{} {}",
            ui::paint("●", Tone::Error),
            ui::status("FAIL", false)
        );
        Ok(1)
    } else {
        println!(
            "{} {}",
            ui::paint("●", Tone::Error),
            ui::status("FAIL", false)
        );
        Ok(2)
    }
}

fn discover_scenarios(opts: &RunOptions) -> anyhow::Result<Vec<LoadedScenario>> {
    if opts.file.is_some() && opts.dir.is_some() {
        anyhow::bail!("pass either --file or --dir, not both");
    }

    if let Some(file) = &opts.file {
        return Ok(vec![load_scenario_file(resolve_scenario_file(file)?)?]);
    }
    if let Some(dir) = &opts.dir {
        return load_scenarios_from_dir(dir, opts);
    }

    let config = load_project_config(std::env::current_dir()?)?;
    let skill_names = if let Some(skill) = &opts.skill {
        vec![skill.clone()]
    } else {
        list_skill_names(&config.skills_dir)?
    };

    let mut out = Vec::new();
    for skill_name in skill_names {
        let tests_dir = config.skills_dir.join(&skill_name).join("tests");
        if !tests_dir.is_dir() {
            continue;
        }
        let mut files = std::fs::read_dir(&tests_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| !name.starts_with('_'))
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            matches!(ext.to_ascii_lowercase().as_str(), "yaml" | "yml")
                        })
            })
            .collect::<Vec<_>>();
        files.sort();

        for file in files {
            let loaded = load_scenario_file(file)?;
            if opts
                .scenario
                .as_ref()
                .is_some_and(|wanted| *wanted != loaded.scenario.scenario)
            {
                continue;
            }
            if let Some(filter) = &opts.filter {
                let re = crate::util::regex::compile_pattern(filter)?;
                if !re.is_match(&loaded.scenario.scenario) {
                    continue;
                }
            }
            out.push(loaded);
        }
    }
    Ok(out)
}

fn resolve_scenario_file(path: &Path) -> anyhow::Result<PathBuf> {
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
    anyhow::bail!("scenario file not found: {}", path.display())
}

fn load_scenarios_from_dir(dir: &Path, opts: &RunOptions) -> anyhow::Result<Vec<LoadedScenario>> {
    if !dir.is_dir() {
        anyhow::bail!("scenario directory not found: {}", dir.display());
    }

    let mut files = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_scenario_file(path))
        .collect::<Vec<_>>();
    files.sort();

    let mut out = Vec::new();
    for file in files {
        let loaded = load_scenario_file(file)?;
        if !scenario_matches_filters(&loaded, opts)? {
            continue;
        }
        out.push(loaded);
    }
    Ok(out)
}

fn is_scenario_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.starts_with('_'))
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "yaml" | "yml"))
}

fn scenario_matches_filters(loaded: &LoadedScenario, opts: &RunOptions) -> anyhow::Result<bool> {
    if opts
        .scenario
        .as_ref()
        .is_some_and(|wanted| *wanted != loaded.scenario.scenario)
    {
        return Ok(false);
    }
    if let Some(filter) = &opts.filter {
        let re = crate::util::regex::compile_pattern(filter)?;
        if !re.is_match(&loaded.scenario.scenario) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn prepare_scenario(loaded: &LoadedScenario, opts: &RunOptions) -> anyhow::Result<Scenario> {
    let mut scenario = loaded.scenario.clone();
    let config = load_project_config(std::env::current_dir()?)?;
    if !loaded.source_meta.runner_runtime_set {
        if let Some(runtime) = config.defaults.runtime {
            scenario.runner.runtime = runtime;
        }
    }
    if !loaded.source_meta.runner_model_set {
        if let Some(model) = config.defaults.model {
            scenario.runner.model = model;
        }
    }
    if !loaded.source_meta.runner_permission_mode_set {
        if let Some(permission_mode) = config.defaults.permission_mode {
            scenario.runner.permission_mode = permission_mode;
        }
    }
    if let Some(model) = opts.model.clone() {
        scenario.runner.model = model;
    }
    if let Some(runtime) = opts.runtime.clone() {
        scenario.runner.runtime = runtime;
    }
    Ok(scenario)
}

fn list_skill_names(skills_dir: &Path) -> anyhow::Result<Vec<String>> {
    if !skills_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = std::fs::read_dir(skills_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn print_run_banner(total: usize, opts: &RunOptions) {
    println!("{}", ui::header("ai-tester", "behavioral run"));
    println!(
        "  {}",
        ui::kv("scenarios", ui::paint(&total.to_string(), Tone::Strong))
    );
    if let Some(runtime) = &opts.runtime {
        println!("  {}", ui::kv("runtime", ui::paint(runtime, Tone::Info)));
    }
    if let Some(model) = &opts.model {
        println!("  {}", ui::kv("model", ui::paint(model, Tone::Info)));
    }
    if let Some(filter) = &opts.filter {
        println!("  {}", ui::kv("filter", filter));
    }
    println!();
}

fn print_scenario_start(
    index: usize,
    total: usize,
    loaded: &LoadedScenario,
    scenario: &Scenario,
    skill: &ResolvedSkill,
) {
    println!(
        "{} {} {}",
        ui::paint(&format!("[{index}/{total}]"), Tone::Accent),
        ui::paint(&scenario.scenario, Tone::Strong),
        ui::paint(&scenario_source_label(scenario), Tone::Muted)
    );
    print_prompt_source(scenario, skill, "  ");
    println!(
        "  {}",
        ui::kv("runtime", ui::paint(&scenario.runner.runtime, Tone::Info))
    );
    println!(
        "  {}",
        ui::kv("model", ui::paint(&scenario.runner.model, Tone::Info))
    );
    println!(
        "  {}",
        ui::kv(
            "permission",
            format_permission_mode(&scenario.runner.permission_mode)
        )
    );
    println!(
        "  {}",
        ui::kv(
            "checks",
            ui::paint(&scenario.assertions.len().to_string(), Tone::Strong)
        )
    );
    println!(
        "  {}{}",
        ui::label("file"),
        ui::fit_value(loaded.file_path.display(), 15)
    );
    if scenario.skill.is_some() {
        println!("  {}", ui::kv("skill", ui::fit_value(&skill.path, 15)));
    }
}

fn print_scenario_dry_run(loaded: &LoadedScenario, scenario: &Scenario) {
    println!(
        "  {} {} {}",
        ui::paint("●", Tone::Success),
        ui::paint(&scenario.scenario, Tone::Strong),
        ui::paint(&format!("({})", loaded.file_path.display()), Tone::Muted)
    );
    println!("    {}", ui::kv("source", scenario_source_label(scenario)));
    println!(
        "    {}",
        ui::kv("prompt", prompt_preview_for_scenario(scenario))
    );
    println!("    {}", ui::kv("runtime", &scenario.runner.runtime));
    println!("    {}", ui::kv("model", &scenario.runner.model));
    println!(
        "    {}",
        ui::kv(
            "permission",
            format_permission_mode(&scenario.runner.permission_mode)
        )
    );
    println!("    {}", ui::kv("assertions", scenario.assertions.len()));
}

fn scenario_source_label(scenario: &Scenario) -> String {
    if let Some(skill) = &scenario.skill {
        format!("skill · {skill}")
    } else if scenario.system_prompt.is_some() {
        "inline prompt".to_string()
    } else {
        "prompt file".to_string()
    }
}

fn print_prompt_source(scenario: &Scenario, skill: &ResolvedSkill, indent: &str) {
    if let Some(skill_name) = &scenario.skill {
        println!(
            "{indent}{}",
            ui::kv(
                "prompt",
                format!("skill {skill_name} · {}", ui::fit_value(&skill.path, 24))
            )
        );
        return;
    }
    if let Some(system_prompt_file) = &scenario.system_prompt_file {
        println!(
            "{indent}{}",
            ui::kv("prompt", format!("file {system_prompt_file}"))
        );
        println!("{indent}{}", ui::kv("preview", prompt_preview(&skill.body)));
        return;
    }
    println!("{indent}{}", ui::kv("prompt", "inline YAML"));
    println!("{indent}{}", ui::kv("preview", prompt_preview(&skill.body)));
}

fn prompt_preview_for_scenario(scenario: &Scenario) -> String {
    if let Some(skill) = &scenario.skill {
        return format!("skill {skill}");
    }
    if let Some(system_prompt_file) = &scenario.system_prompt_file {
        return format!("file {system_prompt_file}");
    }
    scenario
        .system_prompt
        .as_deref()
        .map(prompt_preview)
        .unwrap_or_else(|| "unknown".to_string())
}

fn prompt_preview(prompt: &str) -> String {
    let preview = prompt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    ui::fit_value(preview, 24)
}

fn format_permission_mode(mode: &str) -> String {
    match mode {
        "bypassPermissions" => format!(
            "{} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("bypassPermissions", Tone::Warning)
        ),
        "acceptEdits" => format!(
            "{} {}",
            ui::paint("●", Tone::Success),
            ui::paint("acceptEdits", Tone::Success)
        ),
        "plan" => format!(
            "{} {}",
            ui::paint("●", Tone::Info),
            ui::paint("plan", Tone::Info)
        ),
        "default" => format!(
            "{} {}",
            ui::paint("●", Tone::Muted),
            ui::paint("default", Tone::Muted)
        ),
        other => format!(
            "{} {}",
            ui::paint("●", Tone::Muted),
            ui::paint(other, Tone::Muted)
        ),
    }
}

#[derive(Debug, Clone)]
struct ResolvedSkill {
    name: String,
    path: String,
    body: String,
    body_hash: String,
    source_hash: String,
    version: Option<String>,
    token_budget: Option<f64>,
    allowed_tools_parsed: Vec<crate::skill::allowed_tools::ParsedTool>,
    allowed_tools_raw: Vec<String>,
    install: Option<ResolvedSkillInstall>,
}

#[derive(Debug, Clone)]
struct ResolvedSkillInstall {
    name: String,
    dir_path: PathBuf,
}

fn resolve_skill(loaded: &LoadedScenario, scenario: &Scenario) -> anyhow::Result<ResolvedSkill> {
    if let Some(skill_name) = &scenario.skill {
        let config = load_project_config(std::env::current_dir()?)?;
        let skill = load_skill(config.skills_dir, skill_name)?;
        return Ok(resolved_skill_from_record(skill));
    }
    let body = if let Some(system_prompt) = &scenario.system_prompt {
        system_prompt.clone()
    } else if let Some(system_prompt_file) = &scenario.system_prompt_file {
        let base = loaded
            .file_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::read_to_string(base.join(system_prompt_file))?
    } else {
        anyhow::bail!("scenario has no prompt source");
    };
    let hash = sha256_hex(body.as_bytes());
    Ok(ResolvedSkill {
        name: format!("inline:{}", scenario.scenario),
        path: loaded.file_path.display().to_string(),
        body,
        body_hash: hash.clone(),
        source_hash: hash,
        version: None,
        token_budget: None,
        allowed_tools_parsed: Vec::new(),
        allowed_tools_raw: Vec::new(),
        install: None,
    })
}

fn resolved_skill_from_record(skill: SkillRecord) -> ResolvedSkill {
    ResolvedSkill {
        name: skill.name.clone(),
        path: skill.skill_md_path.display().to_string(),
        body: skill.body,
        body_hash: skill.body_hash,
        source_hash: skill.source_hash,
        version: skill.frontmatter.version,
        token_budget: skill.token_budget,
        allowed_tools_parsed: skill.allowed_tools_parsed,
        allowed_tools_raw: skill.allowed_tools_raw,
        install: Some(ResolvedSkillInstall {
            name: skill.name,
            dir_path: skill.dir_path,
        }),
    }
}

fn build_user_message_chain(scenario: &Scenario, skill_name: &str) -> Vec<String> {
    if let Some(prompts) = &scenario.user_prompts {
        return prompts.clone();
    }
    if let Some(prompt) = &scenario.user_prompt {
        return vec![prompt.clone()];
    }
    if scenario.skill.is_some() {
        let arg_line = scenario
            .argument
            .as_ref()
            .filter(|arg| !arg.is_empty())
            .map(|arg| format!("\nArgument: {arg}"))
            .unwrap_or_default();
        vec![format!(
            "Run the {skill_name} skill defined in your system prompt. Follow its instructions end-to-end against the current working directory.{arg_line}"
        )]
    } else {
        vec![scenario
            .argument
            .clone()
            .filter(|arg| !arg.is_empty())
            .unwrap_or_else(|| "Begin.".to_string())]
    }
}

struct TraceBuildInput<'a> {
    skill: &'a ResolvedSkill,
    scenario: &'a Scenario,
    scenario_path: &'a Path,
    runtime_result: crate::runtime::RuntimeRunResult,
    started_at: chrono::DateTime<Utc>,
    finished_at: chrono::DateTime<Utc>,
    duration_ms: u64,
    sandbox_path: Option<String>,
}

fn build_trace_record(input: TraceBuildInput<'_>) -> TraceRecord {
    let TraceBuildInput {
        skill,
        scenario,
        scenario_path,
        runtime_result,
        started_at,
        finished_at,
        duration_ms,
        sandbox_path,
    } = input;
    let run_id = format!(
        "{}__{}__{}",
        crate::trace::sanitize_path_segment(&skill.name),
        finished_at.format("%Y-%m-%dT%H-%M-%SZ"),
        &skill.source_hash[..8]
    );
    let hit_max_turns = runtime_result.stopped_reason == "max_turns";
    let summary =
        ToolCallSummary::from_turns(&runtime_result.turns, runtime_result.unanswered_questions);
    TraceRecord {
        schema_version: "2.0.0".to_string(),
        run_id,
        skill: TraceSkill {
            name: skill.name.clone(),
            path: skill.path.clone(),
            version: skill.version.clone(),
            source_hash: skill.source_hash.clone(),
            source_hash_short: skill.source_hash.chars().take(8).collect(),
            body_hash: skill.body_hash.clone(),
            allowed_tools_parsed: skill.allowed_tools_parsed.clone(),
            allowed_tools_raw: skill.allowed_tools_raw.clone(),
            token_budget: skill.token_budget,
        },
        scenario: TraceScenario {
            name: scenario.scenario.clone(),
            path: scenario_path.display().to_string(),
            argument: scenario.argument.clone(),
            token_budget: scenario.token_budget,
        },
        runner: TraceRunner {
            runtime: scenario.runner.runtime.clone(),
            model: scenario.runner.model.clone(),
            permission_mode: scenario.runner.permission_mode.clone(),
            started_at,
            finished_at,
            duration_ms,
            max_turns: runtime_result.max_turns_effective,
            max_turns_user_set: runtime_result.max_turns_user_set,
            turns_used: runtime_result.turns_used,
            hit_max_turns,
            session_id: runtime_result.session_id,
            sandbox_path,
        },
        turns: runtime_result.turns,
        final_output: runtime_result.final_output,
        tool_call_summary: summary,
        assertions: Vec::new(),
        scoring: TraceScoring {
            all_passed: false,
            overall_pass: false,
            weighted_score: None,
            pass_threshold: None,
        },
        cost: runtime_result.cost,
        errors: runtime_result.errors,
        diagnostics: runtime_result.diagnostics,
    }
}

fn turn_budget_assertion(record: &TraceRecord) -> AssertionResult {
    AssertionResult {
        id: "turn_budget".to_string(),
        kind: "turn_budget".to_string(),
        pass: false,
        detail: format!(
            "runtime stopped after hitting explicit max_turns={} (turns used: {})",
            record.runner.max_turns, record.runner.turns_used
        ),
        weight: 1.0,
        score: None,
        min_score: None,
        rationale: None,
        captures: Vec::new(),
    }
}

fn print_scenario_result(record: &TraceRecord, trace_path: &Path, verbose: bool) {
    let pass = record.scoring.overall_pass;
    let status = if pass { "PASS" } else { "FAIL" };
    let tone = if pass { Tone::Success } else { Tone::Error };
    let mut stats = vec![
        format_duration(record.runner.duration_ms),
        format!(
            "{}/{} turns",
            record.runner.turns_used, record.runner.max_turns
        ),
    ];
    if let Some(score) = record.scoring.weighted_score {
        stats.push(format_score(score));
    }
    println!(
        "  {} {} {}",
        ui::paint("●", tone),
        ui::status(status, pass),
        ui::paint(&stats.join(" · "), Tone::Muted)
    );
    println!(
        "  {}{}",
        ui::label("trace"),
        ui::fit_value(trace_path.display(), 15)
    );

    if verbose && !record.assertions.is_empty() {
        println!("  {}", ui::section("Assertions"));
        for assertion in &record.assertions {
            let tone = if assertion.pass {
                Tone::Success
            } else {
                Tone::Error
            };
            println!(
                "    {} {} {}",
                ui::paint("●", tone),
                ui::paint(&assertion.id, Tone::Strong),
                ui::paint(&assertion.detail, Tone::Muted)
            );
        }
    } else if !record.assertions.is_empty() {
        let passed = record
            .assertions
            .iter()
            .filter(|assertion| assertion.pass)
            .count();
        println!(
            "  {}{}",
            ui::label("checks"),
            ui::paint(
                &format!("{passed}/{}", record.assertions.len()),
                Tone::Strong
            )
        );
        let failed_assertions = record
            .assertions
            .iter()
            .filter(|assertion| !assertion.pass)
            .collect::<Vec<_>>();
        if !failed_assertions.is_empty() {
            println!("  {}", ui::section("Failures"));
            for assertion in failed_assertions {
                println!(
                    "    {} {} {}",
                    ui::paint("●", Tone::Error),
                    ui::paint(&assertion.id, Tone::Strong),
                    ui::paint(&assertion.detail, Tone::Muted)
                );
            }
        }
    }

    for error in &record.errors {
        println!(
            "  {}{}: {}",
            ui::label("error"),
            ui::paint(&error.kind, Tone::Error),
            error.message
        );
    }
}

fn print_run_summary(passed: usize, failed: usize, runtime_errors: usize) {
    println!("{}", ui::section("Results"));
    println!(
        "  {} {}  {} {}  {} {}",
        ui::paint("●", Tone::Success),
        ui::paint(&format!("{passed} passed"), Tone::Success),
        ui::paint("●", Tone::Error),
        ui::paint(&format!("{failed} failed"), Tone::Error),
        ui::paint("●", Tone::Warning),
        ui::paint(&format!("{runtime_errors} errors"), Tone::Warning)
    );
    println!();
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}

fn format_score(score: f64) -> String {
    let value = format!("{:.0}%", score * 100.0);
    if score >= 1.0 {
        ui::paint(&value, Tone::Success)
    } else if score >= 0.75 {
        ui::paint(&value, Tone::Warning)
    } else {
        ui::paint(&value, Tone::Error)
    }
}
