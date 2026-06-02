use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_process::{Child, ChildStderr, ChildStdin, ChildStdout};
use futures::future::BoxFuture;
use futures::io::BufReader;
use futures::{AsyncBufReadExt, AsyncWriteExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};

#[derive(Debug, Clone, Copy)]
pub(crate) enum LineDirection {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Error {
    pub(crate) code: i64,
    pub(crate) message: String,
}

impl Error {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    pub(crate) fn method_not_found() -> Self {
        Self::new(-32601, "method not found")
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::invalid_params(err.to_string())
    }
}

macro_rules! string_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
            #[allow(dead_code)]
            pub(crate) fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(SessionId);
string_id!(SessionModeId);
string_id!(SessionConfigId);
string_id!(SessionConfigValueId);
string_id!(ToolCallId);
string_id!(TerminalId);
string_id!(PermissionOptionId);

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) enum ProtocolVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeRequest {
    protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_capabilities: Option<ClientCapabilities>,
}

impl InitializeRequest {
    pub(crate) fn new(_version: ProtocolVersion) -> Self {
        Self {
            protocol_version: 1,
            client_capabilities: None,
        }
    }

    pub(crate) fn client_capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.client_capabilities = Some(capabilities);
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeResponse {
    #[serde(default)]
    pub(crate) agent_capabilities: AgentCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentCapabilities {
    #[serde(default)]
    pub(crate) mcp_capabilities: McpCapabilities,
}

impl AgentCapabilities {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn mcp_capabilities(mut self, capabilities: McpCapabilities) -> Self {
        self.mcp_capabilities = capabilities;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpCapabilities {
    #[serde(default)]
    pub(crate) http: bool,
    #[serde(default)]
    pub(crate) sse: bool,
}

impl McpCapabilities {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn http(mut self, value: bool) -> Self {
        self.http = value;
        self
    }

    #[cfg(test)]
    pub(crate) fn sse(mut self, value: bool) -> Self {
        self.sse = value;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientCapabilities {
    pub(crate) fs: FileSystemCapabilities,
    pub(crate) terminal: bool,
}

impl ClientCapabilities {
    pub(crate) fn new() -> Self {
        Self {
            fs: FileSystemCapabilities::new(),
            terminal: false,
        }
    }

    pub(crate) fn fs(mut self, fs: FileSystemCapabilities) -> Self {
        self.fs = fs;
        self
    }

    pub(crate) fn terminal(mut self, terminal: bool) -> Self {
        self.terminal = terminal;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileSystemCapabilities {
    read_text_file: bool,
    write_text_file: bool,
}

impl FileSystemCapabilities {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn read_text_file(mut self, value: bool) -> Self {
        self.read_text_file = value;
        self
    }

    pub(crate) fn write_text_file(mut self, value: bool) -> Self {
        self.write_text_file = value;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewSessionRequest {
    pub(crate) cwd: PathBuf,
    pub(crate) mcp_servers: Vec<McpServer>,
}

impl NewSessionRequest {
    pub(crate) fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            mcp_servers: Vec::new(),
        }
    }

    pub(crate) fn mcp_servers(mut self, mcp_servers: Vec<McpServer>) -> Self {
        self.mcp_servers = mcp_servers;
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewSessionResponse {
    pub(crate) session_id: SessionId,
    #[serde(default)]
    pub(crate) modes: Option<SessionModeState>,
    #[serde(default)]
    pub(crate) config_options: Option<Vec<SessionConfigOption>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptRequest {
    pub(crate) session_id: SessionId,
    pub(crate) prompt: Vec<ContentBlock>,
}

impl PromptRequest {
    pub(crate) fn text(session_id: impl Into<SessionId>, text: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            prompt: vec![ContentBlock::Text(TextContent::new(text))],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptResponse {
    pub(crate) stop_reason: StopReason,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloseSessionRequest {
    pub(crate) session_id: SessionId,
}

impl CloseSessionRequest {
    pub(crate) fn new(session_id: impl Into<SessionId>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CancelNotification {
    pub(crate) session_id: SessionId,
}

impl CancelNotification {
    pub(crate) fn new(session_id: impl Into<SessionId>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetSessionModeRequest {
    pub(crate) session_id: SessionId,
    pub(crate) mode_id: SessionModeId,
}

impl SetSessionModeRequest {
    pub(crate) fn new(session_id: impl Into<SessionId>, mode_id: impl Into<SessionModeId>) -> Self {
        Self {
            session_id: session_id.into(),
            mode_id: mode_id.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SetSessionModeResponse {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetSessionConfigOptionRequest {
    pub(crate) session_id: SessionId,
    pub(crate) config_id: SessionConfigId,
    pub(crate) value: SessionConfigValueId,
}

impl SetSessionConfigOptionRequest {
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        config_id: impl Into<SessionConfigId>,
        value: impl Into<SessionConfigValueId>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            config_id: config_id.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetSessionConfigOptionResponse {
    pub(crate) config_options: Vec<SessionConfigOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum McpServer {
    Stdio(McpServerStdio),
    Http(McpServerHttp),
    Sse(McpServerSse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerStdio {
    pub(crate) name: String,
    pub(crate) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) env: Vec<EnvVariable>,
}

impl McpServerStdio {
    pub(crate) fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    pub(crate) fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub(crate) fn env(mut self, env: Vec<EnvVariable>) -> Self {
        self.env = env;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerHttp {
    pub(crate) name: String,
    pub(crate) url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<HttpHeader>,
}

impl McpServerHttp {
    pub(crate) fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub(crate) fn headers(mut self, headers: Vec<HttpHeader>) -> Self {
        self.headers = headers;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerSse {
    pub(crate) name: String,
    pub(crate) url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<HttpHeader>,
}

impl McpServerSse {
    pub(crate) fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub(crate) fn headers(mut self, headers: Vec<HttpHeader>) -> Self {
        self.headers = headers;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EnvVariable {
    pub(crate) name: String,
    pub(crate) value: String,
}

impl EnvVariable {
    pub(crate) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HttpHeader {
    pub(crate) name: String,
    pub(crate) value: String,
}

impl HttpHeader {
    pub(crate) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionModeState {
    pub(crate) current_mode_id: SessionModeId,
    pub(crate) available_modes: Vec<SessionMode>,
}

impl SessionModeState {
    #[cfg(test)]
    pub(crate) fn new(
        current_mode_id: impl Into<SessionModeId>,
        available_modes: Vec<SessionMode>,
    ) -> Self {
        Self {
            current_mode_id: current_mode_id.into(),
            available_modes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMode {
    pub(crate) id: SessionModeId,
    pub(crate) name: String,
}

impl SessionMode {
    #[cfg(test)]
    pub(crate) fn new(id: impl Into<SessionModeId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionConfigOption {
    pub(crate) id: SessionConfigId,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) category: Option<SessionConfigOptionCategory>,
    #[serde(flatten)]
    pub(crate) kind: SessionConfigKind,
}

impl SessionConfigOption {
    #[cfg(test)]
    pub(crate) fn select(
        id: impl Into<SessionConfigId>,
        name: impl Into<String>,
        current_value: impl Into<SessionConfigValueId>,
        options: Vec<SessionConfigSelectOption>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category: None,
            kind: SessionConfigKind::Select(SessionConfigSelect {
                current_value: current_value.into(),
                options: SessionConfigSelectOptions::Ungrouped(options),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn category(mut self, category: SessionConfigOptionCategory) -> Self {
        self.category = Some(category);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SessionConfigKind {
    Select(SessionConfigSelect),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionConfigSelect {
    pub(crate) current_value: SessionConfigValueId,
    pub(crate) options: SessionConfigSelectOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum SessionConfigSelectOptions {
    Grouped(Vec<SessionConfigSelectGroup>),
    Ungrouped(Vec<SessionConfigSelectOption>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionConfigSelectGroup {
    pub(crate) options: Vec<SessionConfigSelectOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionConfigSelectOption {
    pub(crate) value: SessionConfigValueId,
    pub(crate) name: String,
}

impl SessionConfigSelectOption {
    #[cfg(test)]
    pub(crate) fn new(value: impl Into<SessionConfigValueId>, name: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionConfigOptionCategory {
    Mode,
    Model,
    ThoughtLevel,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Text(TextContent),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TextContent {
    pub(crate) text: String,
}

impl TextContent {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContentChunk {
    pub(crate) content: ContentBlock,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionNotification {
    pub(crate) session_id: SessionId,
    pub(crate) update: SessionUpdate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub(crate) enum SessionUpdate {
    AgentMessageChunk(ContentChunk),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    Plan(Value),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone)]
pub(crate) enum SessionMessage {
    SessionNotification(SessionNotification),
    StopReason(StopReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolCall {
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) kind: ToolKind,
    #[serde(default)]
    pub(crate) status: ToolCallStatus,
    #[serde(default)]
    pub(crate) content: Vec<ToolCallContent>,
    #[serde(default)]
    pub(crate) locations: Vec<Value>,
    #[serde(default)]
    pub(crate) raw_input: Option<Value>,
    #[serde(default)]
    pub(crate) raw_output: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolCallUpdate {
    pub(crate) tool_call_id: ToolCallId,
    #[serde(flatten)]
    pub(crate) fields: ToolCallUpdateFields,
}

impl ToolCallUpdate {
    #[cfg(test)]
    pub(crate) fn new(tool_call_id: impl Into<ToolCallId>, fields: ToolCallUpdateFields) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            fields,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolCallUpdateFields {
    #[serde(default)]
    pub(crate) kind: Option<ToolKind>,
    #[serde(default)]
    pub(crate) status: Option<ToolCallStatus>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) content: Option<Vec<ToolCallContent>>,
    #[serde(default)]
    pub(crate) locations: Option<Vec<Value>>,
    #[serde(default)]
    pub(crate) raw_input: Option<Value>,
    #[serde(default)]
    pub(crate) raw_output: Option<Value>,
}

impl ToolCallUpdateFields {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[cfg(test)]
    pub(crate) fn kind(mut self, kind: ToolKind) -> Self {
        self.kind = Some(kind);
        self
    }

    #[cfg(test)]
    pub(crate) fn raw_input(mut self, input: Value) -> Self {
        self.raw_input = Some(input);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolKind {
    Execute,
    Read,
    Edit,
    #[serde(other)]
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolCallStatus {
    #[default]
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolCallContent {
    Content(ToolCallContentBlock),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolCallContentBlock {
    pub(crate) content: ContentBlock,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestPermissionRequest {
    pub(crate) session_id: SessionId,
    pub(crate) tool_call: ToolCallUpdate,
    pub(crate) options: Vec<PermissionOption>,
}

impl RequestPermissionRequest {
    #[cfg(test)]
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        tool_call: ToolCallUpdate,
        options: Vec<PermissionOption>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            tool_call,
            options,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestPermissionResponse {
    pub(crate) outcome: RequestPermissionOutcome,
}

impl RequestPermissionResponse {
    pub(crate) fn new(outcome: RequestPermissionOutcome) -> Self {
        Self { outcome }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum RequestPermissionOutcome {
    Cancelled,
    #[serde(rename_all = "camelCase")]
    Selected(SelectedPermissionOutcome),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectedPermissionOutcome {
    pub(crate) option_id: PermissionOptionId,
}

impl SelectedPermissionOutcome {
    pub(crate) fn new(option_id: impl Into<PermissionOptionId>) -> Self {
        Self {
            option_id: option_id.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionOption {
    pub(crate) option_id: PermissionOptionId,
    pub(crate) name: String,
    pub(crate) kind: PermissionOptionKind,
}

impl PermissionOption {
    #[cfg(test)]
    pub(crate) fn new(
        option_id: impl Into<PermissionOptionId>,
        name: impl Into<String>,
        kind: PermissionOptionKind,
    ) -> Self {
        Self {
            option_id: option_id.into(),
            name: name.into(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadTextFileRequest {
    pub(crate) session_id: SessionId,
    pub(crate) path: PathBuf,
    #[serde(default)]
    pub(crate) line: Option<u32>,
    #[serde(default)]
    pub(crate) limit: Option<u32>,
}

impl ReadTextFileRequest {
    #[cfg(test)]
    pub(crate) fn new(session_id: impl Into<SessionId>, path: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            path: path.into(),
            line: None,
            limit: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn line(mut self, line: Option<u32>) -> Self {
        self.line = line;
        self
    }

    #[cfg(test)]
    pub(crate) fn limit(mut self, limit: Option<u32>) -> Self {
        self.limit = limit;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReadTextFileResponse {
    pub(crate) content: String,
}

impl ReadTextFileResponse {
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WriteTextFileRequest {
    pub(crate) session_id: SessionId,
    pub(crate) path: PathBuf,
    pub(crate) content: String,
}

impl WriteTextFileRequest {
    #[cfg(test)]
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        path: impl Into<PathBuf>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            path: path.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WriteTextFileResponse {}

impl WriteTextFileResponse {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTerminalRequest {
    pub(crate) session_id: SessionId,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) env: Vec<EnvVariable>,
    #[serde(default)]
    pub(crate) cwd: Option<PathBuf>,
    #[serde(default)]
    pub(crate) output_byte_limit: Option<u64>,
}

impl CreateTerminalRequest {
    #[cfg(test)]
    pub(crate) fn new(session_id: impl Into<SessionId>, command: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            output_byte_limit: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    #[cfg(test)]
    pub(crate) fn cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }

    #[cfg(test)]
    pub(crate) fn output_byte_limit(mut self, output_byte_limit: Option<u64>) -> Self {
        self.output_byte_limit = output_byte_limit;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTerminalResponse {
    pub(crate) terminal_id: TerminalId,
}

impl CreateTerminalResponse {
    pub(crate) fn new(terminal_id: impl Into<TerminalId>) -> Self {
        Self {
            terminal_id: terminal_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalOutputRequest {
    pub(crate) session_id: SessionId,
    pub(crate) terminal_id: TerminalId,
}

impl TerminalOutputRequest {
    #[cfg(test)]
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        terminal_id: impl Into<TerminalId>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            terminal_id: terminal_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalOutputResponse {
    pub(crate) output: String,
    pub(crate) truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exit_status: Option<TerminalExitStatus>,
}

impl TerminalOutputResponse {
    pub(crate) fn new(output: impl Into<String>, truncated: bool) -> Self {
        Self {
            output: output.into(),
            truncated,
            exit_status: None,
        }
    }

    pub(crate) fn exit_status(mut self, status: Option<TerminalExitStatus>) -> Self {
        self.exit_status = status;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WaitForTerminalExitRequest {
    pub(crate) session_id: SessionId,
    pub(crate) terminal_id: TerminalId,
}

impl WaitForTerminalExitRequest {
    #[cfg(test)]
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        terminal_id: impl Into<TerminalId>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            terminal_id: terminal_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WaitForTerminalExitResponse {
    pub(crate) exit_status: TerminalExitStatus,
}

impl WaitForTerminalExitResponse {
    pub(crate) fn new(exit_status: TerminalExitStatus) -> Self {
        Self { exit_status }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KillTerminalRequest {
    pub(crate) session_id: SessionId,
    pub(crate) terminal_id: TerminalId,
}

impl KillTerminalRequest {
    #[cfg(test)]
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        terminal_id: impl Into<TerminalId>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            terminal_id: terminal_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KillTerminalResponse {}

impl KillTerminalResponse {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseTerminalRequest {
    pub(crate) session_id: SessionId,
    pub(crate) terminal_id: TerminalId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReleaseTerminalResponse {}

impl ReleaseTerminalResponse {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalExitStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) signal: Option<String>,
}

impl TerminalExitStatus {
    pub(crate) fn new() -> Self {
        Self {
            exit_code: None,
            signal: None,
        }
    }

    pub(crate) fn exit_code(mut self, code: Option<u32>) -> Self {
        self.exit_code = code;
        self
    }

    pub(crate) fn signal(mut self, signal: Option<String>) -> Self {
        self.signal = signal;
        self
    }
}

pub(crate) type RequestHandler =
    Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<Value, Error>> + Send + Sync>;
pub(crate) type DebugCallback = Arc<dyn Fn(&str, LineDirection) + Send + Sync + 'static>;
type PendingSender = oneshot::Sender<Result<Value, Error>>;
type PendingMap = Arc<Mutex<HashMap<String, PendingSender>>>;

pub(crate) struct AcpConnection {
    writer: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    updates: mpsc::UnboundedReceiver<SessionNotification>,
    errors: mpsc::UnboundedReceiver<Error>,
    debug_callback: Option<DebugCallback>,
    next_id: u64,
    child: ManagedChildGuard,
}

impl AcpConnection {
    pub(crate) fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        stderr: ChildStderr,
        child: Child,
        debug_callback: Option<DebugCallback>,
        request_handler: RequestHandler,
    ) -> Self {
        let writer = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        let (errors_tx, errors_rx) = mpsc::unbounded_channel();

        tokio::spawn(stdout_loop(
            stdout,
            Arc::clone(&writer),
            Arc::clone(&pending),
            updates_tx,
            errors_tx.clone(),
            request_handler,
            debug_callback.clone(),
        ));
        tokio::spawn(stderr_loop(stderr, debug_callback.clone()));

        Self {
            writer,
            pending,
            updates: updates_rx,
            errors: errors_rx,
            debug_callback,
            next_id: 1,
            child: ManagedChildGuard::new(child),
        }
    }

    pub(crate) async fn request<P, T>(&mut self, method: &str, params: P) -> Result<T, Error>
    where
        P: Serialize,
        T: for<'de> Deserialize<'de>,
    {
        let mut pending = self.send_request_pending(method, params).await?;
        pending.recv().await
    }

    pub(crate) async fn send_request_pending<P>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<PendingResponse, Error>
    where
        P: Serialize,
    {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.to_string(), tx);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": serde_json::to_value(params)?,
        });
        if let Err(err) =
            write_json_line(&self.writer, &message, self.debug_callback.as_ref()).await
        {
            let _ = self.pending.lock().await.remove(&id.to_string());
            return Err(err);
        }
        Ok(PendingResponse { receiver: rx })
    }

    pub(crate) async fn notify<P>(&self, method: &str, params: P) -> Result<(), Error>
    where
        P: Serialize,
    {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": serde_json::to_value(params)?,
        });
        write_json_line(&self.writer, &message, self.debug_callback.as_ref()).await
    }

    pub(crate) async fn read_session_message(
        &mut self,
        pending: &mut PendingResponse,
    ) -> Result<SessionMessage, Error> {
        tokio::select! {
            biased;
            update = self.updates.recv() => {
                match update {
                    Some(update) => Ok(SessionMessage::SessionNotification(update)),
                    None => Err(Error::internal("ACP stdout closed")),
                }
            }
            err = self.errors.recv() => {
                Err(err.unwrap_or_else(|| Error::internal("ACP connection closed")))
            }
            response = &mut pending.receiver => {
                let value = response
                    .map_err(|_| Error::internal("ACP pending request was dropped"))??;
                let response: PromptResponse = serde_json::from_value(value)?;
                Ok(SessionMessage::StopReason(response.stop_reason))
            }
        }
    }

    pub(crate) async fn read_update(&mut self) -> Result<SessionMessage, Error> {
        tokio::select! {
            update = self.updates.recv() => {
                match update {
                    Some(update) => Ok(SessionMessage::SessionNotification(update)),
                    None => Err(Error::internal("ACP stdout closed")),
                }
            }
            err = self.errors.recv() => {
                Err(err.unwrap_or_else(|| Error::internal("ACP connection closed")))
            }
        }
    }
}

impl Drop for AcpConnection {
    fn drop(&mut self) {
        self.child.kill_if_running();
    }
}

pub(crate) struct PendingResponse {
    receiver: oneshot::Receiver<Result<Value, Error>>,
}

impl PendingResponse {
    async fn recv<T>(&mut self) -> Result<T, Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = (&mut self.receiver)
            .await
            .map_err(|_| Error::internal("ACP pending request was dropped"))??;
        serde_json::from_value(value).map_err(Into::into)
    }
}

struct ManagedChildGuard {
    child: Child,
    pid: u32,
    killed: bool,
}

impl ManagedChildGuard {
    fn new(child: Child) -> Self {
        let pid = child.id();
        Self {
            child,
            pid,
            killed: false,
        }
    }

    fn kill_if_running(&mut self) {
        if !self.killed {
            kill_process_tree(self.pid);
            let _ = self.child.kill();
            self.killed = true;
        }
    }
}

impl Drop for ManagedChildGuard {
    fn drop(&mut self) {
        self.kill_if_running();
    }
}

async fn stdout_loop(
    stdout: ChildStdout,
    writer: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    updates: mpsc::UnboundedSender<SessionNotification>,
    errors: mpsc::UnboundedSender<Error>,
    request_handler: RequestHandler,
    debug_callback: Option<DebugCallback>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line_result) = lines.next().await {
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                let _ = errors.send(Error::internal(format!("read ACP stdout failed: {err}")));
                return;
            }
        };
        if let Some(callback) = &debug_callback {
            callback(&line, LineDirection::Stdout);
        }
        let message = match serde_json::from_str::<Value>(&line) {
            Ok(message) => message,
            Err(err) => {
                let _ = errors.send(Error::new(
                    -32700,
                    format!("invalid ACP JSON-RPC stdout line: {err}: {line}"),
                ));
                return;
            }
        };
        if let Some(id) = message.get("id").cloned() {
            if message.get("method").is_some() {
                handle_inbound_request(
                    &writer,
                    &request_handler,
                    id,
                    message,
                    debug_callback.as_ref(),
                )
                .await;
            } else {
                handle_inbound_response(&pending, id, message).await;
            }
        } else if message.get("method").and_then(Value::as_str) == Some("session/update") {
            match serde_json::from_value::<SessionNotification>(
                message.get("params").cloned().unwrap_or(Value::Null),
            ) {
                Ok(update) => {
                    let _ = updates.send(update);
                }
                Err(err) => {
                    let _ = errors.send(Error::invalid_params(format!(
                        "invalid session/update params: {err}"
                    )));
                    return;
                }
            }
        }
    }
    let _ = errors.send(Error::internal("ACP stdout closed"));
}

async fn stderr_loop(stderr: ChildStderr, debug_callback: Option<DebugCallback>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(Ok(line)) = lines.next().await {
        if let Some(callback) = &debug_callback {
            callback(&line, LineDirection::Stderr);
        }
    }
}

async fn handle_inbound_request(
    writer: &Arc<Mutex<ChildStdin>>,
    request_handler: &RequestHandler,
    id: Value,
    message: Value,
    debug_callback: Option<&DebugCallback>,
) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let response = match request_handler(method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
    };
    let _ = write_json_line(writer, &response, debug_callback).await;
}

async fn handle_inbound_response(pending: &PendingMap, id: Value, message: Value) {
    let key = rpc_id_key(&id);
    let Some(sender) = pending.lock().await.remove(&key) else {
        return;
    };
    if let Some(error) = message.get("error") {
        let err = serde_json::from_value::<Error>(error.clone())
            .unwrap_or_else(|_| Error::internal(format!("invalid ACP JSON-RPC error: {error}")));
        let _ = sender.send(Err(err));
    } else {
        let result = message.get("result").cloned().unwrap_or(Value::Null);
        let _ = sender.send(Ok(result));
    }
}

async fn write_json_line(
    writer: &Arc<Mutex<ChildStdin>>,
    value: &Value,
    debug_callback: Option<&DebugCallback>,
) -> Result<(), Error> {
    let line = serde_json::to_string(value)?;
    if let Some(callback) = debug_callback {
        callback(&line, LineDirection::Stdin);
    }
    let mut writer = writer.lock().await;
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|err| Error::internal(format!("write ACP stdin failed: {err}")))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|err| Error::internal(format!("write ACP stdin failed: {err}")))?;
    writer
        .flush()
        .await
        .map_err(|err| Error::internal(format!("flush ACP stdin failed: {err}")))?;
    Ok(())
}

fn rpc_id_key(id: &Value) -> String {
    match id {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let group = format!("-{pid}");
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &group])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &group])
            .status();
    }
}
