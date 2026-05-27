use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedTool {
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedTools {
    pub raw: Vec<String>,
    pub parsed: Vec<ParsedTool>,
}

pub fn tokenize_allowed_tools(input: Option<&str>) -> AllowedTools {
    let raw = split_top_level(input.unwrap_or_default());
    let parsed = raw.iter().map(|item| parse_tool(item)).collect();
    AllowedTools { raw, parsed }
}

fn split_top_level(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (idx, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                push_trimmed(&mut out, &input[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    push_trimmed(&mut out, &input[start..]);
    out
}

fn push_trimmed(out: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

fn parse_tool(item: &str) -> ParsedTool {
    let Some(open) = item.find('(') else {
        return ParsedTool {
            name: item.trim().to_string(),
            scopes: Vec::new(),
        };
    };
    let close = item.rfind(')').unwrap_or(item.len());
    let name = item[..open].trim().to_string();
    let scopes = item[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    ParsedTool { name, scopes }
}
