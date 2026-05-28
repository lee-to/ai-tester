use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
enum Commands {
    Init {
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "./skills")]
        skills_dir: String,
        #[arg(long, default_value = crate::config::DEFAULT_MODEL)]
        model: String,
        #[arg(long, default_value = "bypassPermissions")]
        permission_mode: String,
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
        runtime: Option<String>,
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
    Trend {
        skill: String,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long, default_value_t = 20)]
        last: usize,
    },
    Compare {
        run_a: String,
        run_b: String,
    },
    Trace {
        run_id: String,
    },
    Runtimes,
    SandboxPrune {
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 60)]
        min_age: u64,
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
        } => init_command(InitOptions {
            force,
            skills_dir,
            model,
            permission_mode,
        })?,
        Commands::Run {
            skill,
            scenario,
            file,
            dir,
            model,
            runtime,
            filter,
            dry_run,
            keep_sandbox,
            quiet,
            idle_warn,
        } => run_command(RunOptions {
            skill,
            scenario,
            file,
            dir,
            model,
            runtime,
            filter,
            dry_run,
            keep_sandbox,
            quiet,
            idle_warn_seconds: idle_warn,
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
        Commands::Trend { .. } => crate::commands::stubs::trend_command(),
        Commands::Compare { .. } => crate::commands::stubs::compare_command(),
        Commands::Trace { .. } => crate::commands::stubs::trace_command(),
        Commands::Runtimes => {
            for status in crate::runtime::list_runtime_statuses() {
                let label = if status.ready { "ready" } else { "not-ready" };
                println!("  {:<10} {:<9} {}", status.name, label, status.description);
                if let Some(message) = status.message {
                    println!("             {message}");
                }
            }
            0
        }
        Commands::SandboxPrune { yes, min_age } => {
            crate::commands::sandbox_prune::sandbox_prune_command(yes, min_age)?
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
