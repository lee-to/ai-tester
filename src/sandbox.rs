use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use walkdir::WalkDir;

use crate::scenario::{FixtureFile, Fixtures};

const SETUP_OUTPUT_CAPTURE_BYTES: usize = 64 * 1024;
const SETUP_OUTPUT_PREVIEW_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone)]
pub struct SandboxOptions {
    pub keep: bool,
    pub setup_timeout: Duration,
    pub skill: Option<SkillInstall>,
}

impl Default for SandboxOptions {
    fn default() -> Self {
        Self {
            keep: false,
            setup_timeout: Duration::from_secs(crate::config::DEFAULT_SETUP_TIMEOUT_SECONDS),
            skill: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillInstall {
    pub name: String,
    pub dir_path: PathBuf,
}

#[derive(Debug)]
pub struct Sandbox {
    pub path: PathBuf,
    pub skill_install_path: Option<PathBuf>,
    keep: bool,
}

impl Sandbox {
    pub fn cleanup(&self) -> anyhow::Result<()> {
        if self.keep || !self.path.exists() {
            return Ok(());
        }
        remove_dir_all_retry(&self.path)
            .with_context(|| format!("remove sandbox {}", self.path.display()))
    }
}

fn remove_dir_all_retry(path: &Path) -> io::Result<()> {
    const ATTEMPTS: usize = 30;
    const DELAY: Duration = Duration::from_millis(100);

    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(_) if !path.exists() => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                if attempt + 1 < ATTEMPTS {
                    thread::sleep(DELAY);
                }
            }
        }
    }
    Err(last_error.expect("at least one removal attempt ran"))
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Drop cannot return cleanup errors. Normal run paths still call cleanup()
        // explicitly so removal failures can be reported to the caller.
        let _ = self.cleanup();
    }
}

pub fn create_sandbox(
    scenario_name: &str,
    fixtures: &Fixtures,
    opts: SandboxOptions,
) -> anyhow::Result<Sandbox> {
    let temp = tempfile::Builder::new()
        .prefix(&format!("ai-tester-{}-", safe_name(scenario_name)))
        .tempdir()?;
    #[allow(deprecated)]
    let raw = temp.into_path();
    let base = crate::util::path::canonicalize_existing(&raw)?;
    let mut skill_install_path = None;

    let result = (|| {
        if let Some(skill) = &opts.skill {
            let rel = PathBuf::from(".claude").join("skills").join(&skill.name);
            copy_dir_contents(&skill.dir_path, &base.join(&rel))?;
            skill_install_path = Some(rel);
        }

        if fixtures.git_init {
            run_git(&base, &["init", "-q"])?;
            run_git(&base, &["config", "user.email", "ai-tester@example.com"])?;
            run_git(&base, &["config", "user.name", "ai-tester"])?;
            run_git(&base, &["config", "commit.gpgsign", "false"])?;
            if opts.skill.is_some() {
                fs::write(base.join(".gitignore"), ".claude/\n")?;
            }
        }

        for tree in &fixtures.copy_trees {
            copy_dir_contents(Path::new(&tree.from), &resolve_inside(&base, &tree.to)?)?;
        }

        for file in &fixtures.files_committed {
            write_fixture(&base, file)?;
        }
        let has_baseline_content =
            !fixtures.copy_trees.is_empty() || !fixtures.files_committed.is_empty();
        if fixtures.git_init && has_baseline_content {
            run_git(&base, &["add", "-A"])?;
            run_git(
                &base,
                &["commit", "-q", "-m", "ai-tester: fixture baseline"],
            )?;
        } else if fixtures.git_init {
            fs::write(base.join(".ai-tester-keep"), "")?;
            run_git(&base, &["add", ".ai-tester-keep"])?;
            run_git(
                &base,
                &["commit", "-q", "-m", "ai-tester: initial empty commit"],
            )?;
        }

        if fixtures.git_init {
            if let Some(branch) = &fixtures.git_branch {
                run_git(&base, &["checkout", "-q", "-B", branch])?;
            }
        }

        for file in &fixtures.files_staged {
            write_fixture(&base, file)?;
        }
        if fixtures.git_init && !fixtures.files_staged.is_empty() {
            let mut args = vec!["add".to_string()];
            args.extend(fixtures.files_staged.iter().map(|f| f.path.clone()));
            run_git_owned(&base, &args)?;
        }

        for file in &fixtures.files_unstaged {
            write_fixture(&base, file)?;
        }

        for command in &fixtures.setup_commands {
            run_shell(&base, command, &fixtures.env, opts.setup_timeout)?;
        }

        Ok::<_, anyhow::Error>(())
    })();

    if let Err(err) = result {
        if !opts.keep {
            let _ = fs::remove_dir_all(&base);
        }
        return Err(anyhow::anyhow!(
            "sandbox setup failed for `{scenario_name}`: {err}"
        ));
    }

    Ok(Sandbox {
        path: base,
        skill_install_path,
        keep: opts.keep,
    })
}

fn write_fixture(base: &Path, file: &FixtureFile) -> anyhow::Result<()> {
    let abs = resolve_inside(base, &file.path)?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(abs, file.content.as_deref().unwrap_or_default())?;
    Ok(())
}

fn resolve_inside(base: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        bail!("fixture path escapes sandbox: {rel}");
    }
    for component in rel_path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            bail!("fixture path escapes sandbox: {rel}");
        }
    }
    Ok(base.join(rel_path))
}

fn copy_dir_contents(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in WalkDir::new(src) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let owned = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    run_git_owned(cwd, &owned)
}

fn run_git_owned(cwd: &Path, args: &[String]) -> anyhow::Result<()> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn run_shell(
    cwd: &Path,
    command: &str,
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    let mut child = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    };
    #[cfg(not(windows))]
    let mut child = {
        use std::os::unix::process::CommandExt;

        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command]);
        cmd.process_group(0);
        cmd
    };

    let mut child = child
        .current_dir(cwd)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn setup command `{command}`"))?;

    let stdout = child
        .stdout
        .take()
        .map(spawn_output_reader)
        .context("setup command stdout pipe missing")?;
    let stderr = child
        .stderr
        .take()
        .map(spawn_output_reader)
        .context("setup command stderr pipe missing")?;

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("wait for setup command `{command}`"))?
        {
            let stdout = join_output_reader(stdout);
            let stderr = join_output_reader(stderr);
            if status.success() {
                return Ok(());
            }
            bail!(
                "setup command failed: `{command}` ({status})\nstdout preview:\n{}\nstderr preview:\n{}",
                output_preview(&stdout),
                output_preview(&stderr),
            );
        }

        if Instant::now() >= deadline {
            kill_process_tree(&mut child);
            let _ = child.wait();
            let stdout = join_output_reader(stdout);
            let stderr = join_output_reader(stderr);
            bail!(
                "setup command timed out (timeout {}): `{command}`\nstdout preview:\n{}\nstderr preview:\n{}",
                format_timeout(timeout),
                output_preview(&stdout),
                output_preview(&stderr),
            );
        }

        thread::sleep(Duration::from_millis(25));
    }
}

#[derive(Debug)]
struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn spawn_output_reader<R>(mut reader: R) -> thread::JoinHandle<CapturedOutput>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0u8; 8192];
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => break,
            };
            let remaining = SETUP_OUTPUT_CAPTURE_BYTES.saturating_sub(bytes.len());
            if remaining > 0 {
                bytes.extend_from_slice(&buffer[..read.min(remaining)]);
            }
            if read > remaining || bytes.len() >= SETUP_OUTPUT_CAPTURE_BYTES {
                truncated = true;
            }
        }
        CapturedOutput { bytes, truncated }
    })
}

fn join_output_reader(handle: thread::JoinHandle<CapturedOutput>) -> CapturedOutput {
    handle.join().unwrap_or_else(|_| CapturedOutput {
        bytes: b"<reader thread panicked>".to_vec(),
        truncated: false,
    })
}

fn output_preview(output: &CapturedOutput) -> String {
    if output.bytes.is_empty() {
        return "<empty>".to_string();
    }
    let mut text = String::from_utf8_lossy(
        &output.bytes[..output.bytes.len().min(SETUP_OUTPUT_PREVIEW_BYTES)],
    )
    .to_string();
    if output.truncated || output.bytes.len() > SETUP_OUTPUT_PREVIEW_BYTES {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("<truncated>");
    }
    text
}

fn format_timeout(timeout: Duration) -> String {
    if timeout.subsec_nanos() == 0 {
        format!("{}s", timeout.as_secs())
    } else {
        format!("{:.3}s", timeout.as_secs_f64())
    }
}

fn kill_process_tree(child: &mut Child) {
    let pid = child.id().to_string();

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .output();
        let _ = child.kill();
    }

    #[cfg(not(windows))]
    {
        let process_group = format!("-{pid}");
        let _ = Command::new("kill")
            .args(["-TERM", &process_group])
            .output();
        thread::sleep(Duration::from_millis(200));
        if child.try_wait().ok().flatten().is_none() {
            let _ = Command::new("kill")
                .args(["-KILL", &process_group])
                .output();
            let _ = child.kill();
        }
    }
}

fn safe_name(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    out.truncate(40);
    if out.is_empty() {
        "scenario".to_string()
    } else {
        out
    }
}
