//! The config file: where state lives, the webhook listener, and the rules.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::util::resolve_path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    state_dir: Option<String>,
    goose: Option<String>,
    max_concurrent_runs: Option<usize>,
    webhook: Option<RawWebhook>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWebhook {
    listen: Option<String>,
    secret: Option<String>,
    secret_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    name: String,
    folder: Option<String>,
    pattern: Option<String>,
    webhook: Option<bool>,
    recipe: String,
    #[serde(default)]
    hold: Vec<String>,
    working_dir: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    max_turns: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub state_dir: PathBuf,
    /// The goose binary: a path, or a name looked up on PATH.
    pub goose: String,
    pub max_concurrent_runs: usize,
    pub webhook: Option<Webhook>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
pub struct Webhook {
    pub listen: String,
    pub secret: String,
    /// The environment variable the secret came from: kept out of goose's environment.
    pub secret_env: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Source {
    /// A new file in `path` whose name matches `pattern`.
    Folder { path: PathBuf, pattern: String },
    /// A POST to `/hooks/<rule name>` on the webhook listener.
    Webhook,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub source: Source,
    pub recipe: PathBuf,
    /// Tool-name patterns (`*` and `?` wildcards) whose calls wait for the user's approval.
    pub hold: Vec<String>,
    pub working_dir: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
}

impl Config {
    /// Loads and checks the config file (`path`, else the default one): `run` needs it.
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let named = std::env::var_os(crate::CONFIG_ENV);
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => named_config(named)?.unwrap_or_else(crate::default_config_path),
        };
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "reading the config file {} (see the README for an example)",
                path.display()
            )
        })?;
        Self::parse_with(&text, &path, true)
    }

    /// The config as far as `status`, `log`, `approve` and `reject` need it (where state
    /// lives). A file named by `-c` or `GOOSE_TRIGGERS_CONFIG` must exist, as for `run`, so a
    /// mistyped name never makes these commands act on another state folder; with neither, the
    /// defaults serve when the default file does not exist.
    pub fn load_for_state(path: Option<&Path>) -> Result<Config> {
        let named = std::env::var_os(crate::CONFIG_ENV);
        match state_config_path(path, named, crate::default_config_path())? {
            Some(p) => {
                let text = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading the config file {}", p.display()))?;
                Self::parse_with(&text, &p, false)
            }
            None => Ok(Config {
                state_dir: default_state_dir(),
                goose: "goose".into(),
                max_concurrent_runs: 2,
                webhook: None,
                rules: Vec::new(),
            }),
        }
    }

    #[cfg(test)]
    pub fn parse(text: &str, path: &Path) -> Result<Config> {
        Self::parse_with(text, path, true)
    }

    /// `strict` checks what running the rules needs: their folders and recipes exist and the
    /// webhook secret is set. The other commands only need to know where state lives.
    pub fn parse_with(text: &str, path: &Path, strict: bool) -> Result<Config> {
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
            .with_context(|| format!("reading the config file {}", path.display()))?;
        if doc.is_null() {
            bail!(
                "the config file {} is empty (see the README for an example)",
                path.display()
            );
        }
        let raw: RawConfig = serde_yaml_ng::from_value(doc)
            .with_context(|| format!("reading the config file {}", path.display()))?;
        let base = path
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."));
        let base = std::fs::canonicalize(&base).unwrap_or(base);
        let state_dir = raw
            .state_dir
            .as_deref()
            .map(|s| resolve_path(s, &base))
            .unwrap_or_else(default_state_dir);
        let goose = match raw.goose {
            Some(g) if g.contains('/') => resolve_path(&g, &base).to_string_lossy().into_owned(),
            Some(g) => g,
            None => "goose".into(),
        };
        let max_concurrent_runs = raw.max_concurrent_runs.unwrap_or(2);
        if max_concurrent_runs == 0 {
            bail!("max_concurrent_runs must be at least 1");
        }

        let mut rules = Vec::new();
        let mut names = HashSet::new();
        for r in raw.rules {
            let rule = check_rule(r, &base, &state_dir, strict)?;
            if !names.insert(rule.name.clone()) {
                bail!("two rules are named {:?}", rule.name);
            }
            rules.push(rule);
        }

        let needs_webhook = strict && rules.iter().any(|r| matches!(r.source, Source::Webhook));
        let webhook = match raw.webhook {
            None if needs_webhook => {
                bail!("a rule has `webhook: true`, so the config needs a `webhook:` section with a secret")
            }
            None => None,
            Some(w) if !strict => Some(Webhook {
                listen: w.listen.unwrap_or_else(|| "127.0.0.1:8787".into()),
                secret: String::new(),
                secret_env: w.secret_env,
            }),
            Some(w) => {
                let secret_env = w.secret_env.clone();
                let secret = match (w.secret, w.secret_env) {
                    (Some(_), Some(_)) => bail!("webhook: give `secret` or `secret_env`, not both"),
                    (Some(s), None) => s,
                    (None, Some(var)) => std::env::var(&var).with_context(|| {
                        format!("webhook.secret_env names {var}, which is not set")
                    })?,
                    (None, None) => {
                        bail!("webhook: a shared secret is required (`secret_env` or `secret`)")
                    }
                };
                if secret.chars().count() < 16 {
                    bail!("webhook: the shared secret must be at least 16 characters long");
                }
                Some(Webhook {
                    listen: w.listen.unwrap_or_else(|| "127.0.0.1:8787".into()),
                    secret,
                    secret_env,
                })
            }
        };

        Ok(Config {
            state_dir,
            goose,
            max_concurrent_runs,
            webhook,
            rules,
        })
    }

    pub fn rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("state.db")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.state_dir.join("runs")
    }
}

/// The config file `GOOSE_TRIGGERS_CONFIG` names (its value, `named`): set but empty is an
/// error, as a name that is not a file is wherever it is read.
fn named_config(named: Option<std::ffi::OsString>) -> Result<Option<PathBuf>> {
    match named {
        Some(v) if v.is_empty() => bail!(
            "{} is set but empty: unset it, or set it to the config file",
            crate::CONFIG_ENV
        ),
        Some(v) => Ok(Some(PathBuf::from(v))),
        None => Ok(None),
    }
}

/// The config file the state commands read: the one `-c` gives, else the one
/// `GOOSE_TRIGGERS_CONFIG` names (`named`), which must exist; else the default file
/// (`default`) if it exists; else none, and the defaults serve.
fn state_config_path(
    given: Option<&Path>,
    named: Option<std::ffi::OsString>,
    default: PathBuf,
) -> Result<Option<PathBuf>> {
    if let Some(p) = given {
        return Ok(Some(p.to_path_buf()));
    }
    match named_config(named)? {
        Some(p) if !p.is_file() => bail!(
            "{} names {}, which is not a file",
            crate::CONFIG_ENV,
            p.display()
        ),
        Some(p) => Ok(Some(p)),
        None if default.exists() => Ok(Some(default)),
        None => Ok(None),
    }
}

pub fn default_state_dir() -> PathBuf {
    crate::util::data_home().join("goose-triggers")
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_rule(r: RawRule, base: &Path, state_dir: &Path, strict: bool) -> Result<Rule> {
    if !valid_name(&r.name) {
        bail!(
            "rule name {:?}: use letters, digits, '-' and '_' (at most 64, starting with a letter or digit)",
            r.name
        );
    }
    let name = r.name;
    let source = match (r.folder, r.webhook.unwrap_or(false)) {
        (Some(_), true) => bail!("rule {name}: give either `folder` or `webhook: true`, not both"),
        (None, false) => {
            bail!("rule {name}: give an event source, `folder: <path>` or `webhook: true`")
        }
        (Some(folder), false) => {
            let path = resolve_path(&folder, base);
            if strict && !path.is_dir() {
                bail!("rule {name}: folder {} does not exist", path.display());
            }
            let pattern = r.pattern.unwrap_or_else(|| "*".into());
            glob::Pattern::new(&pattern)
                .with_context(|| format!("rule {name}: pattern {pattern:?}"))?;
            Source::Folder { path, pattern }
        }
        (None, true) => {
            if r.pattern.is_some() {
                bail!("rule {name}: `pattern` applies to folder rules only");
            }
            Source::Webhook
        }
    };
    let recipe = resolve_path(&r.recipe, base);
    if strict && !recipe.is_file() {
        bail!("rule {name}: recipe {} does not exist", recipe.display());
    }
    for h in &r.hold {
        if h.trim().is_empty() {
            bail!("rule {name}: an empty `hold` entry");
        }
        glob::Pattern::new(h).with_context(|| format!("rule {name}: hold pattern {h:?}"))?;
    }
    let working_dir = r
        .working_dir
        .as_deref()
        .map(|w| resolve_path(w, base))
        .unwrap_or_else(|| state_dir.join("work").join(&name));
    Ok(Rule {
        name,
        source,
        recipe,
        hold: r.hold,
        working_dir,
        provider: r.provider,
        model: r.model,
        max_turns: r.max_turns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gt-config-test-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        std::fs::create_dir_all(dir.join("inbox")).unwrap();
        std::fs::write(dir.join("r.yaml"), "title: t\n").unwrap();
        dir
    }

    #[test]
    fn parses_a_folder_and_a_webhook_rule() {
        let dir = setup();
        let text = r#"
state_dir: state
webhook:
  secret: "0123456789abcdef0123"
rules:
  - name: invoices
    folder: inbox
    pattern: "*.pdf"
    recipe: r.yaml
    hold: [shell, "mail__*"]
  - name: form
    webhook: true
    recipe: r.yaml
"#;
        let c = Config::parse(text, &dir.join("config.yaml")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(c.state_dir, dir.join("state"));
        assert_eq!(c.rules.len(), 2);
        assert!(matches!(&c.rules[0].source, Source::Folder { pattern, .. } if pattern == "*.pdf"));
        assert_eq!(c.rules[0].hold, vec!["shell", "mail__*"]);
        assert_eq!(c.rules[0].working_dir, dir.join("state/work/invoices"));
        assert!(matches!(c.rules[1].source, Source::Webhook));
        assert_eq!(c.webhook.unwrap().listen, "127.0.0.1:8787");
    }

    #[test]
    fn state_commands_refuse_a_named_config_that_is_missing_or_empty() {
        let dir = setup();
        let missing = dir.join("config.yml");
        let default = dir.join("default.yaml");
        // GOOSE_TRIGGERS_CONFIG naming a missing file: an error, not the default state folder.
        let err = state_config_path(None, Some(missing.clone().into()), default.clone())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a file"), "{err}");
        let err = state_config_path(None, Some("".into()), default.clone())
            .unwrap_err()
            .to_string();
        assert!(err.contains("set but empty"), "{err}");
        // -c wins; with neither, the default file only when it exists.
        assert_eq!(
            state_config_path(Some(&missing), Some("x".into()), default.clone()).unwrap(),
            Some(missing.clone())
        );
        assert_eq!(
            state_config_path(None, None, default.clone()).unwrap(),
            None
        );
        std::fs::write(&default, "state_dir: s\n").unwrap();
        assert_eq!(
            state_config_path(None, None, default.clone()).unwrap(),
            Some(default.clone())
        );
        assert_eq!(
            state_config_path(None, Some(default.clone().into()), dir.join("other")).unwrap(),
            Some(default)
        );
        // An empty file, or one of comments only, is refused by every command.
        for text in ["", "\n", "# nothing yet\n"] {
            let err = format!(
                "{:#}",
                Config::parse_with(text, &missing, false).unwrap_err()
            );
            assert!(err.contains("is empty"), "{text:?}: {err}");
        }
    }

    #[test]
    fn refuses_bad_rules() {
        let dir = setup();
        let p = dir.join("config.yaml");
        let cases = [
            ("rules:\n  - {name: a, recipe: r.yaml}\n", "event source"),
            ("rules:\n  - {name: a, folder: inbox, webhook: true, recipe: r.yaml}\n", "not both"),
            ("rules:\n  - {name: 'a b', folder: inbox, recipe: r.yaml}\n", "rule name"),
            ("rules:\n  - {name: a, folder: nowhere, recipe: r.yaml}\n", "does not exist"),
            ("rules:\n  - {name: a, folder: inbox, recipe: none.yaml}\n", "does not exist"),
            ("rules:\n  - {name: a, webhook: true, recipe: r.yaml}\n", "webhook:"),
            (
                "webhook: {secret: short}\nrules:\n  - {name: a, webhook: true, recipe: r.yaml}\n",
                "16 characters",
            ),
            ("rules:\n  - {name: a, folder: inbox, recipe: r.yaml, colour: red}\n", "unknown field"),
            (
                "rules:\n  - {name: a, folder: inbox, recipe: r.yaml}\n  - {name: a, folder: inbox, recipe: r.yaml}\n",
                "two rules",
            ),
        ];
        for (text, want) in cases {
            let err = format!("{:#}", Config::parse(text, &p).unwrap_err());
            assert!(err.contains(want), "{text:?}: {err}");
        }
    }
}
