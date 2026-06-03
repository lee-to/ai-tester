use std::fs;
use std::path::Path;

use anyhow::bail;

use crate::ui::{self, Tone};

pub struct InitOptions {
    pub force: bool,
    pub skills_dir: String,
    pub model: Option<String>,
    pub permission_mode: String,
    pub acp_agent: Option<String>,
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
    let acp_agent = opts.acp_agent.as_deref();
    if let Some(agent) = acp_agent {
        if crate::config::BuiltinAcpAgentProfile::from_name(agent).is_none() {
            let allowed = crate::config::BUILTIN_ACP_AGENT_PROFILES
                .iter()
                .map(|profile| profile.name())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("Invalid --acp-agent `{agent}`. Expected one of: {allowed}");
        }
    }

    let mut content = format!(
        "skills_dir: {}\n# runs_dir: runs   # where recorded run traces are stored (default: ./runs)\ndefaults:\n",
        opts.skills_dir
    );
    if let Some(agent) = acp_agent {
        content.push_str("  runtime: acp\n");
        content.push_str(&format!("  agent: {agent}\n"));
    }
    if let Some(model) = opts
        .model
        .as_deref()
        .or_else(|| acp_agent.is_none().then_some(crate::config::DEFAULT_MODEL))
    {
        content.push_str(&format!("  model: {model}\n"));
    }
    content.push_str(&format!("  permission_mode: {}\n", opts.permission_mode));
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
