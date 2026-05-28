use std::fs;
use std::path::Path;

use anyhow::bail;

use crate::ui::{self, Tone};

pub struct InitOptions {
    pub force: bool,
    pub skills_dir: String,
    pub model: String,
    pub permission_mode: String,
}

pub fn init_command(opts: InitOptions) -> anyhow::Result<i32> {
    const ALLOWED: &[&str] = &["acceptEdits", "bypassPermissions", "plan", "default"];
    if !ALLOWED.contains(&opts.permission_mode.as_str()) {
        bail!(
            "Invalid --permission-mode `{}`. Expected one of: {}",
            opts.permission_mode,
            ALLOWED.join(", ")
        );
    }

    let path = Path::new(".ai-tester.yaml");
    if path.exists() && !opts.force {
        bail!(".ai-tester.yaml already exists; pass --force to overwrite");
    }
    let content = format!(
        "skills_dir: {}\ndefaults:\n  model: {}\n  permission_mode: {}\n",
        opts.skills_dir, opts.model, opts.permission_mode
    );
    fs::write(path, content)?;
    println!("{}", ui::header("ai-tester", "init"));
    println!(
        "  {} {}",
        ui::paint("●", Tone::Success),
        ui::paint("Created", Tone::Strong)
    );
    println!("  {}", ui::kv("config", path.display()));
    Ok(0)
}
