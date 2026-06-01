use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::{
    AgentCapabilities, ClientCapabilities, CloseSessionRequest, ContentBlock,
    CreateTerminalRequest, EnvVariable, FileSystemCapabilities, HttpHeader, InitializeRequest,
    KillTerminalRequest, McpServer, McpServerHttp, McpServerSse, McpServerStdio, NewSessionRequest,
    ProtocolVersion, ReadTextFileRequest, ReleaseTerminalRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigSelectOptions,
    SessionConfigValueId, SessionModeId, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TerminalOutputRequest, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolKind, WaitForTerminalExitRequest, WriteTextFileRequest,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo, SessionMessage};
use anyhow::Context;
use serde_json::{Map, Value};

use crate::config::{
    mcp_servers_diagnostic, AcpAgentConfig, McpServerTransport, NamedMcpServerConfig,
};
use crate::scenario::{Runner, UserResponse};
use crate::trace::{ToolCallRecord, TraceCost, Turn};
use crate::ui::{self, Tone};
use crate::util::regex::compile_pattern;

use super::{RuntimeRunRequest, RuntimeRunResult};

mod client_capabilities;

use client_capabilities::AcpClientBridge;

pub fn run_acp(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_acp_async(req))
}

async fn run_acp_async(req: RuntimeRunRequest) -> anyhow::Result<RuntimeRunResult> {
    let max_turns = req
        .scenario
        .max_turns
        .unwrap_or(crate::config::INTERNAL_MAX_TURNS);
    let max_turns_user_set = req.scenario.max_turns.is_some();
    let agent_name = req
        .acp_agent_name
        .clone()
        .context("runtime `acp` requires `runner.agent`, `defaults.agent`, or `--agent`")?;
    let agent_config = req
        .acp_agent
        .clone()
        .with_context(|| format!("unknown ACP agent `{agent_name}`"))?;
    let acp_agent = build_acp_agent(&agent_config)?;
    let process_cwd = req.cwd.clone();
    let session_cwd = acp_session_cwd(&process_cwd);
    let user_messages = build_acp_user_messages(&req);
    let idle_timeout = Duration::from_secs(req.idle_warn_seconds.max(1));
    let client_bridge = Arc::new(AcpClientBridge::new(
        process_cwd.clone(),
        terminal_wait_timeout(idle_timeout),
    )?);
    let policy = Arc::new(PermissionPolicy::from_runner(
        &req.scenario.runner,
        &req.allowed_tools,
        &req.user_responses,
    ));
    let permission_diagnostics = Arc::new(Mutex::new(Vec::<String>::new()));
    let permission_diagnostics_for_handler = Arc::clone(&permission_diagnostics);
    let policy_for_handler = Arc::clone(&policy);
    let bridge_for_read = Arc::clone(&client_bridge);
    let bridge_for_write = Arc::clone(&client_bridge);
    let bridge_for_terminal_create = Arc::clone(&client_bridge);
    let bridge_for_terminal_output = Arc::clone(&client_bridge);
    let bridge_for_terminal_wait = Arc::clone(&client_bridge);
    let bridge_for_terminal_kill = Arc::clone(&client_bridge);
    let bridge_for_terminal_release = Arc::clone(&client_bridge);
    let bridge_for_connection = Arc::clone(&client_bridge);

    let _cwd_guard = CurrentDirGuard::push(&process_cwd)?;
    let mut result = Client
        .builder()
        .name("ai-tester")
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let decision = policy_for_handler.choose(&request);
                if let Ok(mut diagnostics) = permission_diagnostics_for_handler.lock() {
                    diagnostics.push(decision.diagnostic);
                }
                responder.respond(RequestPermissionResponse::new(decision.outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ReadTextFileRequest, responder, _connection| match bridge_for_read
                .handle_read_text_file(request)
            {
                Ok(response) => responder.respond(response),
                Err(err) => responder.respond_with_error(err),
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WriteTextFileRequest, responder, _connection| {
                match bridge_for_write.handle_write_text_file(request) {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CreateTerminalRequest, responder, _connection| {
                match bridge_for_terminal_create.handle_create_terminal(request) {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: TerminalOutputRequest, responder, _connection| {
                match bridge_for_terminal_output.handle_terminal_output(request) {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WaitForTerminalExitRequest, responder, _connection| {
                match bridge_for_terminal_wait
                    .handle_wait_for_terminal_exit(request)
                    .await
                {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: KillTerminalRequest, responder, _connection| {
                match bridge_for_terminal_kill.handle_kill_terminal(request) {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ReleaseTerminalRequest, responder, _connection| {
                match bridge_for_terminal_release.handle_release_terminal(request) {
                    Ok(response) => responder.respond(response),
                    Err(err) => responder.respond_with_error(err),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            acp_agent,
            move |connection: ConnectionTo<Agent>| async move {
                let client_bridge = Arc::clone(&bridge_for_connection);
                let initialize_response = tokio::time::timeout(
                    idle_timeout,
                    connection
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::V1)
                                .client_capabilities(acp_client_capabilities()),
                        )
                        .block_task(),
                )
                .await
                .map_err(|_| acp_timeout_error("initialize", idle_timeout))??;
                let mcp_servers = build_acp_mcp_servers(
                    &req.mcp_servers,
                    &initialize_response.agent_capabilities,
                )
                .map_err(|err| agent_client_protocol::Error::new(-32602, err.to_string()))?;
                let mcp_diagnostic =
                    (!req.mcp_servers.is_empty()).then(|| mcp_servers_diagnostic(&req.mcp_servers));

                let session_request =
                    NewSessionRequest::new(session_cwd.clone()).mcp_servers(mcp_servers);
                let session_response = tokio::time::timeout(
                    idle_timeout,
                    connection
                        .send_request_to(Agent, session_request)
                        .block_task(),
                )
                .await
                .map_err(|_| acp_timeout_error("session/new", idle_timeout))??;
                let acp_config_diagnostic = negotiate_acp_config(
                    &connection,
                    &session_response.session_id,
                    session_response.config_options.clone().unwrap_or_default(),
                    session_response.modes.clone(),
                    &req.acp_config,
                    idle_timeout,
                )
                .await?;
                let mut session = connection.attach_session(session_response, Vec::new())?;

                let mut out = RuntimeRunResult::new(max_turns, max_turns_user_set);
                out.cost = TraceCost {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    usd_estimate: 0.0,
                    source: "acp".to_string(),
                };
                out.session_id = Some(session.session_id().to_string());
                if let Some(diagnostic) = mcp_diagnostic {
                    out.diagnostics.push(diagnostic);
                }
                if let Some(diagnostic) = acp_config_diagnostic {
                    out.diagnostics.push(diagnostic);
                }
                client_bridge.set_session_id(session.session_id().to_string());
                let mut progress = req.progress.then(AcpProgress::new);

                let user_message_count = user_messages.len();
                for (message_index, user_message) in user_messages.into_iter().enumerate() {
                    if out.turns_used >= max_turns {
                        out.stopped_reason = "max_turns".to_string();
                        break;
                    }

                    let turn_index = out.turns.len();
                    out.turns.push(Turn {
                        index: turn_index,
                        role: "assistant".to_string(),
                        text_deltas: Vec::new(),
                        tool_calls: Vec::new(),
                        usage: None,
                    });
                    out.turns_used += 1;

                    session.send_prompt(user_message)?;
                    loop {
                        let message = tokio::time::timeout(idle_timeout, session.read_update())
                            .await
                            .map_err(|_| acp_timeout_error("session update", idle_timeout))??;
                        match message {
                            SessionMessage::SessionMessage(dispatch) => {
                                MatchDispatch::new(dispatch)
                                    .if_notification(async |notification: SessionNotification| {
                                        if let Some(progress) = &mut progress {
                                            progress.print_update(&notification.update);
                                        }
                                        apply_session_update(notification.update, &mut out);
                                        flush_bridge_tool_calls(&client_bridge, &mut out);
                                        Ok(())
                                    })
                                    .await
                                    .otherwise_ignore()?;
                            }
                            SessionMessage::StopReason(reason) => {
                                out.stopped_reason = stop_reason_to_string(reason);
                                flush_bridge_tool_calls(&client_bridge, &mut out);
                                if let Some(progress) = &mut progress {
                                    progress.stop_reason(&out.stopped_reason);
                                }
                                break;
                            }
                            _ => {}
                        }
                    }

                    if out.turns_used >= max_turns && message_index + 1 < user_message_count {
                        out.stopped_reason = "max_turns".to_string();
                        break;
                    }
                }

                let close_request = session
                    .connection()
                    .send_request_to(
                        Agent,
                        CloseSessionRequest::new(session.session_id().clone()),
                    )
                    .block_task();
                let _ = tokio::time::timeout(idle_timeout, close_request).await;
                flush_bridge_tool_calls(&client_bridge, &mut out);

                if out.stopped_reason == "other" && out.errors.is_empty() {
                    out.stopped_reason = "end_turn".to_string();
                }
                if let Some(mut progress) = progress {
                    progress.finish();
                }
                Ok(out)
            },
        )
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?;

    if let Ok(diagnostics) = permission_diagnostics.lock() {
        result.diagnostics.extend(diagnostics.iter().cloned());
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpConfigTarget {
    Mode,
    Model,
    Reasoning,
}

impl AcpConfigTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Model => "model",
            Self::Reasoning => "reasoning",
        }
    }

    fn category(self) -> SessionConfigOptionCategory {
        match self {
            Self::Mode => SessionConfigOptionCategory::Mode,
            Self::Model => SessionConfigOptionCategory::Model,
            Self::Reasoning => SessionConfigOptionCategory::ThoughtLevel,
        }
    }

    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Mode => &["mode"],
            Self::Model => &["model"],
            Self::Reasoning => &["reasoning", "reason", "thought", "thought_level"],
        }
    }
}

#[derive(Debug, Clone)]
struct AcpConfigSelection {
    config_id: SessionConfigId,
    value_id: SessionConfigValueId,
    current_value_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpConfigSelectionErrorKind {
    NoOption,
    UnsupportedValue,
    AmbiguousOption,
}

#[derive(Debug, Clone)]
struct AcpConfigSelectionError {
    kind: AcpConfigSelectionErrorKind,
    message: String,
}

impl std::fmt::Display for AcpConfigSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AcpConfigSelectionError {}

#[derive(Debug, Clone)]
struct AcpAppliedConfig {
    target: AcpConfigTarget,
    requested: String,
    applied: Option<String>,
    status: String,
    method: Option<&'static str>,
    config_id: Option<String>,
    mode_id: Option<String>,
}

async fn negotiate_acp_config(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    mut config_options: Vec<SessionConfigOption>,
    modes: Option<SessionModeState>,
    request: &super::AcpConfigRequest,
    idle_timeout: Duration,
) -> Result<Option<String>, agent_client_protocol::Error> {
    let mut applied = Vec::new();

    if let Some(requested) = request.mode.as_deref() {
        let entry = negotiate_acp_mode(
            connection,
            session_id,
            &mut config_options,
            modes.as_ref(),
            requested,
            idle_timeout,
        )
        .await?;
        applied.push(entry);
    }

    if let Some(requested) = request.model.as_deref() {
        let entry = negotiate_acp_config_option(
            connection,
            session_id,
            &mut config_options,
            AcpConfigTarget::Model,
            requested,
            idle_timeout,
        )
        .await?;
        applied.push(entry);
    }

    if let Some(requested) = request.reasoning.as_deref() {
        let entry = negotiate_acp_config_option(
            connection,
            session_id,
            &mut config_options,
            AcpConfigTarget::Reasoning,
            requested,
            idle_timeout,
        )
        .await?;
        applied.push(entry);
    }

    Ok(acp_effective_config_diagnostic(&applied))
}

async fn negotiate_acp_mode(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    config_options: &mut Vec<SessionConfigOption>,
    modes: Option<&SessionModeState>,
    requested: &str,
    idle_timeout: Duration,
) -> Result<AcpAppliedConfig, agent_client_protocol::Error> {
    match resolve_acp_config_selection(config_options, AcpConfigTarget::Mode, requested) {
        Ok(selection) => {
            apply_acp_config_selection(
                connection,
                session_id,
                config_options,
                AcpConfigTarget::Mode,
                requested,
                selection,
                idle_timeout,
            )
            .await
        }
        Err(config_err)
            if matches!(
                config_err.kind,
                AcpConfigSelectionErrorKind::NoOption
                    | AcpConfigSelectionErrorKind::UnsupportedValue
            ) =>
        {
            if let Some(modes) = modes {
                match resolve_acp_session_mode(modes, requested) {
                    Ok(mode_id) => {
                        let current_mode_id = modes.current_mode_id.to_string();
                        if current_mode_id != mode_id.to_string() {
                            let response = tokio::time::timeout(
                                idle_timeout,
                                connection
                                    .send_request_to(
                                        Agent,
                                        SetSessionModeRequest::new(
                                            session_id.clone(),
                                            mode_id.clone(),
                                        ),
                                    )
                                    .block_task(),
                            )
                            .await
                            .map_err(|_| acp_timeout_error("session/set_mode", idle_timeout))??;
                            let _: SetSessionModeResponse = response;
                        }
                        Ok(AcpAppliedConfig {
                            target: AcpConfigTarget::Mode,
                            requested: requested.to_string(),
                            applied: Some(mode_id.to_string()),
                            status: if current_mode_id == mode_id.to_string() {
                                "already_current".to_string()
                            } else {
                                "applied".to_string()
                            },
                            method: (current_mode_id != mode_id.to_string())
                                .then_some("session/set_mode"),
                            config_id: None,
                            mode_id: Some(mode_id.to_string()),
                        })
                    }
                    Err(mode_err) => Err(acp_invalid_params_error(format!(
                        "{config_err}; ACP mode fallback also failed: {mode_err}"
                    ))),
                }
            } else {
                Err(acp_invalid_params_error(config_err))
            }
        }
        Err(config_err) => Err(acp_invalid_params_error(config_err)),
    }
}

async fn negotiate_acp_config_option(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    config_options: &mut Vec<SessionConfigOption>,
    target: AcpConfigTarget,
    requested: &str,
    idle_timeout: Duration,
) -> Result<AcpAppliedConfig, agent_client_protocol::Error> {
    let selection = resolve_acp_config_selection(config_options, target, requested)
        .map_err(acp_invalid_params_error)?;
    apply_acp_config_selection(
        connection,
        session_id,
        config_options,
        target,
        requested,
        selection,
        idle_timeout,
    )
    .await
}

async fn apply_acp_config_selection(
    connection: &ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::SessionId,
    config_options: &mut Vec<SessionConfigOption>,
    target: AcpConfigTarget,
    requested: &str,
    selection: AcpConfigSelection,
    idle_timeout: Duration,
) -> Result<AcpAppliedConfig, agent_client_protocol::Error> {
    let config_id = selection.config_id.to_string();
    let value_id = selection.value_id.to_string();
    let is_current = selection.current_value_id == value_id;
    if !is_current {
        let response: SetSessionConfigOptionResponse = tokio::time::timeout(
            idle_timeout,
            connection
                .send_request_to(
                    Agent,
                    SetSessionConfigOptionRequest::new(
                        session_id.clone(),
                        selection.config_id,
                        selection.value_id,
                    ),
                )
                .block_task(),
        )
        .await
        .map_err(|_| acp_timeout_error("session/set_config_option", idle_timeout))??;
        *config_options = response.config_options;
    }

    Ok(AcpAppliedConfig {
        target,
        requested: requested.to_string(),
        applied: Some(value_id),
        status: if is_current {
            "already_current".to_string()
        } else {
            "applied".to_string()
        },
        method: (!is_current).then_some("session/set_config_option"),
        config_id: Some(config_id),
        mode_id: None,
    })
}

fn resolve_acp_config_selection(
    config_options: &[SessionConfigOption],
    target: AcpConfigTarget,
    requested: &str,
) -> Result<AcpConfigSelection, AcpConfigSelectionError> {
    let category_candidates = config_options
        .iter()
        .filter(|option| option.category.as_ref() == Some(&target.category()))
        .filter(|option| select_payload(option).is_some())
        .collect::<Vec<_>>();

    if !category_candidates.is_empty() {
        return select_single_config_candidate(target, requested, category_candidates, false);
    }

    let fallback_candidates = config_options
        .iter()
        .filter(|option| select_payload(option).is_some())
        .filter(|option| config_option_matches_alias(option, target))
        .collect::<Vec<_>>();
    select_single_config_candidate(target, requested, fallback_candidates, true)
}

fn select_single_config_candidate(
    target: AcpConfigTarget,
    requested: &str,
    candidates: Vec<&SessionConfigOption>,
    from_fallback: bool,
) -> Result<AcpConfigSelection, AcpConfigSelectionError> {
    match candidates.as_slice() {
        [] => Err(AcpConfigSelectionError {
            kind: AcpConfigSelectionErrorKind::NoOption,
            message: format!("ACP agent does not expose {} selection", target.label()),
        }),
        [option] => select_config_value(option, target, requested),
        _ => {
            let ids = candidates
                .iter()
                .map(|option| option.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let source = if from_fallback { " from fallback" } else { "" };
            Err(AcpConfigSelectionError {
                kind: AcpConfigSelectionErrorKind::AmbiguousOption,
                message: format!(
                    "ambiguous ACP {} config option{source} for `{requested}`: {ids}",
                    target.label()
                ),
            })
        }
    }
}

fn select_config_value(
    option: &SessionConfigOption,
    target: AcpConfigTarget,
    requested: &str,
) -> Result<AcpConfigSelection, AcpConfigSelectionError> {
    let select = select_payload(option).expect("candidate is select");
    let choices = flatten_select_options(select);
    let selected =
        find_named_value(&choices, requested).ok_or_else(|| AcpConfigSelectionError {
            kind: AcpConfigSelectionErrorKind::UnsupportedValue,
            message: format!(
                "unsupported ACP {} `{requested}`; supported values: {}",
                target.label(),
                supported_config_values(&choices)
            ),
        })?;

    Ok(AcpConfigSelection {
        config_id: option.id.clone(),
        value_id: selected.value.clone(),
        current_value_id: select.current_value.to_string(),
    })
}

fn resolve_acp_session_mode(
    modes: &SessionModeState,
    requested: &str,
) -> Result<SessionModeId, AcpConfigSelectionError> {
    let options = modes
        .available_modes
        .iter()
        .map(|mode| NamedValue {
            value: mode.id.to_string(),
            name: mode.name.clone(),
        })
        .collect::<Vec<_>>();
    find_named_value_ref(&options, requested)
        .map(|matched| SessionModeId::new(matched.value.clone()))
        .ok_or_else(|| AcpConfigSelectionError {
            kind: AcpConfigSelectionErrorKind::UnsupportedValue,
            message: format!(
                "unsupported ACP mode `{requested}`; supported values: {}",
                supported_named_values(&options)
            ),
        })
}

fn select_payload(option: &SessionConfigOption) -> Option<&SessionConfigSelect> {
    match &option.kind {
        SessionConfigKind::Select(select) => Some(select),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

fn flatten_select_options(select: &SessionConfigSelect) -> Vec<NamedConfigValue<'_>> {
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(NamedConfigValue::from_select_option)
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .map(NamedConfigValue::from_select_option)
            .collect(),
        #[allow(unreachable_patterns)]
        _ => Vec::new(),
    }
}

#[derive(Clone)]
struct NamedConfigValue<'a> {
    value: &'a SessionConfigValueId,
    value_text: String,
    name: &'a str,
}

impl<'a> NamedConfigValue<'a> {
    fn from_select_option(option: &'a SessionConfigSelectOption) -> Self {
        Self {
            value: &option.value,
            value_text: option.value.to_string(),
            name: &option.name,
        }
    }
}

#[derive(Debug, Clone)]
struct NamedValue {
    value: String,
    name: String,
}

fn find_named_value<'a>(
    choices: &'a [NamedConfigValue<'a>],
    requested: &str,
) -> Option<&'a NamedConfigValue<'a>> {
    choices
        .iter()
        .find(|choice| choice.value_text == requested)
        .or_else(|| choices.iter().find(|choice| choice.name == requested))
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.value_text.eq_ignore_ascii_case(requested))
        })
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.name.eq_ignore_ascii_case(requested))
        })
}

fn find_named_value_ref<'a>(choices: &'a [NamedValue], requested: &str) -> Option<&'a NamedValue> {
    choices
        .iter()
        .find(|choice| choice.value == requested)
        .or_else(|| choices.iter().find(|choice| choice.name == requested))
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.value.eq_ignore_ascii_case(requested))
        })
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.name.eq_ignore_ascii_case(requested))
        })
}

fn supported_config_values(choices: &[NamedConfigValue<'_>]) -> String {
    let values = choices
        .iter()
        .map(|choice| format!("{} ({})", choice.value_text, choice.name))
        .collect::<Vec<_>>();
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn supported_named_values(choices: &[NamedValue]) -> String {
    let values = choices
        .iter()
        .map(|choice| format!("{} ({})", choice.value, choice.name))
        .collect::<Vec<_>>();
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn config_option_matches_alias(option: &SessionConfigOption, target: AcpConfigTarget) -> bool {
    let labels = [option.id.to_string(), option.name.clone()];
    labels
        .iter()
        .any(|label| label_matches_alias(label, target.aliases()))
}

fn label_matches_alias(label: &str, aliases: &[&str]) -> bool {
    let tokens = label_tokens(label);
    let compact = label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>();
    aliases.iter().any(|alias| {
        let alias_compact = alias.replace('_', "");
        tokens.iter().any(|token| token == alias)
            || compact == alias_compact
            || compact.contains(&format!("{alias_compact}selector"))
    })
}

fn label_tokens(label: &str) -> Vec<String> {
    label
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

fn acp_effective_config_diagnostic(applied: &[AcpAppliedConfig]) -> Option<String> {
    if applied.is_empty() {
        return None;
    }
    let mut root = Map::new();
    for entry in applied {
        let mut value = Map::new();
        value.insert(
            "requested".to_string(),
            Value::String(entry.requested.clone()),
        );
        value.insert("status".to_string(), Value::String(entry.status.clone()));
        if let Some(applied) = &entry.applied {
            value.insert("applied".to_string(), Value::String(applied.clone()));
        }
        if let Some(method) = entry.method {
            value.insert("method".to_string(), Value::String(method.to_string()));
        }
        if let Some(config_id) = &entry.config_id {
            value.insert("configId".to_string(), Value::String(config_id.clone()));
        }
        if let Some(mode_id) = &entry.mode_id {
            value.insert("modeId".to_string(), Value::String(mode_id.clone()));
        }
        root.insert(entry.target.label().to_string(), Value::Object(value));
    }
    serde_json::to_string(&Value::Object(root))
        .ok()
        .map(|json| format!("ACP effective config: {json}"))
}

fn acp_invalid_params_error(message: impl std::fmt::Display) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(-32602, message.to_string())
}

struct AcpProgress {
    live: Option<super::LiveProgress>,
    active_tools: HashMap<String, String>,
}

impl AcpProgress {
    fn new() -> Self {
        Self {
            live: super::LiveProgress::new(),
            active_tools: HashMap::new(),
        }
    }

    fn print_update(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                let text = content_block_to_string(&chunk.content);
                if text.trim().is_empty() {
                    self.status("Assistant message");
                } else {
                    self.status(&format!(
                        "Assistant: {}",
                        super::truncate_progress_value(text.trim(), 120)
                    ));
                }
            }
            SessionUpdate::ToolCall(tool_call) => {
                let label = acp_tool_call_label(
                    &tool_call.title,
                    &tool_kind_to_string(tool_call.kind),
                    tool_call.raw_input.as_ref(),
                );
                self.active_tools
                    .insert(tool_call.tool_call_id.to_string(), label.clone());
                match tool_call.status {
                    ToolCallStatus::Completed => self.completed(&label, Tone::Success),
                    ToolCallStatus::Failed => self.completed(&label, Tone::Error),
                    _ => self.status(&label),
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let label = self
                    .active_tools
                    .entry(id.clone())
                    .or_insert_with(|| {
                        let title = update.fields.title.as_deref().unwrap_or("ACP tool call");
                        let kind = update
                            .fields
                            .kind
                            .map(tool_kind_to_string)
                            .unwrap_or_else(|| "tool".to_string());
                        acp_tool_call_label(title, &kind, update.fields.raw_input.as_ref())
                    })
                    .clone();
                match update.fields.status {
                    Some(ToolCallStatus::Completed) => {
                        self.active_tools.remove(&id);
                        self.completed(&label, Tone::Success);
                    }
                    Some(ToolCallStatus::Failed) => {
                        self.active_tools.remove(&id);
                        self.completed(&label, Tone::Error);
                    }
                    _ => self.status(&label),
                }
            }
            SessionUpdate::Plan(_) => self.status("Plan update"),
            _ => {}
        }
    }

    fn stop_reason(&mut self, reason: &str) {
        let tone = match reason {
            "end_turn" => Tone::Success,
            "cancelled" | "refusal" | "max_turns" => Tone::Warning,
            "error" => Tone::Error,
            _ => Tone::Muted,
        };
        self.completed(&format!("ACP stop: {reason}"), tone);
    }

    fn status(&mut self, text: &str) {
        if let Some(live) = &self.live {
            live.set_status(text);
        } else {
            println!("    {} {text}", ui::tag("acp", Tone::Info));
        }
    }

    fn completed(&mut self, text: &str, tone: Tone) {
        if let Some(live) = &self.live {
            live.print_completed(text, tone);
            live.set_status("Processing");
        } else {
            println!("    {} {text}", ui::tag("acp", tone));
        }
    }

    fn finish(&mut self) {
        if let Some(live) = &mut self.live {
            live.finish();
        }
    }
}

fn acp_session_cwd(cwd: &Path) -> PathBuf {
    crate::util::path::strip_windows_verbatim_prefix(cwd)
}

fn acp_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(true)
}

fn build_acp_mcp_servers(
    servers: &[NamedMcpServerConfig],
    capabilities: &AgentCapabilities,
) -> anyhow::Result<Vec<McpServer>> {
    servers
        .iter()
        .map(|server| build_acp_mcp_server(server, capabilities))
        .collect()
}

fn build_acp_mcp_server(
    server: &NamedMcpServerConfig,
    capabilities: &AgentCapabilities,
) -> anyhow::Result<McpServer> {
    match server.config.transport {
        McpServerTransport::Stdio => {
            let command = server
                .config
                .command
                .as_deref()
                .with_context(|| format!("MCP server `{}` requires `command`", server.name))?;
            let env = server
                .config
                .env
                .iter()
                .map(|(name, value)| EnvVariable::new(name.clone(), value.clone()))
                .collect::<Vec<_>>();
            Ok(McpServer::Stdio(
                McpServerStdio::new(server.name.clone(), command)
                    .args(server.config.args.clone())
                    .env(env),
            ))
        }
        McpServerTransport::Http => {
            if !capabilities.mcp_capabilities.http {
                anyhow::bail!(
                    "ACP agent does not advertise HTTP MCP support required by MCP server `{}`",
                    server.name
                );
            }
            let url = server
                .config
                .url
                .as_deref()
                .with_context(|| format!("MCP server `{}` requires `url`", server.name))?;
            Ok(McpServer::Http(
                McpServerHttp::new(server.name.clone(), url).headers(mcp_headers(&server.config)),
            ))
        }
        McpServerTransport::Sse => {
            if !capabilities.mcp_capabilities.sse {
                anyhow::bail!(
                    "ACP agent does not advertise SSE MCP support required by MCP server `{}`",
                    server.name
                );
            }
            let url = server
                .config
                .url
                .as_deref()
                .with_context(|| format!("MCP server `{}` requires `url`", server.name))?;
            Ok(McpServer::Sse(
                McpServerSse::new(server.name.clone(), url).headers(mcp_headers(&server.config)),
            ))
        }
    }
}

fn mcp_headers(config: &crate::config::McpServerConfig) -> Vec<HttpHeader> {
    config
        .headers
        .iter()
        .map(|(name, value)| HttpHeader::new(name.clone(), value.clone()))
        .collect()
}

fn terminal_wait_timeout(idle_timeout: Duration) -> Duration {
    let cushion = (idle_timeout / 2)
        .max(Duration::from_millis(500))
        .min(Duration::from_secs(5));
    if idle_timeout > cushion {
        idle_timeout - cushion
    } else {
        idle_timeout
    }
}

fn flush_bridge_tool_calls(bridge: &AcpClientBridge, out: &mut RuntimeRunResult) {
    let calls = bridge.drain_tool_calls();
    if calls.is_empty() {
        return;
    }
    current_turn_mut(out).tool_calls.extend(calls);
}

fn acp_timeout_error(stage: &str, idle_timeout: Duration) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(
        -32000,
        format!(
            "ACP agent did not produce `{stage}` protocol progress within {}s; verify the configured command starts an ACP stdio server",
            idle_timeout.as_secs()
        ),
    )
}

fn acp_tool_call_label(title: &str, kind: &str, raw_input: Option<&Value>) -> String {
    let detail = raw_input
        .and_then(|input| {
            input
                .get("command")
                .or_else(|| input.get("path"))
                .or_else(|| input.get("file_path"))
                .or_else(|| input.get("pattern"))
                .and_then(Value::as_str)
        })
        .map(|value| super::truncate_progress_value(value, 120))
        .or_else(|| {
            raw_input
                .filter(|input| !input.is_null())
                .map(|input| super::truncate_progress_value(&input.to_string(), 120))
        });
    let name = if title.trim().is_empty() { kind } else { title };
    match detail {
        Some(detail) if !detail.is_empty() => format!("{name}({detail})"),
        _ => name.to_string(),
    }
}

fn build_acp_agent(config: &AcpAgentConfig) -> anyhow::Result<AcpAgent> {
    let mut args = config
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    args.push(resolve_command_path(&config.command).unwrap_or_else(|| config.command.clone()));
    args.extend(config.args.clone());
    AcpAgent::from_args(args).map_err(|err| anyhow::anyhow!("{err}"))
}

fn resolve_command_path(command: &str) -> Option<String> {
    let path = Path::new(command);
    if path.components().count() > 1 || path.is_absolute() {
        return Some(command.to_string());
    }

    #[cfg(windows)]
    let output = std::process::Command::new("where")
        .arg(command)
        .output()
        .ok()?;
    #[cfg(not(windows))]
    let output = std::process::Command::new("which")
        .arg(command)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
}

fn build_acp_user_messages(req: &RuntimeRunRequest) -> Vec<String> {
    req.user_messages
        .iter()
        .enumerate()
        .map(|(idx, user_message)| {
            if idx == 0 {
                build_acp_input(
                    &req.skill_body,
                    req.skill_install_rel_path.as_deref(),
                    user_message,
                )
            } else {
                user_message.clone()
            }
        })
        .collect()
}

fn build_acp_input(
    skill_body: &str,
    skill_install_rel_path: Option<&str>,
    user_message: &str,
) -> String {
    let mut parts = vec![skill_body.to_string()];
    if let Some(path) = skill_install_rel_path {
        parts.push(format!(
            "---\n\n## Skill installation context (ai-tester)\n\nThis skill is installed at `{path}` relative to the current working directory."
        ));
    }
    parts.push(format!("---\n\n## User request\n\n{user_message}"));
    parts.join("\n\n")
}

fn apply_session_update(update: SessionUpdate, out: &mut RuntimeRunResult) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            let text = content_block_to_string(&chunk.content);
            out.final_output.push_str(&text);
            current_turn_mut(out).text_deltas.push(text);
        }
        SessionUpdate::ToolCall(tool_call) => {
            current_turn_mut(out)
                .tool_calls
                .push(normalize_tool_call(tool_call));
        }
        SessionUpdate::ToolCallUpdate(update) => merge_tool_call_update(update, out),
        SessionUpdate::Plan(plan) => out
            .diagnostics
            .push(format!("ACP plan update: {}", json_string(&plan))),
        _ => {}
    }
}

fn normalize_tool_call(tool_call: ToolCall) -> ToolCallRecord {
    let name = tool_kind_to_string(tool_call.kind);
    let mut input = raw_value_to_input(tool_call.raw_input);
    insert_meta(&mut input, "_acpTitle", Value::String(tool_call.title));
    insert_meta(&mut input, "_acpKind", Value::String(name.clone()));
    insert_meta(
        &mut input,
        "_acpStatus",
        Value::String(tool_status_to_string(tool_call.status)),
    );
    if !tool_call.locations.is_empty() {
        insert_meta(
            &mut input,
            "_acpLocations",
            serde_json::to_value(tool_call.locations).unwrap_or(Value::Null),
        );
    }
    if let Some(raw_output) = tool_call.raw_output {
        insert_meta(&mut input, "_acpRawOutput", raw_output.clone());
    }

    let result_content = tool_content_to_string(&tool_call.content)
        .or_else(|| input.get("_acpRawOutput").map(value_to_string));

    ToolCallRecord {
        id: tool_call.tool_call_id.to_string(),
        name,
        input,
        result_content,
        result_is_error: tool_call.status == ToolCallStatus::Failed,
        answered: None,
    }
}

fn merge_tool_call_update(update: ToolCallUpdate, out: &mut RuntimeRunResult) {
    let id = update.tool_call_id.to_string();
    if find_tool_call_mut(out, &id).is_none() {
        let name = update
            .fields
            .kind
            .map(tool_kind_to_string)
            .unwrap_or_else(|| "other".to_string());
        let mut input = raw_value_to_input(update.fields.raw_input.clone());
        insert_meta(
            &mut input,
            "_acpTitle",
            Value::String(
                update
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| "ACP tool call".to_string()),
            ),
        );
        insert_meta(&mut input, "_acpKind", Value::String(name.clone()));
        current_turn_mut(out).tool_calls.push(ToolCallRecord {
            id: id.clone(),
            name,
            input,
            result_content: None,
            result_is_error: false,
            answered: None,
        });
    }

    if let Some(call) = find_tool_call_mut(out, &id) {
        if let Some(kind) = update.fields.kind {
            call.name = tool_kind_to_string(kind);
            insert_meta(
                &mut call.input,
                "_acpKind",
                Value::String(call.name.clone()),
            );
        }
        if let Some(title) = update.fields.title {
            insert_meta(&mut call.input, "_acpTitle", Value::String(title));
        }
        if let Some(status) = update.fields.status {
            call.result_is_error = status == ToolCallStatus::Failed;
            insert_meta(
                &mut call.input,
                "_acpStatus",
                Value::String(tool_status_to_string(status)),
            );
        }
        if let Some(locations) = update.fields.locations {
            insert_meta(
                &mut call.input,
                "_acpLocations",
                serde_json::to_value(locations).unwrap_or(Value::Null),
            );
        }
        if let Some(raw_input) = update.fields.raw_input {
            merge_raw_input(&mut call.input, raw_input);
        }
        if let Some(raw_output) = update.fields.raw_output {
            call.result_content = Some(value_to_string(&raw_output));
            insert_meta(&mut call.input, "_acpRawOutput", raw_output);
        }
        if let Some(content) = update.fields.content {
            if let Some(text) = tool_content_to_string(&content) {
                call.result_content = Some(text);
            }
        }
    }
}

fn current_turn_mut(out: &mut RuntimeRunResult) -> &mut Turn {
    if out.turns.is_empty() {
        out.turns.push(Turn {
            index: 0,
            role: "assistant".to_string(),
            text_deltas: Vec::new(),
            tool_calls: Vec::new(),
            usage: None,
        });
        out.turns_used = 1;
    }
    out.turns.last_mut().expect("turn exists")
}

fn find_tool_call_mut<'a>(
    out: &'a mut RuntimeRunResult,
    id: &str,
) -> Option<&'a mut ToolCallRecord> {
    out.turns
        .iter_mut()
        .flat_map(|turn| turn.tool_calls.iter_mut())
        .find(|call| call.id == id)
}

fn raw_value_to_input(raw: Option<Value>) -> Value {
    match raw {
        Some(Value::Object(object)) => Value::Object(object),
        Some(value) => serde_json::json!({ "rawInput": value }),
        None => serde_json::json!({}),
    }
}

fn merge_raw_input(input: &mut Value, raw_input: Value) {
    match (input, raw_input) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                target.insert(key, value);
            }
        }
        (target, value) => {
            insert_meta(target, "rawInput", value);
        }
    }
}

fn insert_meta(input: &mut Value, key: &str, value: Value) {
    if !input.is_object() {
        *input = Value::Object(Map::new());
    }
    input
        .as_object_mut()
        .expect("object ensured")
        .insert(key.to_string(), value);
}

fn content_block_to_string(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text(text) => text.text.clone(),
        other => json_string(other),
    }
}

fn tool_content_to_string(content: &[ToolCallContent]) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    Some(
        content
            .iter()
            .map(|item| match item {
                ToolCallContent::Content(content) => content_block_to_string(&content.content),
                other => json_string(other),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn tool_kind_to_string(kind: ToolKind) -> String {
    json_string(&kind).trim_matches('"').to_string()
}

fn tool_status_to_string(status: ToolCallStatus) -> String {
    json_string(&status).trim_matches('"').to_string()
}

fn stop_reason_to_string(reason: StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens | StopReason::MaxTurnRequests => "max_turns",
        StopReason::Cancelled => "cancelled",
        StopReason::Refusal => "refusal",
        _ => "other",
    }
    .to_string()
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn json_string(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

#[derive(Debug)]
struct PermissionPolicy {
    mode: String,
    allowed_patterns: Vec<String>,
    user_responses: Vec<UserResponse>,
}

#[derive(Debug)]
struct PermissionDecision {
    outcome: RequestPermissionOutcome,
    diagnostic: String,
}

impl PermissionPolicy {
    fn from_runner(
        runner: &Runner,
        allowed_tools: &[String],
        user_responses: &[UserResponse],
    ) -> Self {
        Self {
            mode: runner.permission_mode.clone(),
            allowed_patterns: allowed_tools.to_vec(),
            user_responses: user_responses.to_vec(),
        }
    }

    fn choose(&self, request: &RequestPermissionRequest) -> PermissionDecision {
        let haystack = permission_haystack(request);
        for response in &self.user_responses {
            if compile_pattern(&response.match_question).is_ok_and(|re| re.is_match(&haystack)) {
                if let Some(option) = request
                    .options
                    .iter()
                    .find(|option| option_matches_choice(option, &response.choose))
                {
                    return PermissionDecision {
                        outcome: selected_outcome(option.option_id.clone()),
                        diagnostic: format!(
                            "ACP permission request matched scripted response `{}` and selected `{}`",
                            response.match_question, option.name
                        ),
                    };
                }
            }
        }

        match self.mode.as_str() {
            "bypassPermissions" | "allow" => select_allow(request),
            "plan" | "deny" => select_reject(request),
            "acceptEdits" => {
                if self
                    .allowed_patterns
                    .iter()
                    .any(|pattern| compile_pattern(pattern).is_ok_and(|re| re.is_match(&haystack)))
                {
                    select_allow(request)
                } else {
                    select_reject(request)
                }
            }
            _ => select_reject(request),
        }
    }
}

fn select_allow(request: &RequestPermissionRequest) -> PermissionDecision {
    let option = request
        .options
        .iter()
        .find(|option| {
            matches!(
                option.kind,
                agent_client_protocol::schema::PermissionOptionKind::AllowOnce
                    | agent_client_protocol::schema::PermissionOptionKind::AllowAlways
            )
        })
        .or_else(|| request.options.first());
    match option {
        Some(option) => PermissionDecision {
            outcome: selected_outcome(option.option_id.clone()),
            diagnostic: format!(
                "ACP permission request selected allow option `{}`",
                option.name
            ),
        },
        None => cancelled_decision("ACP permission request had no options; cancelled"),
    }
}

fn select_reject(request: &RequestPermissionRequest) -> PermissionDecision {
    let option = request.options.iter().find(|option| {
        matches!(
            option.kind,
            agent_client_protocol::schema::PermissionOptionKind::RejectOnce
                | agent_client_protocol::schema::PermissionOptionKind::RejectAlways
        )
    });
    match option {
        Some(option) => PermissionDecision {
            outcome: selected_outcome(option.option_id.clone()),
            diagnostic: format!(
                "ACP permission request selected reject option `{}`",
                option.name
            ),
        },
        None => cancelled_decision("ACP permission request had no reject option; cancelled"),
    }
}

fn selected_outcome(
    option_id: agent_client_protocol::schema::PermissionOptionId,
) -> RequestPermissionOutcome {
    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
}

fn cancelled_decision(message: &str) -> PermissionDecision {
    PermissionDecision {
        outcome: RequestPermissionOutcome::Cancelled,
        diagnostic: message.to_string(),
    }
}

fn option_matches_choice(
    option: &agent_client_protocol::schema::PermissionOption,
    choice: &str,
) -> bool {
    option.option_id.to_string() == choice
        || option.name == choice
        || json_string(&option.kind).trim_matches('"') == choice
}

fn permission_haystack(request: &RequestPermissionRequest) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = request.tool_call.fields.kind {
        parts.push(tool_kind_to_string(kind));
    }
    if let Some(title) = &request.tool_call.fields.title {
        parts.push(title.clone());
    }
    if let Some(raw_input) = &request.tool_call.fields.raw_input {
        parts.push(raw_input.to_string());
    }
    for option in &request.options {
        parts.push(option.option_id.to_string());
        parts.push(option.name.clone());
        parts.push(json_string(&option.kind));
    }
    parts.join("\n")
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn push(cwd: &Path) -> anyhow::Result<Self> {
        let original = std::env::current_dir()?;
        // ACP agent process cwd is inherited from this process; scenario runs
        // are sequential, so this process-global change is safe for the MVP.
        std::env::set_current_dir(cwd)
            .with_context(|| format!("set ACP process cwd to {}", cwd.display()))?;
        Ok(Self { original })
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::{
        McpCapabilities, PermissionOption, PermissionOptionKind, SessionConfigOption,
        SessionConfigOptionCategory, SessionConfigSelectOption, SessionMode, SessionModeState,
        ToolCallId, ToolCallUpdateFields,
    };

    use super::*;
    use crate::config::McpServerConfig;

    fn permission_request() -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "s1",
            ToolCallUpdate::new(
                ToolCallId::new("tool-1"),
                ToolCallUpdateFields::new()
                    .title("Run tests".to_string())
                    .kind(ToolKind::Execute)
                    .raw_input(serde_json::json!({"command": "cargo test"})),
            ),
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
            ],
        )
    }

    fn select_option(
        id: &str,
        name: &str,
        category: Option<SessionConfigOptionCategory>,
        current: &str,
        options: &[(&str, &str)],
    ) -> SessionConfigOption {
        let select_options = options
            .iter()
            .map(|(value, name)| {
                SessionConfigSelectOption::new((*value).to_string(), (*name).to_string())
            })
            .collect::<Vec<_>>();
        let mut option = SessionConfigOption::select(
            id.to_string(),
            name.to_string(),
            current.to_string(),
            select_options,
        );
        if let Some(category) = category {
            option = option.category(category);
        }
        option
    }

    #[test]
    fn acp_config_selection_matches_category_and_display_name() {
        let options = vec![select_option(
            "model_selector",
            "Model",
            Some(SessionConfigOptionCategory::Model),
            "sonnet",
            &[("gpt-5-codex", "GPT 5 Codex"), ("sonnet", "Claude Sonnet")],
        )];

        let selection =
            resolve_acp_config_selection(&options, AcpConfigTarget::Model, "gpt 5 codex")
                .expect("model selected by display name");

        assert_eq!(selection.config_id.to_string(), "model_selector");
        assert_eq!(selection.value_id.to_string(), "gpt-5-codex");
    }

    #[test]
    fn acp_config_selection_uses_fallback_id_and_rejects_unsupported_value() {
        let options = vec![select_option(
            "reasoning",
            "Reasoning",
            None,
            "medium",
            &[("low", "Low"), ("medium", "Medium"), ("high", "High")],
        )];

        let err = resolve_acp_config_selection(&options, AcpConfigTarget::Reasoning, "xhigh")
            .expect_err("unsupported reasoning rejected");

        let message = err.to_string();
        assert!(message.contains("unsupported ACP reasoning `xhigh`"));
        assert!(message.contains("low"));
        assert!(message.contains("high"));
    }

    #[test]
    fn acp_config_selection_rejects_ambiguous_fallback_options() {
        let options = vec![
            select_option("mode", "Mode", None, "default", &[("plan", "Plan")]),
            select_option(
                "agent_mode",
                "Agent Mode",
                None,
                "default",
                &[("plan", "Plan")],
            ),
        ];

        let err = resolve_acp_config_selection(&options, AcpConfigTarget::Mode, "plan")
            .expect_err("ambiguous fallback rejected");

        assert!(err.to_string().contains("ambiguous ACP mode config option"));
    }

    #[test]
    fn acp_mode_selection_falls_back_to_session_modes() {
        let modes = SessionModeState::new(
            "default",
            vec![
                SessionMode::new("default", "Default"),
                SessionMode::new("plan", "Plan"),
            ],
        );

        let mode_id = resolve_acp_session_mode(&modes, "plan").expect("mode selected");

        assert_eq!(mode_id.to_string(), "plan");
    }

    #[test]
    fn permission_policy_allows_bypass_permissions() {
        let runner = Runner {
            permission_mode: "bypassPermissions".to_string(),
            ..Runner::default()
        };
        let policy = PermissionPolicy::from_runner(&runner, &[], &[]);

        let decision = policy.choose(&permission_request());

        assert!(matches!(
            decision.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == "allow"
        ));
    }

    #[test]
    fn permission_policy_rejects_plan_mode() {
        let runner = Runner {
            permission_mode: "plan".to_string(),
            ..Runner::default()
        };
        let policy = PermissionPolicy::from_runner(&runner, &[], &[]);

        let decision = policy.choose(&permission_request());

        assert!(matches!(
            decision.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == "reject"
        ));
    }

    #[test]
    fn permission_policy_accept_edits_uses_allowed_patterns() {
        let runner = Runner {
            permission_mode: "acceptEdits".to_string(),
            allowed_tools_override: Some(vec!["execute".to_string()]),
            ..Runner::default()
        };
        let allowed_tools = runner.allowed_tools_override.clone().unwrap_or_default();
        let policy = PermissionPolicy::from_runner(&runner, &allowed_tools, &[]);

        let decision = policy.choose(&permission_request());

        assert!(matches!(
            decision.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == "allow"
        ));
    }

    #[test]
    fn permission_policy_accept_edits_uses_resolved_allowed_tools() {
        let runner = Runner {
            permission_mode: "acceptEdits".to_string(),
            allowed_tools_override: None,
            ..Runner::default()
        };
        let policy = PermissionPolicy::from_runner(&runner, &["execute".to_string()], &[]);

        let decision = policy.choose(&permission_request());

        assert!(matches!(
            decision.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == "allow"
        ));
    }

    #[test]
    fn permission_policy_scripted_response_wins() {
        let runner = Runner {
            permission_mode: "plan".to_string(),
            ..Runner::default()
        };
        let policy = PermissionPolicy::from_runner(
            &runner,
            &[],
            &[UserResponse {
                match_question: "cargo test".to_string(),
                choose: "allow".to_string(),
            }],
        );

        let decision = policy.choose(&permission_request());

        assert!(matches!(
            decision.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == "allow"
        ));
    }

    #[test]
    fn acp_mcp_builder_rejects_http_without_agent_capability() {
        let server = NamedMcpServerConfig {
            name: "docs".to_string(),
            config: McpServerConfig {
                transport: McpServerTransport::Http,
                url: Some("http://127.0.0.1:3001/mcp".to_string()),
                ..McpServerConfig::default()
            },
        };
        let err = build_acp_mcp_servers(&[server], &AgentCapabilities::default())
            .expect_err("http MCP requires agent capability");

        assert!(err.to_string().contains("HTTP MCP support"));
    }

    #[test]
    fn acp_mcp_builder_converts_stdio_http_and_sse_servers() {
        let servers = vec![
            NamedMcpServerConfig {
                name: "codegraph".to_string(),
                config: McpServerConfig {
                    command: Some("mock-codegraph".to_string()),
                    args: vec!["--fixture".to_string()],
                    env: [("API_TOKEN".to_string(), "secret".to_string())].into(),
                    ..McpServerConfig::default()
                },
            },
            NamedMcpServerConfig {
                name: "docs".to_string(),
                config: McpServerConfig {
                    transport: McpServerTransport::Http,
                    url: Some("http://127.0.0.1:3001/mcp".to_string()),
                    headers: [("Authorization".to_string(), "Bearer secret".to_string())].into(),
                    ..McpServerConfig::default()
                },
            },
            NamedMcpServerConfig {
                name: "events".to_string(),
                config: McpServerConfig {
                    transport: McpServerTransport::Sse,
                    url: Some("http://127.0.0.1:3002/events".to_string()),
                    ..McpServerConfig::default()
                },
            },
        ];
        let capabilities =
            AgentCapabilities::new().mcp_capabilities(McpCapabilities::new().http(true).sse(true));

        let converted = build_acp_mcp_servers(&servers, &capabilities).expect("converts");

        assert!(matches!(converted[0], McpServer::Stdio(_)));
        assert!(matches!(converted[1], McpServer::Http(_)));
        assert!(matches!(converted[2], McpServer::Sse(_)));
    }

    #[cfg(windows)]
    #[test]
    fn acp_session_cwd_strips_windows_verbatim_prefix() {
        let cwd = PathBuf::from(r"\\?\C:\Users\Ichi\AppData\Local\Temp\ai-tester-sandbox");

        assert_eq!(
            acp_session_cwd(&cwd),
            PathBuf::from(r"C:\Users\Ichi\AppData\Local\Temp\ai-tester-sandbox")
        );
    }
}
