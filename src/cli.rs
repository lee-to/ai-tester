use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::commands::benchmark::{benchmark_command, BenchmarkOptions};
use crate::commands::init::{init_command, InitOptions};
use crate::commands::run::{run_command, RunOptions};

#[derive(Debug, Parser)]
#[command(name = "ai-tester")]
#[command(about = "Native Rust behavioral test harness for agent skills and prompts")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    Init {
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "./skills")]
        skills_dir: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "bypassPermissions")]
        permission_mode: String,
        #[arg(long)]
        acp_agent: Option<String>,
    },
    Run {
        skill: Option<String>,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        reasoning: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        mcp_profile: Option<String>,
        #[arg(long)]
        acp_log: Option<PathBuf>,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        keep_sandbox: bool,
        #[arg(long)]
        quiet: bool,
        #[arg(long, default_value_t = 30)]
        idle_warn: u64,
        #[arg(long)]
        setup_timeout: Option<u64>,
        #[arg(long)]
        acp_turn_timeout: Option<u64>,
        /// Output format: live (default), json, or markdown
        #[arg(long, value_enum, default_value_t)]
        format: crate::commands::run::OutputFormat,
    },
    #[command(about = "Run a deterministic benchmark suite and compute a 0..100 score")]
    Benchmark {
        #[arg(long)]
        suite: PathBuf,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        reasoning: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        mcp_profile: Option<String>,
        #[arg(long)]
        acp_log: Option<PathBuf>,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        keep_sandbox: bool,
        #[arg(long)]
        quiet: bool,
        #[arg(long, default_value_t = 30)]
        idle_warn: u64,
        #[arg(long)]
        setup_timeout: Option<u64>,
        #[arg(long)]
        acp_turn_timeout: Option<u64>,
        #[arg(long, value_enum, default_value_t)]
        format: crate::commands::run::OutputFormat,
    },
    History {
        skill: Option<String>,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long, default_value_t = 20)]
        last: usize,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Show score trend from recorded v2 traces")]
    Trend {
        skill: String,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long, default_value_t = 20)]
        last: usize,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Diff two recorded runs side-by-side")]
    Compare {
        run_a: String,
        run_b: String,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Pretty-print a recorded trace")]
    Trace {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
    Runtimes,
    #[command(about = "Remove orphan ai-tester sandbox dirs left in the temp directory")]
    SandboxPrune {
        /// Actually delete (without this it's a dry run)
        #[arg(long)]
        yes: bool,
        /// Only prune sandboxes older than this many seconds
        #[arg(long, default_value_t = 60)]
        min_age: u64,
    },
    #[command(about = "Update ai-tester to the latest GitHub release for this platform")]
    Update {
        /// Only report whether an update is available
        #[arg(long)]
        check: bool,
        /// Reinstall even if already on the latest version
        #[arg(long)]
        force: bool,
        /// Install a specific tag instead of the latest release
        #[arg(long)]
        tag: Option<String>,
    },
}

pub fn main_entry() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Init {
            force,
            skills_dir,
            model,
            permission_mode,
            acp_agent,
        } => init_command(InitOptions {
            force,
            skills_dir,
            model,
            permission_mode,
            acp_agent,
        })?,
        Commands::Run {
            skill,
            scenario,
            file,
            dir,
            model,
            mode,
            reasoning,
            runtime,
            agent,
            mcp_profile,
            acp_log,
            filter,
            dry_run,
            keep_sandbox,
            quiet,
            idle_warn,
            setup_timeout,
            acp_turn_timeout,
            format,
        } => run_command(RunOptions {
            skill,
            scenario,
            file,
            dir,
            model,
            mode,
            reasoning,
            runtime,
            agent,
            mcp_profile,
            acp_log,
            filter,
            dry_run,
            keep_sandbox,
            quiet,
            idle_warn_seconds: idle_warn,
            setup_timeout_seconds: setup_timeout,
            acp_turn_timeout_seconds: acp_turn_timeout,
            format,
        })?,
        Commands::Benchmark {
            suite,
            model,
            mode,
            reasoning,
            runtime,
            agent,
            mcp_profile,
            acp_log,
            filter,
            keep_sandbox,
            quiet,
            idle_warn,
            setup_timeout,
            acp_turn_timeout,
            format,
        } => benchmark_command(BenchmarkOptions {
            suite,
            model,
            mode,
            reasoning,
            runtime,
            agent,
            mcp_profile,
            acp_log,
            filter,
            keep_sandbox,
            quiet,
            idle_warn_seconds: idle_warn,
            setup_timeout_seconds: setup_timeout,
            acp_turn_timeout_seconds: acp_turn_timeout,
            format,
        })?,
        Commands::History {
            skill,
            scenario,
            last,
            json,
        } => crate::commands::history::history_command(crate::commands::history::HistoryOptions {
            skill,
            scenario,
            last,
            json,
        })?,
        Commands::Trend {
            skill,
            scenario,
            last,
            json,
        } => crate::commands::recorded_runs::trend_command(
            crate::commands::recorded_runs::TrendOptions {
                skill,
                scenario,
                last,
                json,
            },
        )?,
        Commands::Compare { run_a, run_b, json } => {
            crate::commands::recorded_runs::compare_command(
                crate::commands::recorded_runs::CompareOptions { run_a, run_b, json },
            )?
        }
        Commands::Trace { run_id, json } => crate::commands::recorded_runs::trace_command(
            crate::commands::recorded_runs::TraceOptions { run_id, json },
        )?,
        Commands::Runtimes => {
            println!("{}", crate::ui::header("ai-tester", "runtimes"));
            let config = crate::config::load_project_config(std::env::current_dir()?)?;
            for status in crate::runtime::list_runtime_statuses(&config) {
                let tone = if status.ready {
                    crate::ui::Tone::Success
                } else {
                    crate::ui::Tone::Error
                };
                let label = if status.ready { "ready" } else { "not-ready" };
                println!(
                    "  {} {} {} {}",
                    crate::ui::paint("●", tone),
                    crate::ui::paint(&status.name, crate::ui::Tone::Warning),
                    crate::ui::paint(label, tone),
                    crate::ui::paint(&status.description, crate::ui::Tone::Muted)
                );
                if let Some(message) = status.message {
                    println!("    {}", crate::ui::kv("reason", message));
                }
            }
            0
        }
        Commands::SandboxPrune { yes, min_age } => {
            crate::commands::sandbox_prune::sandbox_prune_command(yes, min_age)?
        }
        Commands::Update { check, force, tag } => {
            crate::commands::update::update_command(crate::commands::update::UpdateOptions {
                check,
                force,
                tag,
            })?
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
