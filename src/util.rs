//! Small helpers shared by the commands.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(p) if !p.is_empty() && Path::new(&p).is_absolute() => PathBuf::from(p),
        _ => home_dir().join(".config"),
    }
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(p) if !p.is_empty() && Path::new(&p).is_absolute() => PathBuf::from(p),
        _ => home_dir().join(".local").join("share"),
    }
}

/// Expands a leading `~/` and makes a relative path relative to `base`.
pub fn resolve_path(raw: &str, base: &Path) -> PathBuf {
    let p = if raw == "~" {
        home_dir()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(raw)
    };
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A Unix time as local `YYYY-MM-DD HH:MM:SS`.
pub fn fmt_time(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(secs, 0) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => secs.to_string(),
    }
}

/// One JSON text per value, whatever the key order or spacing it came with: object keys
/// sorted, no whitespace. Two tool inputs are the same call when their canonical texts match.
pub fn canonical_json(v: &Value) -> String {
    // serde_json's map is ordered by key unless the preserve_order feature is on, which
    // this crate does not enable; the test below guards that.
    serde_json::to_string(v).unwrap_or_default()
}

/// Compares two secrets in time that does not depend on where they first differ.
pub fn same_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Quotes a string for `sh`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Shortens a text to `max` characters for one-line displays.
pub fn short(s: &str, max: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// goose providers that hand the whole turn to another agent CLI, which runs its own tools:
/// goose's hooks never see those calls, so no hold can be enforced with them.
pub fn is_cli_agent_provider(name: &str) -> bool {
    const CLI_AGENTS: &[&str] = &[
        "claude-code",
        "claude-acp",
        "codex",
        "codex-acp",
        "copilot-acp",
        "cursor-agent",
        "gemini-cli",
        "amp-acp",
        "pi-acp",
    ];
    CLI_AGENTS.contains(&name) || name.ends_with("-acp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_ignores_key_order_and_spacing() {
        let a: Value = serde_json::from_str(r#"{"b": 1, "a": {"y": [1, 2], "x": "s"}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":"s","y":[1,2]},"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), r#"{"a":{"x":"s","y":[1,2]},"b":1}"#);
        assert_ne!(
            canonical_json(&a),
            canonical_json(&json!({"a": {"x": "s", "y": [2, 1]}, "b": 1}))
        );
    }

    #[test]
    fn same_secret_compares_whole_values() {
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd"));
        assert!(!same_secret(b"abc", b"abcd"));
        assert!(!same_secret(b"", b"a"));
    }

    #[test]
    fn sh_quote_survives_single_quotes() {
        assert_eq!(sh_quote("/a b/it's"), r"'/a b/it'\''s'");
    }

    #[test]
    fn cli_agent_providers_are_recognised() {
        assert!(is_cli_agent_provider("claude-code"));
        assert!(is_cli_agent_provider("my-agent-acp"));
        assert!(!is_cli_agent_provider("openai"));
        assert!(!is_cli_agent_provider("anthropic"));
        assert!(!is_cli_agent_provider("ollama"));
    }

    #[test]
    fn resolve_path_expands_home_and_relative() {
        let base = Path::new("/etc/gt");
        assert_eq!(resolve_path("/x/y", base), PathBuf::from("/x/y"));
        assert_eq!(
            resolve_path("r/y.yaml", base),
            PathBuf::from("/etc/gt/r/y.yaml")
        );
        assert_eq!(resolve_path("~/in", base), home_dir().join("in"));
    }
}
