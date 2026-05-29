use std::fs;
use std::path::{Path, PathBuf};

use crate::trace::TraceRecord;

#[derive(Debug, Clone)]
pub(crate) struct LoadedTrace {
    pub(crate) path: PathBuf,
    pub(crate) record: TraceRecord,
}

pub(crate) fn load_v2_traces() -> anyhow::Result<(bool, Vec<LoadedTrace>)> {
    let runs_dir = std::env::current_dir()?.join("runs");
    if !runs_dir.is_dir() {
        return Ok((false, Vec::new()));
    }

    let mut traces = Vec::new();
    for entry in walkdir::WalkDir::new(&runs_dir) {
        let entry = entry?;
        let is_json = entry.path().extension().is_some_and(|ext| ext == "json");
        if !entry.file_type().is_file() || !is_json {
            continue;
        }
        if let Some(trace) = read_v2_trace(entry.path())? {
            traces.push(trace);
        }
    }
    Ok((true, traces))
}

pub(crate) fn load_v2_trace_file(path: &Path) -> anyhow::Result<Option<LoadedTrace>> {
    read_v2_trace(path)
}

fn read_v2_trace(path: &Path) -> anyhow::Result<Option<LoadedTrace>> {
    let raw = fs::read_to_string(path)?;
    let record = match serde_json::from_str::<TraceRecord>(&raw) {
        Ok(record) if record.schema_version == "2.0.0" => record,
        _ => return Ok(None),
    };
    Ok(Some(LoadedTrace {
        path: path.to_path_buf(),
        record,
    }))
}
