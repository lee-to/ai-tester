pub mod allowed_tools;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use allowed_tools::{tokenize_allowed_tools, AllowedTools, ParsedTool};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(rename = "argument-hint")]
    pub argument_hint: Option<String>,
    #[serde(rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    pub disable_model_invocation: Option<bool>,
    pub version: Option<String>,
    #[serde(rename = "token-budget")]
    pub token_budget: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSkillFile {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub body_hash: String,
    pub allowed_tools: AllowedTools,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillRecord {
    pub name: String,
    pub dir_path: PathBuf,
    pub skill_md_path: PathBuf,
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub body_hash: String,
    pub source_hash: String,
    pub allowed_tools_parsed: Vec<ParsedTool>,
    pub allowed_tools_raw: Vec<String>,
    pub token_budget: Option<f64>,
}

pub fn load_skill(skills_dir: impl AsRef<Path>, name: &str) -> anyhow::Result<SkillRecord> {
    let dir_path = skills_dir.as_ref().join(name);
    if !dir_path.is_dir() {
        bail!("Skill directory not found: {}", dir_path.display());
    }
    let skill_md_path = dir_path.join("SKILL.md");
    if !skill_md_path.is_file() {
        bail!("SKILL.md not found at {}", skill_md_path.display());
    }
    let parsed = parse_skill_md(&skill_md_path)?;
    if parsed.frontmatter.name != name {
        bail!(
            "Skill name mismatch: directory is `{}` but frontmatter.name is `{}`",
            name,
            parsed.frontmatter.name
        );
    }
    let source_hash = hash_skill_dir(&dir_path)?;
    let token_budget = parsed.frontmatter.token_budget;
    Ok(SkillRecord {
        name: name.to_string(),
        dir_path,
        skill_md_path,
        frontmatter: parsed.frontmatter,
        body: parsed.body,
        body_hash: parsed.body_hash,
        source_hash,
        allowed_tools_parsed: parsed.allowed_tools.parsed,
        allowed_tools_raw: parsed.allowed_tools.raw,
        token_budget,
    })
}

pub fn parse_skill_md(path: impl AsRef<Path>) -> anyhow::Result<ParsedSkillFile> {
    let path = path.as_ref();
    let raw =
        fs::read_to_string(path).with_context(|| format!("read SKILL.md at {}", path.display()))?;
    let (frontmatter_raw, body) = split_frontmatter(&raw).with_context(|| {
        format!(
            "SKILL.md at {} has no valid YAML frontmatter",
            path.display()
        )
    })?;
    let frontmatter: SkillFrontmatter = yaml_serde::from_str(frontmatter_raw)
        .with_context(|| format!("parse frontmatter in {}", path.display()))?;
    if frontmatter.name.trim().is_empty() || frontmatter.description.trim().is_empty() {
        bail!(
            "SKILL.md at {} missing required frontmatter (name, description)",
            path.display()
        );
    }
    if let Some(budget) = frontmatter.token_budget {
        if !budget.is_finite() || budget <= 0.0 {
            bail!("SKILL.md at {} has invalid `token-budget`", path.display());
        }
    }
    let body_hash = sha256_hex(body.as_bytes());
    let allowed_tools = tokenize_allowed_tools(frontmatter.allowed_tools.as_deref());
    Ok(ParsedSkillFile {
        frontmatter,
        body: body.to_string(),
        body_hash,
        allowed_tools,
    })
}

fn split_frontmatter(raw: &str) -> anyhow::Result<(&str, &str)> {
    let Some(rest) = raw.strip_prefix("---") else {
        bail!("missing opening frontmatter fence");
    };
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        bail!("missing closing frontmatter fence");
    };
    let frontmatter = &rest[..end];
    let body_start = end + "\n---".len();
    let body = rest[body_start..]
        .strip_prefix("\r\n")
        .or_else(|| rest[body_start..].strip_prefix('\n'))
        .unwrap_or(&rest[body_start..]);
    Ok((frontmatter, body))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn hash_skill_dir(dir: impl AsRef<Path>) -> anyhow::Result<String> {
    let dir = dir.as_ref();
    let mut files = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        let rel = file.strip_prefix(dir).unwrap_or(&file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(file)?);
        hasher.update([0]);
    }
    Ok(sha256_hex(&hasher.finalize()))
}
