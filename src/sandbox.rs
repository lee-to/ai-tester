use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use walkdir::WalkDir;

use crate::scenario::{FixtureFile, Fixtures};

#[derive(Debug, Clone, Default)]
pub struct SandboxOptions {
    pub keep: bool,
    pub skill: Option<SkillInstall>,
}

#[derive(Debug, Clone)]
pub struct SkillInstall {
    pub name: String,
    pub dir_path: PathBuf,
}

#[derive(Debug, Clone)]
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
        fs::remove_dir_all(&self.path)
            .with_context(|| format!("remove sandbox {}", self.path.display()))
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
    let raw = temp.keep();
    let base = fs::canonicalize(&raw)?;
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
            run_shell(&base, command)?;
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

fn run_shell(cwd: &Path, command: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    let mut child = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    };
    #[cfg(not(windows))]
    let mut child = {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command]);
        cmd
    };
    let output = child.current_dir(cwd).output()?;
    if !output.status.success() {
        bail!(
            "setup command failed: `{command}` - {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
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
