use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use walkdir::WalkDir;

use crate::ui::{self, Tone};

/// Prefix every sandbox temp dir is created with (see `sandbox::create_sandbox`).
const SANDBOX_PREFIX: &str = "ai-tester-";

struct Orphan {
    path: PathBuf,
    name: String,
    age: Duration,
    size_bytes: u64,
}

pub fn sandbox_prune_command(yes: bool, min_age_seconds: u64) -> anyhow::Result<i32> {
    println!("{}", ui::header("ai-tester", "sandbox-prune"));

    let root = std::env::temp_dir();
    println!("  {}", ui::kv("temp dir", root.display()));
    println!("  {}", ui::kv("min age", format!("{min_age_seconds}s")));

    let min_age = Duration::from_secs(min_age_seconds);
    let mut orphans = collect_orphans(&root, min_age)?;
    orphans.sort_by_key(|o| std::cmp::Reverse(o.age));

    if orphans.is_empty() {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Success),
            ui::paint("no orphan sandboxes found", Tone::Muted)
        );
        return Ok(0);
    }

    let total_bytes: u64 = orphans.iter().map(|o| o.size_bytes).sum();
    println!(
        "  {}",
        ui::section(&format!(
            "{} orphan sandbox(es), {}",
            orphans.len(),
            human_size(total_bytes)
        ))
    );

    let mut removed = 0usize;
    let mut freed = 0u64;
    let mut failed = 0usize;

    for orphan in &orphans {
        let detail = format!(
            "{}  {}",
            human_age(orphan.age),
            ui::paint(&human_size(orphan.size_bytes), Tone::Muted)
        );

        if !yes {
            println!(
                "    {} {}  {}",
                ui::paint("○", Tone::Warning),
                ui::paint(&orphan.name, Tone::Strong),
                detail
            );
            continue;
        }

        match fs::remove_dir_all(&orphan.path) {
            Ok(()) => {
                removed += 1;
                freed += orphan.size_bytes;
                println!(
                    "    {} {}  {}",
                    ui::paint("✓", Tone::Success),
                    ui::paint(&orphan.name, Tone::Strong),
                    detail
                );
            }
            Err(err) => {
                failed += 1;
                println!(
                    "    {} {}  {}",
                    ui::paint("✗", Tone::Error),
                    ui::paint(&orphan.name, Tone::Strong),
                    ui::paint(&err.to_string(), Tone::Error)
                );
            }
        }
    }

    if yes {
        println!(
            "  {}",
            ui::kv(
                "removed",
                format!("{removed} sandbox(es), {} freed", human_size(freed))
            )
        );
        if failed > 0 {
            println!(
                "  {} {}",
                ui::paint("●", Tone::Error),
                ui::paint(&format!("{failed} could not be removed"), Tone::Error)
            );
            return Ok(1);
        }
    } else {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Info),
            ui::paint("dry run — re-run with --yes to delete", Tone::Muted)
        );
    }

    Ok(0)
}

fn collect_orphans(root: &std::path::Path, min_age: Duration) -> anyhow::Result<Vec<Orphan>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };

    let mut orphans = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(SANDBOX_PREFIX) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(md) if md.is_dir() => md,
            _ => continue,
        };
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .unwrap_or_default();
        if age < min_age {
            continue;
        }
        orphans.push(Orphan {
            size_bytes: dir_size(&path),
            path,
            name,
            age,
        });
    }
    Ok(orphans)
}

fn dir_size(path: &std::path::Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|md| md.len())
        .sum()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn human_age(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}
