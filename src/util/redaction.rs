use regex::Regex;
use serde_json::{Map, Value};

const REDACTED: &str = "<redacted>";

#[derive(Debug, Clone)]
pub struct Redactor {
    known_values: Vec<String>,
    auth_re: Regex,
    key_value_re: Regex,
    url_re: Regex,
}

impl Redactor {
    pub fn new(mut known_values: Vec<String>) -> Self {
        known_values.retain(|value| !value.trim().is_empty());
        known_values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        known_values.dedup();
        Self {
            known_values,
            auth_re: Regex::new(r"(?i)(\bauthorization\s*[:=]\s*)[^\r\n]+")
                .expect("authorization redaction regex compiles"),
            key_value_re: Regex::new(
                r#"(?i)\b(?P<key>api[_-]?key|token|secret|password|credential|key)\s*(?P<sep>[:=])\s*(?P<value>"[^"]*"|'[^']*'|[^\s,;]+)"#,
            )
            .expect("key/value redaction regex compiles"),
            url_re: Regex::new(
                r#"(?P<prefix>\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s"'?#]+(?:/[^\s"'?#]*)?\?)(?P<query>[^\s"'#]*)(?P<fragment>#[^\s"']*)?"#,
            )
            .expect("url redaction regex compiles"),
        }
    }

    pub fn redact_line(&self, line: &str) -> String {
        if let Ok(mut value) = serde_json::from_str::<Value>(line) {
            self.redact_json_value(&mut value);
            return serde_json::to_string(&value).unwrap_or_else(|_| self.redact_text(line));
        }
        self.redact_text(line)
    }

    fn redact_json_value(&self, value: &mut Value) {
        match value {
            Value::Object(object) => self.redact_json_object(object),
            Value::Array(values) => {
                for value in values {
                    self.redact_json_value(value);
                }
            }
            Value::String(text) => {
                *text = self.redact_text(text);
            }
            _ => {}
        }
    }

    fn redact_json_object(&self, object: &mut Map<String, Value>) {
        let name_is_secret = object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(is_secret_like_key);
        for (key, value) in object.iter_mut() {
            if is_secret_like_key(key) || (key == "value" && name_is_secret) {
                *value = Value::String(REDACTED.to_string());
            } else {
                self.redact_json_value(value);
            }
        }
    }

    fn redact_text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in &self.known_values {
            out = out.replace(value, REDACTED);
        }
        out = self
            .auth_re
            .replace_all(&out, format!("${{1}}{REDACTED}"))
            .to_string();
        out = self
            .key_value_re
            .replace_all(&out, format!("${{key}}${{sep}}{REDACTED}"))
            .to_string();
        redact_url_query_with(&self.url_re, &out)
    }
}

pub fn redact_url_query(url: &str) -> String {
    let re = Regex::new(
        r#"(?P<prefix>\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s"'?#]+(?:/[^\s"'?#]*)?\?)(?P<query>[^\s"'#]*)(?P<fragment>#[^\s"']*)?"#,
    )
    .expect("url redaction regex compiles");
    redact_url_query_with(&re, url)
}

fn redact_url_query_with(re: &Regex, text: &str) -> String {
    re.replace_all(text, |caps: &regex::Captures<'_>| {
        let fragment = caps.name("fragment").map(|m| m.as_str()).unwrap_or("");
        format!("{}{}{}", &caps["prefix"], REDACTED, fragment)
    })
    .to_string()
}

pub fn is_secret_like_key(key: &str) -> bool {
    let compact = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>();
    if matches!(
        compact.as_str(),
        "authorization" | "apikey" | "token" | "secret" | "password" | "credential" | "key"
    ) {
        return true;
    }
    key.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .any(|part| {
            matches!(
                part.as_str(),
                "authorization"
                    | "api"
                    | "apikey"
                    | "token"
                    | "secret"
                    | "password"
                    | "credential"
                    | "key"
            )
        })
}
