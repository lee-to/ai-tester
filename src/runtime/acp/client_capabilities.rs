use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::wire::{
    CreateTerminalRequest, CreateTerminalResponse, Error, KillTerminalRequest,
    KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, TerminalExitStatus, TerminalOutputRequest, TerminalOutputResponse,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use crate::trace::ToolCallRecord;
use crate::util::path::{
    canonicalize_existing, path_is_within, resolve_existing_inside, resolve_write_target_inside,
};

const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
const MAX_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct AcpClientBridge {
    sandbox_root: PathBuf,
    session_id: Mutex<Option<String>>,
    records: Mutex<Vec<ToolCallRecord>>,
    next_record_id: AtomicU64,
    terminal_manager: TerminalManager,
}

impl AcpClientBridge {
    pub(crate) fn new(
        sandbox_root: PathBuf,
        terminal_wait_timeout: Duration,
        scenario_env: BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        let sandbox_root = canonicalize_existing(&sandbox_root)?;
        Ok(Self {
            sandbox_root,
            session_id: Mutex::new(None),
            records: Mutex::new(Vec::new()),
            next_record_id: AtomicU64::new(1),
            terminal_manager: TerminalManager::new(terminal_wait_timeout, scenario_env),
        })
    }

    pub(crate) fn set_session_id(&self, session_id: String) {
        if let Ok(mut expected) = self.session_id.lock() {
            *expected = Some(session_id);
        }
    }

    pub(crate) fn drain_tool_calls(&self) -> Vec<ToolCallRecord> {
        self.records
            .lock()
            .map(|mut records| records.drain(..).collect())
            .unwrap_or_default()
    }

    pub(crate) fn handle_read_text_file(
        &self,
        request: ReadTextFileRequest,
    ) -> Result<ReadTextFileResponse, Error> {
        let mut input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            let resolved = resolve_existing_inside(&self.sandbox_root, &request.path)
                .map_err(acp_invalid_params)?;
            insert_meta(
                &mut input,
                "_acpResolvedPath",
                Value::String(resolved.display().to_string()),
            );
            let content = fs::read_to_string(&resolved)
                .map_err(|err| acp_invalid_params(format!("read text file failed: {err}")))?;
            Ok(ReadTextFileResponse::new(slice_lines(
                &content,
                request.line,
                request.limit,
            )))
        })();
        self.record_result(
            "fs/read_text_file",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) fn handle_write_text_file(
        &self,
        request: WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, Error> {
        let mut input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            let resolved = resolve_write_target_inside(&self.sandbox_root, &request.path)
                .map_err(acp_invalid_params)?;
            insert_meta(
                &mut input,
                "_acpResolvedPath",
                Value::String(resolved.display().to_string()),
            );
            if let Some(parent) = resolved.parent() {
                fs::create_dir_all(parent).map_err(|err| {
                    acp_invalid_params(format!("create parent dirs failed: {err}"))
                })?;
            }
            fs::write(&resolved, request.content)
                .map_err(|err| acp_invalid_params(format!("write text file failed: {err}")))?;
            Ok(WriteTextFileResponse::new())
        })();
        self.record_result(
            "fs/write_text_file",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) fn handle_create_terminal(
        &self,
        request: CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, Error> {
        let mut input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            let cwd = match request.cwd.as_deref() {
                Some(cwd) => resolve_existing_inside(&self.sandbox_root, cwd),
                None => Ok(self.sandbox_root.clone()),
            }
            .map_err(acp_invalid_params)?;
            insert_meta(
                &mut input,
                "_acpResolvedCwd",
                Value::String(cwd.display().to_string()),
            );
            self.ensure_command_path_allowed(&request.command)?;
            let terminal_id = self.terminal_manager.create(
                request.command,
                request.args,
                request.env,
                cwd,
                request.output_byte_limit,
            )?;
            Ok(CreateTerminalResponse::new(terminal_id))
        })();
        self.record_result(
            "terminal/create",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) fn handle_terminal_output(
        &self,
        request: TerminalOutputRequest,
    ) -> Result<TerminalOutputResponse, Error> {
        let input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            self.terminal_manager
                .output(&request.terminal_id.to_string())
        })();
        self.record_result(
            "terminal/output",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) async fn handle_wait_for_terminal_exit(
        &self,
        request: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, Error> {
        let input = value_object(&request);
        let result = match self.ensure_session(&request.session_id.to_string()) {
            Ok(()) => {
                self.terminal_manager
                    .wait_for_exit(&request.terminal_id.to_string())
                    .await
            }
            Err(err) => Err(err),
        };
        self.record_result(
            "terminal/wait_for_exit",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) fn handle_kill_terminal(
        &self,
        request: KillTerminalRequest,
    ) -> Result<KillTerminalResponse, Error> {
        let input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            self.terminal_manager.kill(&request.terminal_id.to_string())
        })();
        self.record_result(
            "terminal/kill",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    pub(crate) fn handle_release_terminal(
        &self,
        request: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, Error> {
        let input = value_object(&request);
        let result = (|| {
            self.ensure_session(&request.session_id.to_string())?;
            self.terminal_manager
                .release(&request.terminal_id.to_string())
        })();
        self.record_result(
            "terminal/release",
            input,
            response_text(&result),
            result.is_err(),
        );
        result
    }

    fn ensure_session(&self, actual: &str) -> Result<(), Error> {
        let expected = self
            .session_id
            .lock()
            .ok()
            .and_then(|session_id| session_id.clone())
            .ok_or_else(|| acp_invalid_params("ACP session is not initialized"))?;
        if expected == actual {
            Ok(())
        } else {
            Err(acp_invalid_params(format!(
                "request session `{actual}` does not match active session `{expected}`"
            )))
        }
    }

    fn ensure_command_path_allowed(&self, command: &str) -> Result<(), Error> {
        let command_path = Path::new(command);
        if !command_path.is_absolute() && command_path.components().count() <= 1 {
            return Ok(());
        }
        let resolved = resolve_existing_inside(&self.sandbox_root, command_path)
            .map_err(acp_invalid_params)?;
        if path_is_within(&resolved, &self.sandbox_root) {
            Ok(())
        } else {
            Err(acp_invalid_params("terminal command escapes sandbox"))
        }
    }

    fn record_result(
        &self,
        name: &str,
        mut input: Value,
        result_content: Option<String>,
        result_is_error: bool,
    ) {
        if let Some(content) = &result_content {
            if let Ok(raw_output) = serde_json::from_str::<Value>(content) {
                insert_meta(&mut input, "_acpRawOutput", raw_output);
            } else if result_is_error {
                insert_meta(&mut input, "_acpError", Value::String(content.clone()));
            }
        }
        let id = self.next_record_id.fetch_add(1, Ordering::SeqCst);
        let record = ToolCallRecord {
            id: format!("acp-client-{id}"),
            name: name.to_string(),
            input,
            result_content,
            result_is_error,
            answered: None,
        };
        if let Ok(mut records) = self.records.lock() {
            records.push(record);
        }
    }
}

#[derive(Debug)]
struct TerminalManager {
    entries: Mutex<HashMap<String, Arc<TerminalEntry>>>,
    next_id: AtomicU64,
    wait_timeout: Duration,
    scenario_env: BTreeMap<String, String>,
}

impl TerminalManager {
    fn new(wait_timeout: Duration, scenario_env: BTreeMap<String, String>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            wait_timeout,
            scenario_env,
        }
    }

    fn create(
        &self,
        command: String,
        args: Vec<String>,
        env: Vec<super::wire::EnvVariable>,
        cwd: PathBuf,
        output_byte_limit: Option<u64>,
    ) -> Result<String, Error> {
        let limit = output_byte_limit
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(DEFAULT_OUTPUT_LIMIT)
            .clamp(1, MAX_OUTPUT_LIMIT);
        let mut process = Command::new(&command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        process
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process.envs(&self.scenario_env);
        for var in env {
            process.env(var.name, var.value);
        }
        let mut child = process
            .spawn()
            .map_err(|err| acp_invalid_params(format!("terminal create failed: {err}")))?;
        let pid = child.id();
        let output = Arc::new(Mutex::new(BoundedOutput::new(limit)));
        if let Some(stdout) = child.stdout.take() {
            spawn_output_reader(stdout, Arc::clone(&output));
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_output_reader(stderr, Arc::clone(&output));
        }
        let status = Arc::new(Mutex::new(None));
        let status_for_waiter = Arc::clone(&status);
        thread::spawn(move || {
            if let Ok(exit_status) = child.wait() {
                let terminal_status = exit_status_to_terminal(exit_status);
                if let Ok(mut status) = status_for_waiter.lock() {
                    if status.is_none() {
                        *status = Some(terminal_status);
                    }
                }
            }
        });

        let id = format!("term-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let entry = Arc::new(TerminalEntry {
            pid,
            output,
            status,
        });
        self.entries
            .lock()
            .map_err(|_| acp_invalid_params("terminal table lock poisoned"))?
            .insert(id.clone(), entry);
        Ok(id)
    }

    fn output(&self, terminal_id: &str) -> Result<TerminalOutputResponse, Error> {
        let entry = self.entry(terminal_id)?;
        let (output, truncated) = entry
            .output
            .lock()
            .map(|output| (output.text.clone(), output.truncated))
            .map_err(|_| acp_invalid_params("terminal output lock poisoned"))?;
        let exit_status = entry
            .status
            .lock()
            .map_err(|_| acp_invalid_params("terminal status lock poisoned"))?
            .clone();
        Ok(TerminalOutputResponse::new(output, truncated).exit_status(exit_status))
    }

    async fn wait_for_exit(&self, terminal_id: &str) -> Result<WaitForTerminalExitResponse, Error> {
        let entry = self.entry(terminal_id)?;
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            if let Some(status) = entry
                .status
                .lock()
                .map_err(|_| acp_invalid_params("terminal status lock poisoned"))?
                .clone()
            {
                return Ok(WaitForTerminalExitResponse::new(status));
            }
            if Instant::now() >= deadline {
                let timeout_status = TerminalExitStatus::new().signal(Some("timeout".to_string()));
                set_status_once(&entry.status, timeout_status.clone())?;
                let _ = kill_process_tree(entry.pid);
                return Ok(WaitForTerminalExitResponse::new(timeout_status));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn kill(&self, terminal_id: &str) -> Result<KillTerminalResponse, Error> {
        let entry = self.entry(terminal_id)?;
        let _ = kill_process_tree(entry.pid);
        if entry
            .status
            .lock()
            .map_err(|_| acp_invalid_params("terminal status lock poisoned"))?
            .is_none()
        {
            set_status_once(
                &entry.status,
                TerminalExitStatus::new().signal(Some("killed".to_string())),
            )?;
        }
        Ok(KillTerminalResponse::new())
    }

    fn release(&self, terminal_id: &str) -> Result<ReleaseTerminalResponse, Error> {
        let entry = self
            .entries
            .lock()
            .map_err(|_| acp_invalid_params("terminal table lock poisoned"))?
            .remove(terminal_id)
            .ok_or_else(|| acp_invalid_params(format!("unknown terminal id `{terminal_id}`")))?;
        if entry
            .status
            .lock()
            .map_err(|_| acp_invalid_params("terminal status lock poisoned"))?
            .is_none()
        {
            let _ = kill_process_tree(entry.pid);
            set_status_once(
                &entry.status,
                TerminalExitStatus::new().signal(Some("released".to_string())),
            )?;
        }
        Ok(ReleaseTerminalResponse::new())
    }

    fn entry(&self, terminal_id: &str) -> Result<Arc<TerminalEntry>, Error> {
        self.entries
            .lock()
            .map_err(|_| acp_invalid_params("terminal table lock poisoned"))?
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| acp_invalid_params(format!("unknown terminal id `{terminal_id}`")))
    }
}

#[derive(Debug)]
struct TerminalEntry {
    pid: u32,
    output: Arc<Mutex<BoundedOutput>>,
    status: Arc<Mutex<Option<TerminalExitStatus>>>,
}

#[derive(Debug)]
struct BoundedOutput {
    text: String,
    limit: usize,
    truncated: bool,
}

impl BoundedOutput {
    fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
            truncated: false,
        }
    }

    fn push_lossy(&mut self, bytes: &[u8]) {
        self.text.push_str(&String::from_utf8_lossy(bytes));
        while self.text.len() > self.limit {
            self.truncated = true;
            let remove = self.text.len() - self.limit;
            let boundary = self
                .text
                .char_indices()
                .map(|(idx, _)| idx)
                .find(|idx| *idx >= remove)
                .unwrap_or(self.text.len());
            self.text.drain(..boundary);
        }
    }
}

fn spawn_output_reader(mut reader: impl Read + Send + 'static, output: Arc<Mutex<BoundedOutput>>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut output) = output.lock() {
                        output.push_lossy(&buffer[..read]);
                    }
                }
            }
        }
    });
}

fn set_status_once(
    status: &Mutex<Option<TerminalExitStatus>>,
    next: TerminalExitStatus,
) -> Result<(), Error> {
    let mut status = status
        .lock()
        .map_err(|_| acp_invalid_params("terminal status lock poisoned"))?;
    if status.is_none() {
        *status = Some(next);
    }
    Ok(())
}

fn exit_status_to_terminal(status: std::process::ExitStatus) -> TerminalExitStatus {
    let mut out = TerminalExitStatus::new();
    if let Some(code) = status.code() {
        out = out.exit_code(Some(code as u32));
    } else {
        out = out.signal(Some("terminated".to_string()));
    }
    out
}

fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let process_group = format!("-{pid}");
        Command::new("kill")
            .args(["-TERM", "--", &process_group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        thread::sleep(Duration::from_millis(100));
        Command::new("kill")
            .args(["-KILL", "--", &process_group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|_| ())
    }
}

fn slice_lines(content: &str, line: Option<u32>, limit: Option<u32>) -> String {
    let Some(start) = line else {
        return content.to_string();
    };
    let skip = start.saturating_sub(1) as usize;
    let take = limit.map(|limit| limit as usize);
    let lines = content.lines().skip(skip);
    match take {
        Some(0) => String::new(),
        Some(take) => lines.take(take).collect::<Vec<_>>().join("\n"),
        None => lines.collect::<Vec<_>>().join("\n"),
    }
}

fn value_object(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| Value::Object(Map::new()))
}

fn response_text<T: serde::Serialize>(result: &Result<T, Error>) -> Option<String> {
    match result {
        Ok(value) => serde_json::to_string(value).ok(),
        Err(err) => Some(err.message.clone()),
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

fn acp_invalid_params(message: impl ToString) -> Error {
    Error::new(-32602, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::wire::{
        KillTerminalRequest, TerminalOutputRequest, WaitForTerminalExitRequest,
    };
    use super::*;
    use tempfile::TempDir;

    fn bridge(root: &Path, wait_timeout: Duration) -> AcpClientBridge {
        let bridge = AcpClientBridge::new(root.to_path_buf(), wait_timeout, BTreeMap::new())
            .expect("bridge");
        bridge.set_session_id("s1".to_string());
        bridge
    }

    #[test]
    fn bridge_fs_reads_line_limits_and_writes_nested_files() {
        let tmp = TempDir::new().expect("temp dir");
        fs::create_dir_all(tmp.path().join("notes")).expect("notes dir");
        fs::write(tmp.path().join("notes/input.txt"), "one\ntwo\nthree\n").expect("input");
        let bridge = bridge(tmp.path(), Duration::from_millis(100));

        let read = bridge
            .handle_read_text_file(
                ReadTextFileRequest::new("s1", tmp.path().join("notes/input.txt"))
                    .line(Some(2))
                    .limit(Some(1)),
            )
            .expect("read succeeds");
        assert_eq!(read.content, "two");

        bridge
            .handle_write_text_file(WriteTextFileRequest::new(
                "s1",
                tmp.path().join("generated/output.txt"),
                "created",
            ))
            .expect("write succeeds");
        assert_eq!(
            fs::read_to_string(tmp.path().join("generated/output.txt")).expect("written"),
            "created"
        );

        let records = bridge.drain_tool_calls();
        assert_eq!(records[0].name, "fs/read_text_file");
        assert_eq!(records[1].name, "fs/write_text_file");
        assert!(records.iter().all(|record| !record.result_is_error));
    }

    #[test]
    fn bridge_fs_rejects_outside_paths_and_logs_errors() {
        let sandbox = TempDir::new().expect("sandbox");
        let outside = TempDir::new().expect("outside");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").expect("outside file");
        let bridge = bridge(sandbox.path(), Duration::from_millis(100));

        let err = bridge
            .handle_read_text_file(ReadTextFileRequest::new("s1", &outside_file))
            .expect_err("outside read rejected");
        assert!(err.message.contains("escapes sandbox"));

        let records = bridge.drain_tool_calls();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "fs/read_text_file");
        assert!(records[0].result_is_error);
    }

    #[tokio::test]
    async fn bridge_terminal_times_out_hanging_commands_and_kill_is_idempotent() {
        let tmp = TempDir::new().expect("temp dir");
        let bridge = bridge(tmp.path(), Duration::from_millis(100));

        let created = bridge
            .handle_create_terminal(sleep_terminal_request(tmp.path(), 1024))
            .expect("terminal created");
        let terminal_id = created.terminal_id.to_string();

        let waited = bridge
            .handle_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                "s1",
                terminal_id.clone(),
            ))
            .await
            .expect("wait returns");
        assert_eq!(waited.exit_status.signal.as_deref(), Some("timeout"));

        bridge
            .handle_kill_terminal(KillTerminalRequest::new("s1", terminal_id))
            .expect("kill is idempotent after timeout");
    }

    #[tokio::test]
    async fn bridge_terminal_reports_output_truncation_and_unknown_ids() {
        let tmp = TempDir::new().expect("temp dir");
        let bridge = bridge(tmp.path(), Duration::from_secs(2));

        let created = bridge
            .handle_create_terminal(print_terminal_request(tmp.path(), 4))
            .expect("terminal created");
        let terminal_id = created.terminal_id.to_string();
        bridge
            .handle_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                "s1",
                terminal_id.clone(),
            ))
            .await
            .expect("wait succeeds");
        let output = bridge
            .handle_terminal_output(TerminalOutputRequest::new("s1", terminal_id))
            .expect("output succeeds");
        assert!(output.truncated);
        assert!(output.output.len() <= 4);

        let err = bridge
            .handle_kill_terminal(KillTerminalRequest::new("s1", "missing-terminal"))
            .expect_err("unknown terminal id rejected");
        assert!(err.message.contains("unknown terminal id"));
    }

    fn sleep_terminal_request(root: &Path, limit: u64) -> CreateTerminalRequest {
        #[cfg(windows)]
        {
            CreateTerminalRequest::new("s1", "powershell")
                .args(vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "Start-Sleep -Seconds 5".to_string(),
                ])
                .cwd(Some(root.to_path_buf()))
                .output_byte_limit(Some(limit))
        }
        #[cfg(not(windows))]
        {
            CreateTerminalRequest::new("s1", "sh")
                .args(vec!["-c".to_string(), "sleep 5".to_string()])
                .cwd(Some(root.to_path_buf()))
                .output_byte_limit(Some(limit))
        }
    }

    fn print_terminal_request(root: &Path, limit: u64) -> CreateTerminalRequest {
        #[cfg(windows)]
        {
            CreateTerminalRequest::new("s1", "powershell")
                .args(vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "Write-Output abcdefghij".to_string(),
                ])
                .cwd(Some(root.to_path_buf()))
                .output_byte_limit(Some(limit))
        }
        #[cfg(not(windows))]
        {
            CreateTerminalRequest::new("s1", "sh")
                .args(vec!["-c".to_string(), "printf abcdefghij".to_string()])
                .cwd(Some(root.to_path_buf()))
                .output_byte_limit(Some(limit))
        }
    }
}
