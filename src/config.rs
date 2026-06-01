pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
pub const INTERNAL_MAX_TURNS: u32 = 40;
pub const DEFAULT_PASS_THRESHOLD: f64 = 0.85;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub root_dir: PathBuf,
    pub config_path: Option<PathBuf>,
    pub skills_dir: PathBuf,
    pub runs_dir: PathBuf,
    pub defaults: ProjectDefaults,
    pub acp_agents: BTreeMap<String, AcpAgentConfig>,
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    pub mcp_profiles: BTreeMap<String, McpProfileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectDefaults {
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub reasoning: Option<String>,
    pub permission_mode: Option<String>,
    pub agent: Option<String>,
    pub mcp_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AcpAgentConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerTransport {
    Stdio,
    Http,
    Sse,
}

impl Default for McpServerTransport {
    fn default() -> Self {
        Self::Stdio
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpServerConfig {
    #[serde(default, rename = "type")]
    pub transport: McpServerTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpProfileConfig {
    #[serde(default)]
    pub servers: Vec<String>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedMcpServerConfig {
    pub name: String,
    pub config: McpServerConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerResolution {
    pub profile: Option<String>,
    pub servers: Vec<NamedMcpServerConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawProjectConfig {
    skills_dir: Option<String>,
    runs_dir: Option<String>,
    defaults: Option<RawProjectDefaults>,
    #[serde(default)]
    acp_agents: BTreeMap<String, AcpAgentConfig>,
    #[serde(default)]
    mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    mcp_profiles: BTreeMap<String, McpProfileConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawProjectDefaults {
    runtime: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    reasoning: Option<String>,
    permission_mode: Option<String>,
    agent: Option<String>,
    mcp_profile: Option<String>,
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
                mode: raw.defaults.as_ref().and_then(|d| d.mode.clone()),
                reasoning: raw.defaults.as_ref().and_then(|d| d.reasoning.clone()),
                permission_mode: raw
                    .defaults
                    .as_ref()
                    .and_then(|d| d.permission_mode.clone()),
                agent: raw.defaults.as_ref().and_then(|d| d.agent.clone()),
                mcp_profile: raw.defaults.and_then(|d| d.mcp_profile),
            },
            acp_agents: raw.acp_agents,
            mcp_servers: raw.mcp_servers,
            mcp_profiles: raw.mcp_profiles,
        })
    } else {
        Ok(ProjectConfig {
            root_dir: start.clone(),
            config_path: None,
            skills_dir: start.join("skills"),
            runs_dir: start.join("runs"),
            defaults: ProjectDefaults::default(),
            acp_agents: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
            mcp_profiles: BTreeMap::new(),
        })
    }
}

pub fn resolve_mcp_servers_for_run(
    project: &ProjectConfig,
    scenario_mcp_servers: &BTreeMap<String, McpServerConfig>,
    runner_mcp_profile: Option<&str>,
    cli_mcp_profile: Option<&str>,
) -> anyhow::Result<McpServerResolution> {
    let selected_profile = non_empty(cli_mcp_profile)
        .or_else(|| non_empty(runner_mcp_profile))
        .or_else(|| non_empty(project.defaults.mcp_profile.as_deref()));
    let mut registry = project.mcp_servers.clone();
    let mut active_names = None;

    if let Some(profile_name) = selected_profile {
        let profile = project
            .mcp_profiles
            .get(profile_name)
            .with_context(|| format!("unknown MCP profile `{profile_name}`"))?;
        for (name, server) in &profile.mcp_servers {
            registry.insert(name.clone(), server.clone());
        }
        active_names = Some(profile.servers.clone());
    }

    let scenario_names = scenario_mcp_servers.keys().cloned().collect::<Vec<_>>();
    for (name, server) in scenario_mcp_servers {
        registry.insert(name.clone(), server.clone());
    }

    let names = if let Some(profile_names) = active_names {
        unique_ordered(
            profile_names
                .into_iter()
                .chain(scenario_names)
                .collect::<Vec<_>>(),
        )
    } else {
        registry.keys().cloned().collect::<Vec<_>>()
    };

    let mut servers = Vec::new();
    for name in names {
        let config = registry
            .get(&name)
            .with_context(|| format!("unknown MCP server `{name}` in selected MCP profile"))?;
        validate_mcp_server(&name, config)?;
        servers.push(NamedMcpServerConfig {
            name,
            config: config.clone(),
        });
    }

    Ok(McpServerResolution {
        profile: selected_profile.map(ToOwned::to_owned),
        servers,
    })
}

pub fn mcp_servers_diagnostic(servers: &[NamedMcpServerConfig]) -> String {
    let values = servers
        .iter()
        .map(redacted_mcp_server_value)
        .collect::<Vec<_>>();
    let json = serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string());
    format!("ACP MCP servers: {json}")
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

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn unique_ordered(names: Vec<String>) -> Vec<String> {
    let mut seen = BTreeMap::<String, ()>::new();
    let mut out = Vec::new();
    for name in names {
        if seen.insert(name.clone(), ()).is_none() {
            out.push(name);
        }
    }
    out
}

fn validate_mcp_server(name: &str, config: &McpServerConfig) -> anyhow::Result<()> {
    match config.transport {
        McpServerTransport::Stdio => {
            if config
                .command
                .as_deref()
                .is_none_or(|command| command.trim().is_empty())
            {
                bail!("MCP server `{name}` with type `stdio` requires `command`");
            }
        }
        McpServerTransport::Http => {
            if config
                .url
                .as_deref()
                .is_none_or(|url| url.trim().is_empty())
            {
                bail!("MCP server `{name}` with type `http` requires `url`");
            }
        }
        McpServerTransport::Sse => {
            if config
                .url
                .as_deref()
                .is_none_or(|url| url.trim().is_empty())
            {
                bail!("MCP server `{name}` with type `sse` requires `url`");
            }
        }
    }
    Ok(())
}

fn redacted_mcp_server_value(server: &NamedMcpServerConfig) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(server.name.clone()));
    object.insert(
        "type".to_string(),
        Value::String(mcp_transport_name(&server.config.transport).to_string()),
    );
    match server.config.transport {
        McpServerTransport::Stdio => {
            if let Some(command) = &server.config.command {
                object.insert("command".to_string(), Value::String(command.clone()));
            }
            object.insert(
                "argsCount".to_string(),
                Value::Number(serde_json::Number::from(server.config.args.len())),
            );
            object.insert(
                "env".to_string(),
                Value::Object(redacted_string_map(&server.config.env)),
            );
        }
        McpServerTransport::Http | McpServerTransport::Sse => {
            if let Some(url) = &server.config.url {
                object.insert("url".to_string(), Value::String(redact_url(url)));
            }
            object.insert(
                "headers".to_string(),
                Value::Object(redacted_string_map(&server.config.headers)),
            );
        }
    }
    Value::Object(object)
}

fn redacted_string_map(values: &BTreeMap<String, String>) -> Map<String, Value> {
    values
        .keys()
        .map(|key| (key.clone(), Value::String("<redacted>".to_string())))
        .collect()
}

fn redact_url(url: &str) -> String {
    let (without_fragment, fragment) = url
        .split_once('#')
        .map(|(url, fragment)| (url, Some(fragment)))
        .unwrap_or((url, None));
    let mut redacted = without_fragment
        .split_once('?')
        .map(|(base, _)| format!("{base}?<redacted>"))
        .unwrap_or_else(|| without_fragment.to_string());
    if let Some(fragment) = fragment {
        redacted.push('#');
        redacted.push_str(fragment);
    }
    redacted
}

fn mcp_transport_name(transport: &McpServerTransport) -> &'static str {
    match transport {
        McpServerTransport::Stdio => "stdio",
        McpServerTransport::Http => "http",
        McpServerTransport::Sse => "sse",
    }
}
