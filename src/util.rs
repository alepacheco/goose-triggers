//! Small helpers shared by the commands.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// The home folder, found as goose finds it (the `dirs` crate): `$HOME` unless it is unset or
/// empty, else the user's entry in the system's user database (as far as this build can read
/// it: the static release reads `/etc/passwd`); `/` when neither gives one.
pub fn home_dir() -> PathBuf {
    home_found().unwrap_or_else(|| PathBuf::from("/"))
}

/// The home folder `$HOME` or the user database gives, as goose finds it; none when neither
/// does.
pub fn home_found() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), passwd_home)
}

/// The home folder from `$HOME`'s value, unless it is unset or empty, else from the user
/// database (`passwd`).
fn home_from(
    home: Option<std::ffi::OsString>,
    passwd: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match home {
        Some(h) if !h.is_empty() => Some(PathBuf::from(h)),
        _ => passwd(),
    }
}

/// The home folder the user database gives this process's user.
fn passwd_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: getpwuid_r fills `pw` with pointers into `buf`, which outlives their use here.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pw,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        );
        if rc != 0 || out.is_null() || pw.pw_dir.is_null() {
            return None;
        }
        let dir = CStr::from_ptr(pw.pw_dir).to_bytes();
        (!dir.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(dir)))
    }
}

/// `$GOOSE_PATH_ROOT` when it is an absolute path: goose then keeps its config, data and
/// settings under it, not under the home folder.
pub fn goose_path_root() -> Option<PathBuf> {
    std::env::var_os("GOOSE_PATH_ROOT")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
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

/// Creates a folder (and its missing parents) readable by this user only; one that exists is
/// left as it is.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
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

/// A text safe to print on a terminal: control characters (escape sequences included) shown
/// as `\u{..}` escapes, newlines and tabs as `\n` and `\t`.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || invisible(c) => out.push_str(&c.escape_unicode().to_string()),
            c => out.push(c),
        }
    }
    out
}

/// Characters that change how text around them shows without showing themselves:
/// direction overrides and isolates, zero-width characters, the byte-order mark.
fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064
        | 0x2066..=0x206F | 0xFEFF)
}

/// Shortens a text to `max` characters for one-line displays, cleaned for the terminal.
pub fn short(s: &str, max: usize) -> String {
    let one_line = clean(s);
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// A tool call's input (canonical JSON text) for a one-line display: an object's fields
/// shortest value first, so short fields such as a file path are not cut off behind a long
/// one. `goose-triggers log <run>` shows the whole call.
pub fn call_line(input: &str, max: usize) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(input) else {
        return short(input, max);
    };
    let mut fields: Vec<(String, String)> = map
        .into_iter()
        .map(|(k, v)| (serde_json::to_string(&k).unwrap_or_default(), v.to_string()))
        .collect();
    fields.sort_by_key(|(_, v)| v.len());
    let body: Vec<String> = fields
        .into_iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect();
    short(&format!("{{{}}}", body.join(",")), max)
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

/// Runs a short command to its end, reading its output whole, and kills it if it takes over
/// `limit`: its output, or None when it took too long.
pub fn output_within(
    cmd: &mut std::process::Command,
    limit: std::time::Duration,
) -> std::io::Result<Option<std::process::Output>> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    wait_output(&mut child, limit)
}

/// Reads a started command's piped output whole and waits for it to end, killing it if it
/// takes over `limit`: its output, or None when it took too long.
pub fn wait_output(
    child: &mut std::process::Child,
    limit: std::time::Duration,
) -> std::io::Result<Option<std::process::Output>> {
    wait_output_unless(child, limit, &|| false)
}

/// `wait_output`, which also kills the command and gives None as soon as `stop` says so
/// (asked every 50 ms).
pub fn wait_output_unless(
    child: &mut std::process::Child,
    limit: std::time::Duration,
    stop: &dyn Fn() -> bool,
) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // Read on threads of their own, so a full pipe never stops the command.
    let out_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > limit || stop() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(Some(std::process::Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    }))
}

/// `goose --version`: checks that the goose binary runs, and says which goose it is, with
/// `env` set and `hide` (the webhook secret's variable) removed from its environment. `hint`
/// says how to point goose-triggers at another goose.
pub fn goose_version(
    goose: &str,
    env: &[(&str, &Path)],
    hide: Option<&str>,
    hint: &str,
) -> anyhow::Result<String> {
    let mut cmd = std::process::Command::new(goose);
    cmd.arg("--version");
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(var) = hide {
        cmd.env_remove(var);
    }
    let out = match output_within(&mut cmd, std::time::Duration::from_secs(60)) {
        Ok(Some(out)) => out,
        Ok(None) => anyhow::bail!("`{goose} --version` took over 60 s"),
        Err(e) => anyhow::bail!(
            "running `{goose} --version`: {e}: install goose (https://goose-docs.ai/docs/getting-started/installation) or {hint}"
        ),
    };
    if !out.status.success() {
        anyhow::bail!("`{goose} --version` failed ({}): {hint}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn home_is_home_unless_unset_or_empty() {
        let db = || Some(PathBuf::from("/home/db"));
        assert_eq!(home_from(Some("/h".into()), db), Some(PathBuf::from("/h")));
        assert_eq!(
            home_from(Some("".into()), db),
            Some(PathBuf::from("/home/db"))
        );
        assert_eq!(home_from(None, db), Some(PathBuf::from("/home/db")));
        assert_eq!(home_from(None, || None), None);
    }

    #[test]
    fn the_user_database_gives_a_home_folder() {
        // The fallback when HOME is unset or empty, as goose has it (a container's user may
        // have no entry: then there is none).
        if let Some(home) = passwd_home() {
            assert!(home.is_absolute(), "{}", home.display());
        }
    }

    #[test]
    fn a_missing_goose_is_named_with_the_hint() {
        let e = goose_version(
            "/nonexistent/goose",
            &[],
            None,
            "set `goose:` in the config",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("/nonexistent/goose") && e.contains("set `goose:` in the config"),
            "{e}"
        );
        // A command that runs but is not goose (it fails on --version) is refused too.
        let e = goose_version("false", &[], None, "set `goose:`")
            .unwrap_err()
            .to_string();
        assert!(e.contains("`false --version` failed"), "{e}");
    }

    #[test]
    fn output_within_reads_big_output_and_stops_slow_commands() {
        let mut big = std::process::Command::new("sh");
        big.args(["-c", "head -c 300000 /dev/zero; echo err >&2"]);
        let out = output_within(&mut big, std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        assert_eq!(out.stdout.len(), 300000);
        assert_eq!(out.stderr, b"err\n");
        let mut slow = std::process::Command::new("sleep");
        slow.arg("30");
        let start = std::time::Instant::now();
        assert!(
            output_within(&mut slow, std::time::Duration::from_millis(200))
                .unwrap()
                .is_none()
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

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
    fn clean_escapes_control_characters() {
        assert_eq!(clean("a\u{1b}[2Jb\nc\td"), "a\\u{1b}[2Jb\\nc\\td");
        assert_eq!(clean("rm \u{202e}fdp.exe"), "rm \\u{202e}fdp.exe");
        assert_eq!(short("abcdef", 4), "abc…");
    }

    #[test]
    fn call_line_shows_short_fields_first() {
        let input = canonical_json(&json!({"content": "x".repeat(300), "path": "/etc/passwd"}));
        let line = call_line(&input, 60);
        assert!(
            line.starts_with(r#"{"path":"/etc/passwd","content":"xxx"#),
            "{line}"
        );
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
