use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ui::{self, Tone};

const REPO: &str = "lee-to/ai-tester";

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    /// Only report the latest version, don't download/replace.
    pub check: bool,
    /// Reinstall even if already on the latest version.
    pub force: bool,
    /// Install a specific tag instead of the latest release.
    pub tag: Option<String>,
}

pub fn update_command(opts: UpdateOptions) -> anyhow::Result<i32> {
    println!("{}", ui::header("ai-tester", "update"));

    let current_version = env!("CARGO_PKG_VERSION");
    let target = current_target().ok_or_else(|| {
        anyhow!(
            "unsupported platform: {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    println!("  {}", ui::kv("current", format!("v{current_version}")));
    println!("  {}", ui::kv("platform", target));

    let release = fetch_release(opts.tag.as_deref())?;
    let latest = release.tag.trim_start_matches('v').to_string();
    println!("  {}", ui::kv("latest", &release.tag));

    if !opts.force && !is_newer(&latest, current_version) {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Success),
            ui::paint("already up to date", Tone::Muted)
        );
        return Ok(0);
    }

    let asset_name = asset_name(target);
    let asset_url = release
        .asset_url(&asset_name)
        .ok_or_else(|| anyhow!("release {} has no asset `{asset_name}`", release.tag))?;

    if opts.check {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Info),
            ui::paint(
                &format!("update available: v{current_version} -> {}", release.tag),
                Tone::Strong
            )
        );
        println!("  {}", ui::kv("asset", &asset_name));
        return Ok(0);
    }

    let workdir = tempfile::Builder::new()
        .prefix("ai-tester-update-")
        .tempdir()?;
    let archive_path = workdir.path().join(&asset_name);

    println!(
        "  {}{}",
        ui::label("download"),
        ui::paint(&asset_name, Tone::Info)
    );
    download(&asset_url, &archive_path)?;

    verify_checksum(&release, &asset_name, &archive_path)?;

    let extract_dir = workdir.path().join("unpacked");
    std::fs::create_dir_all(&extract_dir)?;
    extract(&archive_path, &extract_dir)?;

    let new_binary = extract_dir.join(inner_binary_rel(target));
    if !new_binary.is_file() {
        bail!(
            "extracted archive missing expected binary at `{}`",
            new_binary.display()
        );
    }

    let current_exe = std::fs::canonicalize(std::env::current_exe()?)?;
    replace_binary(&new_binary, &current_exe)?;

    println!(
        "  {} {}",
        ui::paint("●", Tone::Success),
        ui::paint(
            &format!("updated to {} -> {}", release.tag, current_exe.display()),
            Tone::Strong
        )
    );
    Ok(0)
}

struct Release {
    tag: String,
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset_url(&self, name: &str) -> Option<String> {
        self.assets
            .iter()
            .find(|(asset, _)| asset == name)
            .map(|(_, url)| url.clone())
    }
}

fn fetch_release(tag: Option<&str>) -> anyhow::Result<Release> {
    let url = match tag {
        Some(tag) => format!("https://api.github.com/repos/{REPO}/releases/tags/{tag}"),
        None => format!("https://api.github.com/repos/{REPO}/releases/latest"),
    };
    let body = http_get(&url).context("fetch release metadata from GitHub")?;
    parse_release(&body)
}

fn parse_release(body: &str) -> anyhow::Result<Release> {
    let value: Value = serde_json::from_str(body).context("parse GitHub release JSON")?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!(
                "release JSON missing tag_name (message: {})",
                api_message(&value)
            )
        })?
        .to_string();
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?;
                    let url = item.get("browser_download_url").and_then(Value::as_str)?;
                    Some((name.to_string(), url.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Release { tag, assets })
}

fn api_message(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn verify_checksum(release: &Release, asset_name: &str, archive_path: &Path) -> anyhow::Result<()> {
    let Some(url) = release.asset_url("SHA256SUMS.txt") else {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint(
                "no SHA256SUMS.txt in release — skipping checksum",
                Tone::Muted
            )
        );
        return Ok(());
    };
    let sums = http_get(&url).context("download SHA256SUMS.txt")?;
    let Some(expected) = parse_sha256sums(&sums, asset_name) else {
        println!(
            "  {} {}",
            ui::paint("●", Tone::Warning),
            ui::paint("checksum for asset not listed — skipping", Tone::Muted)
        );
        return Ok(());
    };
    let actual = sha256_file(archive_path)?;
    if actual != expected {
        bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    println!(
        "  {} {}",
        ui::paint("●", Tone::Success),
        ui::paint("checksum verified", Tone::Muted)
    );
    Ok(())
}

fn parse_sha256sums(sums: &str, asset_name: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        // filenames may be prefixed with '*' (binary mode) in sha256sum output
        let name = parts.next()?.trim_start_matches('*');
        if name == asset_name {
            return Some(hash.to_lowercase());
        }
    }
    None
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn current_target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

fn archive_ext(target: &str) -> &'static str {
    if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

fn asset_name(target: &str) -> String {
    format!("ai-tester-{target}.{}", archive_ext(target))
}

fn inner_binary_rel(target: &str) -> PathBuf {
    let bin = if target.contains("windows") {
        "ai-tester.exe"
    } else {
        "ai-tester"
    };
    PathBuf::from(format!("ai-tester-{target}")).join(bin)
}

/// Compare dotted numeric versions; falls back to inequality when unparsable.
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest.trim_start_matches('v') != current.trim_start_matches('v'),
    }
}

fn parse_semver(value: &str) -> Option<(u64, u64, u64)> {
    let cleaned = value.trim().trim_start_matches('v');
    let core = cleaned.split(['-', '+']).next().unwrap_or(cleaned);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn http_get(url: &str) -> anyhow::Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            concat!("User-Agent: ai-tester-update/", env!("CARGO_PKG_VERSION")),
            "-H",
            "Accept: application/vnd.github+json",
            url,
        ])
        .output()
        .context("run curl")?;
    if !output.status.success() {
        bail!(
            "curl GET {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn download(url: &str, dest: &Path) -> anyhow::Result<()> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            concat!("User-Agent: ai-tester-update/", env!("CARGO_PKG_VERSION")),
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .output()
        .context("run curl download")?;
    if !output.status.success() {
        bail!(
            "download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let name = archive.to_string_lossy();
    let status = if name.ends_with(".zip") {
        Command::new("unzip")
            .arg("-oq")
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status()
            .context("run unzip")?
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
            .context("run tar")?
    };
    if !status.success() {
        bail!("failed to extract {}", archive.display());
    }
    Ok(())
}

fn replace_binary(new_binary: &Path, current_exe: &Path) -> anyhow::Result<()> {
    let dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve install directory"))?;
    let staging = dir.join(".ai-tester.update.tmp");

    std::fs::copy(new_binary, &staging).with_context(|| {
        format!(
            "copy new binary into {} (need write permission, try sudo)",
            dir.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    }

    // On Unix a running binary can be replaced via rename on the same filesystem.
    // On Windows the running file is locked, so move it aside first.
    #[cfg(windows)]
    {
        let backup = current_exe.with_extension("old");
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(current_exe, &backup)
            .with_context(|| format!("move aside current binary {}", current_exe.display()))?;
    }

    if let Err(err) = std::fs::rename(&staging, current_exe) {
        let _ = std::fs::remove_file(&staging);
        return Err(anyhow!(
            "install new binary to {}: {err}",
            current_exe.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_name_maps_target_to_archive() {
        assert_eq!(
            asset_name("x86_64-apple-darwin"),
            "ai-tester-x86_64-apple-darwin.tar.gz"
        );
        assert_eq!(
            asset_name("x86_64-pc-windows-msvc"),
            "ai-tester-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn inner_binary_uses_exe_on_windows() {
        assert_eq!(
            inner_binary_rel("aarch64-apple-darwin"),
            PathBuf::from("ai-tester-aarch64-apple-darwin/ai-tester")
        );
        assert_eq!(
            inner_binary_rel("x86_64-pc-windows-msvc"),
            PathBuf::from("ai-tester-x86_64-pc-windows-msvc/ai-tester.exe")
        );
    }

    #[test]
    fn is_newer_compares_semver() {
        assert!(is_newer("1.2.0", "1.1.0"));
        assert!(is_newer("v1.1.1", "1.1.0"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.0", "1.1.0"));
        assert!(is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn parse_release_extracts_tag_and_assets() {
        let json = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "ai-tester-x86_64-apple-darwin.tar.gz", "browser_download_url": "https://example/a"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example/sums"}
            ]
        }"#;
        let release = parse_release(json).expect("parses");
        assert_eq!(release.tag, "v1.2.0");
        assert_eq!(
            release.asset_url("ai-tester-x86_64-apple-darwin.tar.gz"),
            Some("https://example/a".to_string())
        );
        assert_eq!(
            release.asset_url("SHA256SUMS.txt"),
            Some("https://example/sums".to_string())
        );
        assert_eq!(release.asset_url("missing"), None);
    }

    #[test]
    fn parse_sha256sums_finds_asset() {
        let sums = "abc123  ai-tester-x86_64-apple-darwin.tar.gz\ndef456 *SHA256SUMS.txt\n";
        assert_eq!(
            parse_sha256sums(sums, "ai-tester-x86_64-apple-darwin.tar.gz"),
            Some("abc123".to_string())
        );
        assert_eq!(
            parse_sha256sums(sums, "SHA256SUMS.txt"),
            Some("def456".to_string())
        );
        assert_eq!(parse_sha256sums(sums, "nope"), None);
    }
}
