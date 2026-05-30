pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
pub const INTERNAL_MAX_TURNS: u32 = 40;
pub const DEFAULT_PASS_THRESHOLD: f64 = 0.85;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub root_dir: PathBuf,
    pub config_path: Option<PathBuf>,
    pub skills_dir: PathBuf,
    pub runs_dir: PathBuf,
    pub defaults: ProjectDefaults,
    pub acp_agents: BTreeMap<String, AcpAgentConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectDefaults {
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AcpAgentConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawProjectConfig {
    skills_dir: Option<String>,
    runs_dir: Option<String>,
    defaults: Option<RawProjectDefaults>,
    #[serde(default)]
    acp_agents: BTreeMap<String, AcpAgentConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawProjectDefaults {
    runtime: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    agent: Option<String>,
}

pub fn load_project_config(start_dir: impl AsRef<Path>) -> anyhow::Result<ProjectConfig> {
    let start = absolutize(start_dir.as_ref())?;
    let mut current = start.as_path();
    let mut found = None;

    loop {
        let candidate = current.join(".ai-tester.yaml");
        if candidate.is_file() {
            found = Some(candidate);
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent;
    }

    if let Some(config_path) = found {
        let root_dir = config_path.parent().unwrap_or(&start).to_path_buf();
        let raw: RawProjectConfig = yaml_serde::from_str(&fs::read_to_string(&config_path)?)?;
        let skills_dir = raw
            .skills_dir
            .as_deref()
            .map(|p| root_dir.join(p))
            .unwrap_or_else(|| root_dir.join("skills"));
        let runs_dir = raw
            .runs_dir
            .as_deref()
            .map(|p| root_dir.join(p))
            .unwrap_or_else(|| root_dir.join("runs"));
        Ok(ProjectConfig {
            root_dir,
            config_path: Some(config_path),
            skills_dir,
            runs_dir,
            defaults: ProjectDefaults {
                runtime: raw.defaults.as_ref().and_then(|d| d.runtime.clone()),
                model: raw.defaults.as_ref().and_then(|d| d.model.clone()),
                permission_mode: raw
                    .defaults
                    .as_ref()
                    .and_then(|d| d.permission_mode.clone()),
                agent: raw.defaults.and_then(|d| d.agent),
            },
            acp_agents: raw.acp_agents,
        })
    } else {
        Ok(ProjectConfig {
            root_dir: start.clone(),
            config_path: None,
            skills_dir: start.join("skills"),
            runs_dir: start.join("runs"),
            defaults: ProjectDefaults::default(),
            acp_agents: BTreeMap::new(),
        })
    }
}

/// Resolve the runs directory for the project rooted at the current working dir.
pub fn resolve_runs_dir() -> anyhow::Result<PathBuf> {
    Ok(load_project_config(std::env::current_dir()?)?.runs_dir)
}

fn absolutize(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
