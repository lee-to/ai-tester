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

pub fn label(text: &str) -> String {
    paint(&format!("{text:<11}"), Tone::Muted)
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
