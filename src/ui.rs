use std::io::IsTerminal;
use std::sync::OnceLock;

use owo_colors::OwoColorize;

#[derive(Debug, Clone, Copy)]
pub enum Tone {
    Accent,
    Info,
    Muted,
    Success,
    Warning,
    Error,
    Strong,
}

pub fn title(text: &str) -> String {
    paint(text, Tone::Accent)
}

pub fn header(title: &str, subtitle: &str) -> String {
    if subtitle.is_empty() {
        format!(
            "{} {}",
            paint("●", Tone::Accent),
            paint(title, Tone::Strong)
        )
    } else {
        format!(
            "{} {} {}",
            paint("●", Tone::Accent),
            paint(title, Tone::Strong),
            paint(subtitle, Tone::Muted)
        )
    }
}

pub fn section(text: &str) -> String {
    format!("{} {}", paint("▪", Tone::Accent), paint(text, Tone::Strong))
}

pub fn label(text: &str) -> String {
    paint(&format!("{text:<11}"), Tone::Muted)
}

pub fn kv(label_text: &str, value: impl std::fmt::Display) -> String {
    format!("{}{}", label(label_text), value)
}

pub fn fit_value(value: impl std::fmt::Display, reserved_cols: usize) -> String {
    let value = value.to_string();
    let max_chars = terminal_columns()
        .saturating_sub(reserved_cols)
        .clamp(24, 120);
    truncate_middle(&value, max_chars)
}

pub fn status(text: &str, pass: bool) -> String {
    if pass {
        paint(text, Tone::Success)
    } else {
        paint(text, Tone::Error)
    }
}

pub fn tag(text: &str, tone: Tone) -> String {
    paint(&format!("[{text}]"), tone)
}

pub fn paint(text: &str, tone: Tone) -> String {
    if !colors_enabled() {
        return text.to_string();
    }

    match tone {
        Tone::Accent => text.bright_cyan().bold().to_string(),
        Tone::Info => text.bright_blue().bold().to_string(),
        Tone::Muted => text.bright_black().to_string(),
        Tone::Success => text.bright_green().bold().to_string(),
        Tone::Warning => text.bright_yellow().bold().to_string(),
        Tone::Error => text.bright_red().bold().to_string(),
        Tone::Strong => text.bold().to_string(),
    }
}

fn terminal_columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|columns| *columns > 0)
        .unwrap_or(100)
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return "...".chars().take(max_chars).collect();
    }
    let keep = max_chars - 3;
    let head = keep / 2;
    let tail = keep - head;
    let start = value.chars().take(head).collect::<String>();
    let end = value
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{start}...{end}")
}

fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if truthy_env("AI_TESTER_FORCE_COLOR") {
            return true;
        }
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

fn truthy_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && value != "0" && value != "false" && value != "no"
    })
}
