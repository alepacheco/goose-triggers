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
            "UserPromptSubmit": [
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

/// The other plugins in a working folder: goose takes every folder there for a plugin; an
/// empty one holds nothing for it to load.
pub fn other_plugins(working_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(working_dir.join(".agents").join("plugins")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| std::fs::read_dir(e.path()).map_or(true, |mut d| d.next().is_some()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != PLUGIN_NAME)
        .collect();
    names.sort();
    names
}

/// goose's config folder: `$GOOSE_PATH_ROOT/config` when that is set, else
/// `$XDG_CONFIG_HOME/goose` or `~/.config/goose`.
pub fn goose_config_dir() -> PathBuf {
    match std::env::var_os("GOOSE_PATH_ROOT") {
        Some(root) if Path::new(&root).is_absolute() => PathBuf::from(root).join("config"),
        _ => crate::util::config_home().join("goose"),
    }
}

/// A plugin settings file as goose 1.57.0 reads it (the open-plugins format): a file that does
/// not parse as this is passed over whole.
#[derive(serde::Deserialize)]
struct Settings {
    #[serde(default, rename = "enabledPlugins")]
    enabled: Vec<String>,
    #[serde(default, rename = "disabledPlugins")]
    disabled: Vec<String>,
}

/// goose's plugin settings files for a working folder, in the order goose 1.57.0 reads them,
/// the first that names a plugin deciding: the folder's `.config/goose/settings.local.json`,
/// its `.config/goose/settings.json`, then the user's, `<user>/.config/goose/settings.json`
/// (`user` is `$GOOSE_PATH_ROOT` when that is set, else the home folder).
fn settings_files(working_dir: &Path, user: &Path) -> Vec<PathBuf> {
    vec![
        working_dir.join(".config/goose/settings.local.json"),
        working_dir.join(".config/goose/settings.json"),
        user.join(".config/goose/settings.json"),
    ]
}

/// goose's `plugins` setting as goose 1.57.0 reads it (`get_param`): the `PLUGINS` variable
/// (`env`) when it is set, read as JSON first; else the last of goose's config files
/// (`config_layers`, their texts in goose's merge order) that has `plugins:`, which gives the
/// whole map, a text value there read as JSON. None when no source has it.
fn plugins_setting(env: Option<&str>, config_layers: &[String]) -> Option<Value> {
    if let Some(v) = env {
        return Some(env_value(v));
    }
    let v = config_layers
        .iter()
        .rev()
        .filter_map(|t| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t).ok())
        .find_map(|v| v.get("plugins").cloned())?;
    match v.as_str() {
        Some(text) => Some(env_value(text)),
        None => serde_json::to_value(&v).ok(),
    }
}

/// A setting's text as goose reads it (`parse_env_value`): JSON, else `true` or `false`, a
/// number, or the text.
fn env_value(v: &str) -> Value {
    if let Ok(j) = serde_json::from_str(v) {
        return j;
    }
    let t = v.trim();
    match t.to_lowercase().as_str() {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::from(i);
    }
    if let Some(n) = t.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
        return Value::Number(n);
    }
    Value::String(v.to_string())
}

/// Whether goose's `plugins` setting turns off the plugin at `root`: goose reads it as a map of
/// plugin folders to `{enabled: <bool>}` (a setting it cannot read so is an empty map, which
/// turns nothing off), and drops a plugin whose entry has `enabled: false`.
fn disabled_in_config(setting: Option<Value>, root: &Path) -> bool {
    #[derive(serde::Deserialize)]
    struct Entry {
        enabled: bool,
    }
    setting
        .and_then(|v| serde_json::from_value::<std::collections::HashMap<String, Entry>>(v).ok())
        .and_then(|m| m.get(root.to_string_lossy().as_ref()).map(|e| !e.enabled))
        .unwrap_or(false)
}

/// The settings file that turns goose-triggers' plugin off, as goose decides it: the first
/// file (in `settings_files` order) that lists it, if it lists it under `disabledPlugins`.
fn disabled_by(files: &[PathBuf]) -> Option<PathBuf> {
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let Ok(s) = serde_json::from_str::<Settings>(&text) else {
            continue;
        };
        if s.disabled.iter().any(|n| n == PLUGIN_NAME) {
            return Some(f.clone());
        }
        if s.enabled.iter().any(|n| n == PLUGIN_NAME) {
            return None;
        }
    }
    None
}

/// Refuses a working folder where goose's settings turn goose-triggers' plugin off, as goose
/// 1.57.0 reads them: its hooks would not run, so nothing would be held. goose reads two kinds:
/// its `plugins` setting (the `PLUGINS` variable, else `plugins:` in its config files), keyed
/// by the plugin's folder, with `enabled: false`; and `disabledPlugins` (and `enabledPlugins`)
/// lists in its plugin settings files.
pub fn preflight(working_dir: &Path) -> Result<()> {
    let user = match std::env::var_os("GOOSE_PATH_ROOT") {
        Some(r) if Path::new(&r).is_absolute() => PathBuf::from(r),
        _ => crate::util::home_dir(),
    };
    preflight_with(
        working_dir,
        std::env::var("PLUGINS").ok().as_deref(),
        &crate::runner::goose_config_layers(),
        &user,
    )
}

/// `preflight` with goose's `PLUGINS` variable (`plugins_env`), its config files' texts
/// (`config_layers`) and the folder that holds the user's `.config/goose/settings.json`
/// (`user`) given.
fn preflight_with(
    working_dir: &Path,
    plugins_env: Option<&str>,
    config_layers: &[String],
    user: &Path,
) -> Result<()> {
    // goose keys its config by the folder it runs in, as the system names it.
    let wd = std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    let root = plugin_root(&wd);
    if disabled_in_config(plugins_setting(plugins_env, config_layers), &root) {
        bail!(
            "goose's plugins setting ({}) turns off the plugin at {}: set its `enabled` to true, or nothing could be held",
            if plugins_env.is_some() {
                "the PLUGINS variable".to_string()
            } else {
                format!(
                    "`plugins:` in {}, or a file goose merges with it",
                    goose_config_dir().join("config.yaml").display()
                )
            },
            root.display()
        );
    }
    if let Some(f) = disabled_by(&settings_files(&wd, user)) {
        bail!(
            "{} lists {PLUGIN_NAME} in disabledPlugins: remove it, or nothing could be held",
            f.display()
        );
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
        // goose's own settings given, not read from this machine's.
        let user = dir.join("home");
        preflight_with(&dir, None, &[], &user).unwrap();
        assert!(other_plugins(&dir).is_empty());
        // An empty folder holds no plugin.
        std::fs::create_dir_all(dir.join(".agents/plugins/empty")).unwrap();
        assert!(other_plugins(&dir).is_empty());
        std::fs::create_dir_all(dir.join(".agents/plugins/other/hooks")).unwrap();
        assert_eq!(other_plugins(&dir), ["other"]);
        std::fs::create_dir_all(dir.join(".config/goose")).unwrap();
        std::fs::write(
            dir.join(".config/goose/settings.json"),
            r#"{"disabledPlugins":["goose-triggers"]}"#,
        )
        .unwrap();
        assert!(preflight_with(&dir, None, &[], &user).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_settings_file_that_names_the_plugin_decides_as_in_goose() {
        let dir = std::env::temp_dir().join(format!(
            "gt-plugin-settings-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, text: &str| {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let (local, project, user) = (dir.join("l.json"), dir.join("p.json"), dir.join("u.json"));
        let files = [local.clone(), project.clone(), user.clone()];
        assert_eq!(disabled_by(&files), None);
        file("u.json", r#"{"disabledPlugins":["goose-triggers"]}"#);
        assert_eq!(disabled_by(&files), Some(user.clone()));
        // A file nearer the folder that enables it wins, as in goose.
        file("p.json", r#"{"enabledPlugins":["goose-triggers"]}"#);
        assert_eq!(disabled_by(&files), None);
        file(
            "l.json",
            r#"{"disabledPlugins":["goose-triggers"],"enabledPlugins":[]}"#,
        );
        assert_eq!(disabled_by(&files), Some(local.clone()));
        // goose passes over a file it cannot read as settings (a map where it wants a list).
        file(
            "l.json",
            r#"{"enabledPlugins":{"goose-triggers":false},"disabledPlugins":["goose-triggers"]}"#,
        );
        assert_eq!(disabled_by(&files), None);
        file("p.json", r#"{"disabledPlugins":["other"]}"#);
        assert_eq!(disabled_by(&files), Some(user));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plugins_setting_is_read_as_goose_reads_it() {
        let root = Path::new("/w/.agents/plugins/goose-triggers");
        let off_of = |layers: &[String]| disabled_in_config(plugins_setting(None, layers), root);
        let off =
            "plugins:\n  /w/.agents/plugins/goose-triggers:\n    enabled: false\n".to_string();
        let on = "plugins:\n  /w/.agents/plugins/goose-triggers:\n    enabled: true\n".to_string();
        let other =
            "plugins:\n  /x/.agents/plugins/goose-triggers:\n    enabled: false\n".to_string();
        // The last file with `plugins:` gives the whole map.
        assert!(off_of(std::slice::from_ref(&off)));
        assert!(!off_of(&[off.clone(), on.clone()]));
        assert!(!off_of(&[off.clone(), other]));
        assert!(off_of(&[off.clone(), "GOOSE_PROVIDER: openai\n".into()]));
        assert!(!off_of(&[]));
        // A text value is read as JSON.
        let quoted = r#"plugins: '{"/w/.agents/plugins/goose-triggers": {"enabled": false}}'"#;
        assert!(off_of(&[quoted.to_string()]));
        // A map goose cannot read whole is an empty one for goose.
        let odd = "plugins:\n  /w/.agents/plugins/goose-triggers:\n    enabled: false\n  /y:\n    on: 1\n";
        assert!(!off_of(&[odd.to_string()]));
        // The PLUGINS variable, when set, is the setting, whatever the files say.
        let env = r#"{"/w/.agents/plugins/goose-triggers": {"enabled": false}}"#;
        assert!(disabled_in_config(
            plugins_setting(Some(env), std::slice::from_ref(&on)),
            root
        ));
        assert!(!disabled_in_config(
            plugins_setting(Some("{}"), std::slice::from_ref(&off)),
            root
        ));
        assert!(!disabled_in_config(
            plugins_setting(Some("x"), &[off]),
            root
        ));
    }

    #[test]
    fn a_working_folder_reached_through_a_link_is_keyed_by_its_real_path() {
        let base = std::env::temp_dir().join(format!(
            "gt-plugin-link-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();
        let key = plugin_root(&real);
        let off = format!("plugins:\n  {}:\n    enabled: false\n", key.display());
        let user = base.join("home");
        let err = preflight_with(&link, None, std::slice::from_ref(&off), &user).unwrap_err();
        assert!(err.to_string().contains("turns off the plugin"), "{err}");
        // The same through the PLUGINS variable, and a settings file in the folder.
        let env = format!(r#"{{"{}": {{"enabled": false}}}}"#, key.display());
        assert!(preflight_with(&link, Some(&env), &[], &user).is_err());
        assert!(preflight_with(&link, None, &[], &user).is_ok());
        std::fs::create_dir_all(real.join(".config/goose")).unwrap();
        std::fs::write(
            real.join(".config/goose/settings.json"),
            r#"{"disabledPlugins":["goose-triggers"]}"#,
        )
        .unwrap();
        assert!(preflight_with(&link, None, &[], &user).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
