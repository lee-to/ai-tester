use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::McpServerConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub scenario: String,
    pub description: Option<String>,
    pub skill: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    pub argument: Option<String>,
    pub user_prompt: Option<String>,
    pub user_prompts: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    #[serde(default, alias = "token-budget")]
    pub token_budget: Option<f64>,
    #[serde(default)]
    pub runner: Runner,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub fixtures: Fixtures,
    #[serde(default)]
    pub user_responses: Vec<UserResponse>,
    #[serde(default)]
    pub assertions: Vec<AssertionSpec>,
}

impl Scenario {
    pub fn from_yaml_str(input: &str) -> anyhow::Result<Self> {
        let scenario: Scenario = yaml_serde::from_str(input)?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.scenario.trim().is_empty() {
            bail!("scenario must not be empty");
        }
        let prompt_sources = [
            self.skill.as_ref(),
            self.system_prompt.as_ref(),
            self.system_prompt_file.as_ref(),
        ]
        .iter()
        .filter(|v| v.is_some())
        .count();
        if prompt_sources != 1 {
            bail!("scenario must declare exactly one of: `skill`, `system_prompt`, or `system_prompt_file`");
        }
        if self.user_prompt.is_some() && self.user_prompts.is_some() {
            bail!("scenario must declare at most one of: `user_prompt` or `user_prompts`");
        }
        if let Some(turns) = self.max_turns {
            if turns == 0 {
                bail!("max_turns must be positive");
            }
        }
        if let Some(budget) = self.token_budget {
            if !budget.is_finite() || budget <= 0.0 {
                bail!("token_budget must be positive");
            }
        }
        if self.fixtures.setup_timeout_seconds == Some(0) {
            bail!("fixtures.setup_timeout_seconds must be positive");
        }
        if self.runner.acp_turn_timeout_seconds == Some(0) {
            bail!("runner.acp_turn_timeout_seconds must be positive");
        }
        if !self.fixtures.git_init {
            if self.fixtures.git_branch.is_some() {
                bail!("fixtures.git_branch requires fixtures.git_init: true");
            }
            if !self.fixtures.files_staged.is_empty() {
                bail!("fixtures.files_staged requires fixtures.git_init: true");
            }
        }
        for file in self
            .fixtures
            .files_committed
            .iter()
            .chain(self.fixtures.files_staged.iter())
            .chain(self.fixtures.files_unstaged.iter())
        {
            file.validate()?;
        }
        let mut assertion_ids = BTreeSet::new();
        for (index, assertion) in self.assertions.iter().enumerate() {
            let id = assertion.id();
            if id.trim().is_empty() {
                bail!("assertions[{index}].id must not be empty");
            }
            if !assertion_ids.insert(id) {
                bail!("assertions[].id must be unique: '{id}'");
            }
            let weight = assertion.weight();
            if !weight.is_finite() || weight <= 0.0 {
                bail!("assertions[{index}].weight must be finite and positive");
            }
            validate_assertion_shape(index, assertion)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LoadedScenario {
    pub scenario: Scenario,
    pub file_path: PathBuf,
    pub source_meta: ScenarioSourceMeta,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScenarioSourceMeta {
    pub runner_runtime_set: bool,
    pub runner_model_set: bool,
    pub runner_mode_set: bool,
    pub runner_reasoning_set: bool,
    pub runner_permission_mode_set: bool,
    pub runner_agent_set: bool,
    pub runner_mcp_profile_set: bool,
}

pub fn load_scenario_file(path: impl AsRef<Path>) -> anyhow::Result<LoadedScenario> {
    let file_path = path.as_ref().canonicalize()?;
    let raw = fs::read_to_string(&file_path)?;
    let source_meta = scenario_source_meta(&raw);
    let mut scenario = Scenario::from_yaml_str(&raw)?;
    materialize_fixtures(&mut scenario, file_path.parent().unwrap_or(Path::new(".")))?;
    Ok(LoadedScenario {
        scenario,
        file_path,
        source_meta,
    })
}

fn scenario_source_meta(raw: &str) -> ScenarioSourceMeta {
    let value = yaml_serde::from_str::<Value>(raw).unwrap_or(Value::Null);
    let runner = value.get("runner");
    ScenarioSourceMeta {
        runner_runtime_set: runner
            .and_then(|runner| runner.get("runtime"))
            .is_some_and(|value| !value.is_null()),
        runner_model_set: runner
            .and_then(|runner| runner.get("model"))
            .is_some_and(|value| !value.is_null()),
        runner_mode_set: runner
            .and_then(|runner| runner.get("mode"))
            .is_some_and(|value| !value.is_null()),
        runner_reasoning_set: runner
            .and_then(|runner| runner.get("reasoning"))
            .is_some_and(|value| !value.is_null()),
        runner_permission_mode_set: runner
            .and_then(|runner| runner.get("permission_mode"))
            .is_some_and(|value| !value.is_null()),
        runner_agent_set: runner
            .and_then(|runner| runner.get("agent"))
            .is_some_and(|value| !value.is_null()),
        runner_mcp_profile_set: runner
            .and_then(|runner| runner.get("mcp_profile"))
            .is_some_and(|value| !value.is_null()),
    }
}

pub fn materialize_fixtures(scenario: &mut Scenario, scenario_dir: &Path) -> anyhow::Result<()> {
    for file in scenario
        .fixtures
        .files_committed
        .iter_mut()
        .chain(scenario.fixtures.files_staged.iter_mut())
        .chain(scenario.fixtures.files_unstaged.iter_mut())
    {
        if let Some(content_from) = &file.content_from {
            let path = scenario_dir.join(content_from);
            file.content = Some(fs::read_to_string(&path).map_err(|err| {
                anyhow!(
                    "fixture content_from unreadable: {} - {err}",
                    path.display()
                )
            })?);
        } else if file.content.is_none() {
            file.content = Some(String::new());
        }
    }
    for tree in &mut scenario.fixtures.copy_trees {
        let from = scenario_dir.join(&tree.from);
        if !from.is_dir() {
            bail!("copy_trees.from must be a directory: {}", from.display());
        }
        tree.from = from.to_string_lossy().to_string();
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_model")]
    pub model: String,
    pub mode: Option<String>,
    pub reasoning: Option<String>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    pub agent: Option<String>,
    pub mcp_profile: Option<String>,
    pub acp_turn_timeout_seconds: Option<u64>,
    pub allowed_tools_override: Option<Vec<String>>,
    pub setting_sources: Option<Vec<String>>,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            runtime: default_runtime(),
            model: default_model(),
            mode: None,
            reasoning: None,
            permission_mode: default_permission_mode(),
            agent: None,
            mcp_profile: None,
            acp_turn_timeout_seconds: None,
            allowed_tools_override: None,
            setting_sources: None,
        }
    }
}

fn default_runtime() -> String {
    "claude".to_string()
}

fn default_model() -> String {
    crate::config::DEFAULT_MODEL.to_string()
}

fn default_permission_mode() -> String {
    "bypassPermissions".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Fixtures {
    #[serde(default)]
    pub git_init: bool,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub copy_trees: Vec<CopyTree>,
    #[serde(default)]
    pub files_committed: Vec<FixtureFile>,
    #[serde(default)]
    pub files_staged: Vec<FixtureFile>,
    #[serde(default)]
    pub files_unstaged: Vec<FixtureFile>,
    #[serde(default)]
    pub setup_commands: Vec<String>,
    pub setup_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyTree {
    pub from: String,
    #[serde(default = "default_copy_tree_dest")]
    pub to: String,
}

fn default_copy_tree_dest() -> String {
    ".".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureFile {
    pub path: String,
    pub content: Option<String>,
    pub content_from: Option<String>,
}

impl FixtureFile {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.content.is_some() && self.content_from.is_some() {
            bail!("fixture file must declare `content` or `content_from`, not both");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserResponse {
    pub match_question: String,
    pub choose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssertionSpec {
    ToolCalled {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        tool: Option<String>,
        tool_pattern: Option<String>,
        tool_kind: Option<String>,
        title_pattern: Option<String>,
        args_match: Option<BTreeMap<String, String>>,
        raw_input_match: Option<BTreeMap<String, String>>,
        call_index: Option<usize>,
        capture: Option<Vec<String>>,
        capture_max_chars: Option<usize>,
    },
    ToolCallSequence {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        sequence: Vec<SequenceStep>,
        capture_max_chars: Option<usize>,
    },
    NoToolCalled {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        tool: Option<String>,
        tool_pattern: Option<String>,
        tool_kind: Option<String>,
        title_pattern: Option<String>,
        args_match: Option<BTreeMap<String, String>>,
        raw_input_match: Option<BTreeMap<String, String>>,
    },
    OutputContains {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        pattern: String,
    },
    NoOutputContains {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        pattern: String,
    },
    FileContains {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
        pattern: String,
    },
    FileNotContains {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
        pattern: String,
    },
    FileEquals {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
        content: String,
    },
    FileExists {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
    },
    FileNotExists {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
    },
    JsonValid {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
    },
    JsonPathEquals {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
        json_path: String,
        value: Value,
    },
    CommandSucceeds {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        command: String,
        timeout_seconds: Option<u64>,
    },
    CommandOutputContains {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        command: String,
        pattern: String,
        timeout_seconds: Option<u64>,
    },
    FileRead {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        path: String,
    },
    TurnCountAtMost {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        max: u32,
    },
    NoPathEscape {
        id: String,
        #[serde(default = "default_weight")]
        weight: f64,
        tools: Option<Vec<String>>,
        allow_outside: Option<Vec<String>>,
    },
}

impl AssertionSpec {
    pub fn id(&self) -> &str {
        match self {
            Self::ToolCalled { id, .. }
            | Self::ToolCallSequence { id, .. }
            | Self::NoToolCalled { id, .. }
            | Self::OutputContains { id, .. }
            | Self::NoOutputContains { id, .. }
            | Self::FileContains { id, .. }
            | Self::FileNotContains { id, .. }
            | Self::FileEquals { id, .. }
            | Self::FileExists { id, .. }
            | Self::FileNotExists { id, .. }
            | Self::JsonValid { id, .. }
            | Self::JsonPathEquals { id, .. }
            | Self::CommandSucceeds { id, .. }
            | Self::CommandOutputContains { id, .. }
            | Self::FileRead { id, .. }
            | Self::TurnCountAtMost { id, .. }
            | Self::NoPathEscape { id, .. } => id,
        }
    }

    pub fn weight(&self) -> f64 {
        match self {
            Self::ToolCalled { weight, .. }
            | Self::ToolCallSequence { weight, .. }
            | Self::NoToolCalled { weight, .. }
            | Self::OutputContains { weight, .. }
            | Self::NoOutputContains { weight, .. }
            | Self::FileContains { weight, .. }
            | Self::FileNotContains { weight, .. }
            | Self::FileEquals { weight, .. }
            | Self::FileExists { weight, .. }
            | Self::FileNotExists { weight, .. }
            | Self::JsonValid { weight, .. }
            | Self::JsonPathEquals { weight, .. }
            | Self::CommandSucceeds { weight, .. }
            | Self::CommandOutputContains { weight, .. }
            | Self::FileRead { weight, .. }
            | Self::TurnCountAtMost { weight, .. }
            | Self::NoPathEscape { weight, .. } => *weight,
        }
    }

    pub fn tool_called(id: &str, tool: &str, args_match: Value) -> Self {
        Self::ToolCalled {
            id: id.to_string(),
            weight: 1.0,
            tool: Some(tool.to_string()),
            tool_pattern: None,
            tool_kind: None,
            title_pattern: None,
            args_match: json_object_to_regex_map(args_match),
            raw_input_match: None,
            call_index: None,
            capture: None,
            capture_max_chars: None,
        }
    }

    pub fn no_tool_called(id: &str, tool: &str) -> Self {
        Self::NoToolCalled {
            id: id.to_string(),
            weight: 1.0,
            tool: Some(tool.to_string()),
            tool_pattern: None,
            tool_kind: None,
            title_pattern: None,
            args_match: None,
            raw_input_match: None,
        }
    }

    pub fn output_contains(id: &str, pattern: &str) -> Self {
        Self::OutputContains {
            id: id.to_string(),
            weight: 1.0,
            pattern: pattern.to_string(),
        }
    }

    pub fn no_output_contains(id: &str, pattern: &str) -> Self {
        Self::NoOutputContains {
            id: id.to_string(),
            weight: 1.0,
            pattern: pattern.to_string(),
        }
    }

    pub fn turn_count_at_most(id: &str, max: u32) -> Self {
        Self::TurnCountAtMost {
            id: id.to_string(),
            weight: 1.0,
            max,
        }
    }

    pub fn file_read(id: &str, path: &str) -> Self {
        Self::FileRead {
            id: id.to_string(),
            weight: 1.0,
            path: path.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceStep {
    pub tool: Option<String>,
    pub tool_kind: Option<String>,
    pub title_pattern: Option<String>,
    pub args_match: Option<BTreeMap<String, String>>,
    pub raw_input_match: Option<BTreeMap<String, String>>,
    pub capture: Option<Vec<String>>,
}

fn default_weight() -> f64 {
    1.0
}

fn json_object_to_regex_map(value: Value) -> Option<BTreeMap<String, String>> {
    let Value::Object(map) = value else {
        return None;
    };
    Some(
        map.into_iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                (k, s)
            })
            .collect(),
    )
}

pub fn validate_assertion_shape(index: usize, spec: &AssertionSpec) -> anyhow::Result<()> {
    match spec {
        AssertionSpec::ToolCalled {
            tool,
            tool_pattern,
            tool_kind,
            ..
        } if primary_selector_count(tool, tool_pattern, tool_kind) != 1 => Err(anyhow!(
            "tool_called assertion must declare exactly one of `tool`, `tool_pattern`, or `tool_kind`"
        )),
        AssertionSpec::ToolCallSequence { sequence, .. } if sequence.is_empty() => Err(anyhow!(
            "tool_call_sequence assertion must declare at least one step"
        )),
        AssertionSpec::ToolCallSequence { sequence, .. }
            if sequence.iter().any(|step| {
                step.tool.is_some() == step.tool_kind.is_some()
            }) =>
        {
            Err(anyhow!(
                "tool_call_sequence steps must declare exactly one of `tool` or `tool_kind`"
            ))
        }
        AssertionSpec::NoToolCalled {
            tool,
            tool_pattern,
            tool_kind,
            ..
        } if primary_selector_count(tool, tool_pattern, tool_kind) != 1 => Err(anyhow!(
            "no_tool_called assertion must declare exactly one of `tool`, `tool_pattern`, or `tool_kind`"
        )),
        AssertionSpec::FileContains { path, pattern, .. }
        | AssertionSpec::FileNotContains { path, pattern, .. } => {
            if path.trim().is_empty() {
                Err(anyhow!("file content assertion must declare non-empty `path`"))
            } else if pattern.trim().is_empty() {
                Err(anyhow!(
                    "file content assertion must declare non-empty `pattern`"
                ))
            } else {
                Ok(())
            }
        }
        AssertionSpec::FileEquals { path, .. }
        | AssertionSpec::FileExists { path, .. }
        | AssertionSpec::FileNotExists { path, .. }
        | AssertionSpec::JsonValid { path, .. }
        | AssertionSpec::JsonPathEquals { path, .. } if path.trim().is_empty() => {
            Err(anyhow!("file assertion must declare non-empty `path`"))
        }
        AssertionSpec::JsonPathEquals { json_path, .. } if json_path.trim().is_empty() => {
            Err(anyhow!(
                "json_path_equals assertion must declare non-empty `json_path`"
            ))
        }
        AssertionSpec::CommandSucceeds { command, .. }
        | AssertionSpec::CommandOutputContains { command, .. }
            if command.trim().is_empty() =>
        {
            Err(anyhow!("command assertion must declare non-empty `command`"))
        }
        AssertionSpec::CommandOutputContains { pattern, .. } if pattern.trim().is_empty() => {
            Err(anyhow!(
                "command_output_contains assertion must declare non-empty `pattern`"
            ))
        }
        AssertionSpec::CommandSucceeds {
            timeout_seconds: Some(0),
            ..
        }
        | AssertionSpec::CommandOutputContains {
            timeout_seconds: Some(0),
            ..
        } => Err(anyhow!(
            "command assertion timeout_seconds must be positive"
        )),
        AssertionSpec::FileRead { path, .. } if path.trim().is_empty() => Err(anyhow!(
            "file_read assertion must declare non-empty `path` regex"
        )),
        AssertionSpec::TurnCountAtMost { max, .. } if *max == 0 => {
            Err(anyhow!("assertions[{index}].max must be positive"))
        }
        _ => Ok(()),
    }
}

fn primary_selector_count(
    tool: &Option<String>,
    tool_pattern: &Option<String>,
    tool_kind: &Option<String>,
) -> usize {
    usize::from(tool.is_some())
        + usize::from(tool_pattern.is_some())
        + usize::from(tool_kind.is_some())
}
