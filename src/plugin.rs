//! goose-triggers' own goose plugin: the hooks that hold calls.
//!
//! goose loads plugins from `<working folder>/.agents/plugins/<name>/` when a session starts
//! in that folder. goose-triggers writes its plugin into each rule's working folder, so the
//! hooks apply where goose-triggers runs goose. Its command does nothing in sessions that
//! goose-triggers did not start (no `GOOSE_TRIGGERS_RUN` in the environment).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::util::sh_quote;

pub const PLUGIN_NAME: &str = "goose-triggers";

pub fn plugin_root(working_dir: &Path) -> PathBuf {
    working_dir
        .join(".agents")
        .join("plugins")
        .join(PLUGIN_NAME)
}

/// The hook command: the goose-triggers binary at `exe`, only inside goose-triggers' runs.
pub fn hook_command(exe: &Path) -> String {
    format!(
        "[ -n \"${}\" ] || exit 0; exec {} hook",
        crate::hook::ENV_RUN,
        sh_quote(&exe.to_string_lossy())
    )
}

pub fn hooks_json(exe: &Path) -> Value {
    let cmd = hook_command(exe);
    json!({
        "hooks": {
            "SessionStart": [
                {"hooks": [{"type": "command", "command": cmd, "timeout": 30}]}
            ],
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": cmd, "timeout": 30, "on_failure": "block"}]}
            ]
        }
    })
}

/// Writes (or rewrites) the plugin into a working folder.
pub fn install(working_dir: &Path, exe: &Path) -> Result<PathBuf> {
    let root = plugin_root(working_dir);
    std::fs::create_dir_all(root.join("hooks"))
        .with_context(|| format!("creating {}", root.display()))?;
    let manifest = json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Holds goose-triggers runs at the tool calls their rule marks, until the user approves or rejects them."
    });
    write_if_changed(
        &root.join("plugin.json"),
        &serde_json::to_string_pretty(&manifest)?,
    )?;
    write_if_changed(
        &root.join("hooks").join("hooks.json"),
        &serde_json::to_string_pretty(&hooks_json(exe))?,
    )?;
    Ok(root)
}

fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    let text = format!("{text}\n");
    if std::fs::read_to_string(path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// goose's config folder: `$GOOSE_PATH_ROOT/config` when that is set, else
/// `$XDG_CONFIG_HOME/goose` or `~/.config/goose`.
pub fn goose_config_dir() -> PathBuf {
    match std::env::var_os("GOOSE_PATH_ROOT") {
        Some(root) if Path::new(&root).is_absolute() => PathBuf::from(root).join("config"),
        _ => crate::util::config_home().join("goose"),
    }
}

/// Refuses a working folder where goose's settings disable goose-triggers' plugin: its
/// hooks would not run, so nothing would be held.
pub fn preflight(working_dir: &Path) -> Result<()> {
    let root = plugin_root(working_dir);
    let config = goose_config_dir().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&config) {
        if let Ok(v) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
            let key = serde_yaml_ng::Value::String(root.to_string_lossy().into_owned());
            let enabled = v
                .get("plugins")
                .and_then(|p| p.get(&key))
                .and_then(|e| e.get("enabled"))
                .and_then(|e| e.as_bool());
            if enabled == Some(false) {
                bail!(
                    "goose's config ({}) disables the plugin at {}: set its `enabled` to true, or nothing could be held",
                    config.display(),
                    root.display()
                );
            }
        }
    }
    let user_settings = match std::env::var_os("GOOSE_PATH_ROOT") {
        Some(r) if Path::new(&r).is_absolute() => {
            PathBuf::from(r).join(".config/goose/settings.json")
        }
        _ => crate::util::home_dir().join(".config/goose/settings.json"),
    };
    for settings in [
        user_settings,
        working_dir.join(".config/goose/settings.json"),
        working_dir.join(".config/goose/settings.local.json"),
    ] {
        let Ok(text) = std::fs::read_to_string(&settings) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let disabled = v
            .get("disabledPlugins")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|n| n.as_str() == Some(PLUGIN_NAME)));
        if disabled {
            bail!(
                "{} lists {PLUGIN_NAME} in disabledPlugins: remove it, or nothing could be held",
                settings.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_command_runs_only_in_goose_triggers_runs() {
        let cmd = hook_command(Path::new("/opt/it's here/goose-triggers"));
        assert_eq!(
            cmd,
            r#"[ -n "$GOOSE_TRIGGERS_RUN" ] || exit 0; exec '/opt/it'\''s here/goose-triggers' hook"#
        );
        let v = hooks_json(Path::new("/x"));
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["on_failure"],
            "block"
        );
        assert!(v["hooks"]["PreToolUse"][0].get("matcher").is_none());
    }

    #[test]
    fn install_writes_the_plugin_and_preflight_spots_a_disabled_one() {
        let dir = std::env::temp_dir().join(format!(
            "gt-plugin-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        let root = install(&dir, Path::new("/bin/gt")).unwrap();
        assert!(root.join("plugin.json").is_file());
        assert!(root.join("hooks/hooks.json").is_file());
        preflight(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".config/goose")).unwrap();
        std::fs::write(
            dir.join(".config/goose/settings.json"),
            r#"{"disabledPlugins":["goose-triggers"]}"#,
        )
        .unwrap();
        assert!(preflight(&dir).is_err());
    }
}
