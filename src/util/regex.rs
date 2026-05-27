use regex::{Regex, RegexBuilder};

pub fn compile_pattern(pattern: &str) -> anyhow::Result<Regex> {
    let mut flags = String::new();
    let mut body = pattern;

    if let Some(rest) = pattern.strip_prefix("(?") {
        if let Some(end) = rest.find(')') {
            let maybe_flags = &rest[..end];
            if !maybe_flags.is_empty() && maybe_flags.chars().all(|c| matches!(c, 'i' | 'm' | 's'))
            {
                flags.push_str(maybe_flags);
                body = &rest[end + 1..];
            }
        }
    }

    let mut builder = RegexBuilder::new(body);
    builder.case_insensitive(flags.contains('i'));
    builder.multi_line(flags.contains('m'));
    builder.dot_matches_new_line(flags.contains('s'));
    Ok(builder.build()?)
}
