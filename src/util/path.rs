use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};

pub fn strip_windows_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy();
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = path.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

pub fn canonicalize_existing(path: &Path) -> anyhow::Result<PathBuf> {
    fs::canonicalize(path)
        .map(|path| strip_windows_verbatim_prefix(&path))
        .with_context(|| format!("canonicalize {}", path.display()))
}

pub fn resolve_existing_inside(sandbox_root: &Path, requested: &Path) -> anyhow::Result<PathBuf> {
    let sandbox = canonicalize_existing(sandbox_root)?;
    let candidate = candidate_path(&sandbox, requested);
    if !candidate.exists() {
        let checked = resolve_against_existing_ancestor(&candidate)?;
        ensure_inside(&sandbox, &checked)?;
    }
    let resolved = canonicalize_existing(&candidate)?;
    ensure_inside(&sandbox, &resolved)?;
    Ok(resolved)
}

pub fn resolve_write_target_inside(
    sandbox_root: &Path,
    requested: &Path,
) -> anyhow::Result<PathBuf> {
    let sandbox = canonicalize_existing(sandbox_root)?;
    let candidate = candidate_path(&sandbox, requested);
    if candidate.exists() {
        let resolved = canonicalize_existing(&candidate)?;
        ensure_inside(&sandbox, &resolved)?;
        return Ok(resolved);
    }

    let resolved = resolve_against_existing_ancestor(&candidate)?;
    ensure_inside(&sandbox, &resolved)?;
    Ok(resolved)
}

fn resolve_against_existing_ancestor(candidate: &Path) -> anyhow::Result<PathBuf> {
    let (existing_parent, missing_components) = nearest_existing_ancestor(candidate)?;
    let mut resolved = canonicalize_existing(&existing_parent)?;
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_path_lexical(&resolved))
}

pub fn candidate_path(sandbox_root: &Path, requested: &Path) -> PathBuf {
    if requested.is_absolute() {
        strip_windows_verbatim_prefix(requested)
    } else {
        sandbox_root.join(requested)
    }
}

pub fn normalize_path_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

pub fn path_is_within(path: &Path, root: &Path) -> bool {
    let path_components = path_component_keys(path);
    let root_components = path_component_keys(root);
    path_components.len() >= root_components.len()
        && path_components
            .iter()
            .zip(root_components.iter())
            .all(|(path, root)| path == root)
}

fn ensure_inside(sandbox: &Path, resolved: &Path) -> anyhow::Result<()> {
    if path_is_within(resolved, sandbox) {
        Ok(())
    } else {
        bail!(
            "path escapes sandbox: {} is outside {}",
            resolved.display(),
            sandbox.display()
        )
    }
}

fn nearest_existing_ancestor(path: &Path) -> anyhow::Result<(PathBuf, Vec<OsString>)> {
    let mut cursor = path.to_path_buf();
    let mut missing = Vec::new();
    while !cursor.exists() {
        if let Some(name) = cursor.file_name() {
            missing.push(name.to_os_string());
        }
        let Some(parent) = cursor.parent() else {
            bail!("path has no existing parent: {}", path.display());
        };
        if parent == cursor {
            bail!("path has no existing parent: {}", path.display());
        }
        cursor = parent.to_path_buf();
    }
    Ok((cursor, missing))
}

fn path_component_keys(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy().to_string();
            if cfg!(windows) {
                text.to_ascii_lowercase()
            } else {
                text
            }
        })
        .collect()
}
