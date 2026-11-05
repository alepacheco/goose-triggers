//! What goose-triggers reads of a goose recipe: the parameters it declares (an event's
//! fields are passed as those parameters) and the provider it asks for.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawRecipe {
    #[serde(default)]
    parameters: Vec<RawParam>,
    settings: Option<RawSettings>,
}

#[derive(Debug, Deserialize)]
struct RawParam {
    key: String,
    requirement: Option<String>,
    default: Option<serde_yaml_ng::Value>,
}

#[derive(Debug, Deserialize)]
struct RawSettings {
    goose_provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub key: String,
    /// The event must have this field: the recipe requires it and gives no default.
    pub needed: bool,
}

#[derive(Debug, Clone)]
pub struct RecipeInfo {
    pub params: Vec<Param>,
    pub provider: Option<String>,
}

pub fn read(path: &Path) -> Result<RecipeInfo> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading recipe {}", path.display()))?;
    parse(&text).with_context(|| format!("reading recipe {}", path.display()))
}

pub fn parse(text: &str) -> Result<RecipeInfo> {
    let raw: RawRecipe = if path_is_json(text) {
        serde_json::from_str(text)?
    } else {
        serde_yaml_ng::from_str(text)?
    };
    let params = raw
        .parameters
        .into_iter()
        .map(|p| Param {
            needed: p.default.is_none() && p.requirement.as_deref() != Some("optional"),
            key: p.key,
        })
        .collect();
    Ok(RecipeInfo {
        params,
        provider: raw.settings.and_then(|s| s.goose_provider),
    })
}

fn path_is_json(text: &str) -> bool {
    text.trim_start().starts_with('{')
}

/// The `--params` for a run: each parameter the recipe declares, from the event field of the
/// same name. Fields the recipe does not declare are not passed. Fails when the event lacks a
/// field the recipe needs.
pub fn params_for(
    info: &RecipeInfo,
    fields: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for p in &info.params {
        match fields.get(&p.key) {
            Some(v) => out.push((p.key.clone(), v.clone())),
            None if p.needed => missing.push(p.key.clone()),
            None => {}
        }
    }
    if !missing.is_empty() {
        let have: Vec<&str> = fields.keys().map(String::as_str).collect();
        bail!(
            "the recipe needs parameter(s) {} that the event does not have (its fields: {})",
            missing.join(", "),
            if have.is_empty() {
                "none".to_string()
            } else {
                have.join(", ")
            }
        );
    }
    Ok(out)
}

/// A value that cannot change a YAML or JSON document's structure wherever a template puts
/// it: letters, digits, spaces and `._-/@+=`.
pub fn inert(v: &str) -> bool {
    v.chars()
        .all(|c| c.is_alphanumeric() || " ._-/@+=".contains(c))
}

/// goose fills `{{ param }}` into the recipe file's text before it parses it, so a value
/// with YAML or JSON syntax in it could add keys to the recipe (extensions, retry commands)
/// that no hold would see. This renders the recipe through goose twice, with the real values
/// and with placeholders, and refuses the run unless both have the same structure and the
/// values only fill in text. Values that are inert need no check.
pub fn check_values_stay_text(
    goose: &str,
    recipe: &Path,
    params: &[(String, String)],
    wd: &Path,
) -> Result<()> {
    if params.iter().all(|(_, v)| inert(v)) {
        return Ok(());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tokens = Vec::new();
    let mut placeholders = Vec::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if inert(v) {
            placeholders.push((k.clone(), v.clone()));
        } else {
            let t = format!("goosetriggersvalue{i}x{nonce}");
            tokens.push((t.clone(), v.clone()));
            placeholders.push((k.clone(), t));
        }
    }
    let template = render(goose, recipe, &placeholders, wd)
        .context("goose could not render the recipe with placeholder values")?;
    let real = render(goose, recipe, params, wd).map_err(|e| {
        anyhow::anyhow!(
            "with the event's values, goose cannot read the recipe ({e:#}); the run was not started"
        )
    })?;
    if !same_shape(&real, &template, &tokens) {
        bail!("the event's values would change the recipe itself, not only fill in its text (a value carries YAML or JSON syntax); the run was not started");
    }
    Ok(())
}

fn render(
    goose: &str,
    recipe: &Path,
    params: &[(String, String)],
    wd: &Path,
) -> Result<serde_yaml_ng::Value> {
    let mut cmd = std::process::Command::new(goose);
    cmd.arg("run").arg("--recipe").arg(recipe);
    for (k, v) in params {
        cmd.arg("--params").arg(format!("{k}={v}"));
    }
    let out = cmd
        .arg("--render-recipe")
        .current_dir(wd)
        .env_remove(crate::hook::ENV_RUN)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {goose}"))?;
    if !out.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        bail!("{}", crate::util::short(text.trim(), 300));
    }
    Ok(serde_yaml_ng::from_slice(&out.stdout)?)
}

fn fill(text: &str, tokens: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (t, v) in tokens {
        out = out.replace(t.as_str(), v);
    }
    out
}

fn scalar_text(v: &serde_yaml_ng::Value) -> Option<String> {
    use serde_yaml_ng::Value as Y;
    match v {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        Y::Null => Some(String::new()),
        _ => None,
    }
}

/// Whether `real` is `template` with the placeholder tokens replaced by their values inside
/// text, and no other difference.
pub fn same_shape(
    real: &serde_yaml_ng::Value,
    template: &serde_yaml_ng::Value,
    tokens: &[(String, String)],
) -> bool {
    use serde_yaml_ng::Value as Y;
    match (real, template) {
        (Y::Mapping(a), Y::Mapping(b)) => {
            a.len() == b.len()
                && b.iter().all(|(k, vb)| {
                    let k = match k {
                        Y::String(s) => Y::String(fill(s, tokens)),
                        other => other.clone(),
                    };
                    a.get(&k).is_some_and(|va| same_shape(va, vb, tokens))
                })
        }
        (Y::Sequence(a), Y::Sequence(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_shape(x, y, tokens))
        }
        (Y::Tagged(a), Y::Tagged(b)) => a.tag == b.tag && same_shape(&a.value, &b.value, tokens),
        (Y::Mapping(_) | Y::Sequence(_) | Y::Tagged(_), _)
        | (_, Y::Mapping(_) | Y::Sequence(_) | Y::Tagged(_)) => false,
        (a, Y::String(b)) => scalar_text(a).is_some_and(|t| t == fill(b, tokens)),
        (a, b) => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECIPE: &str = r#"
version: "1.0.0"
title: t
description: d
settings:
  goose_provider: ollama
parameters:
  - key: file_path
    input_type: string
    requirement: required
    description: x
  - key: note
    input_type: string
    requirement: optional
    default: ""
    description: y
  - key: who
    input_type: string
    requirement: user_prompt
    description: z
prompt: "{{ file_path }} {{ note }} {{ who }}"
"#;

    fn y(text: &str) -> serde_yaml_ng::Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn values_that_only_fill_text_keep_the_shape() {
        let t = vec![("TOKEN0".to_string(), "a \"quoted\" value: {x}".to_string())];
        let template = y(
            "title: t\nprompt: 'see TOKEN0 now'\nextensions: [{type: builtin, name: developer}]\n",
        );
        let real = y("title: t\nprompt: 'see a \"quoted\" value: {x} now'\nextensions: [{type: builtin, name: developer}]\n");
        assert!(same_shape(&real, &template, &t));
    }

    #[test]
    fn values_that_add_keys_or_items_change_the_shape() {
        let t = vec![("TOKEN0".to_string(), "x".to_string())];
        let template = y("prompt: 'TOKEN0'\nextensions: [{type: builtin, name: developer}]\n");
        let added_key = y("prompt: 'x'\nretry: {checks: [{type: shell, command: touch p}]}\nextensions: [{type: builtin, name: developer}]\n");
        let added_item = y("prompt: 'x'\nextensions: [{type: builtin, name: developer}, {type: stdio, name: e, cmd: sh}]\n");
        let changed_text = y("prompt: 'y'\nextensions: [{type: builtin, name: developer}]\n");
        assert!(!same_shape(&added_key, &template, &t));
        assert!(!same_shape(&added_item, &template, &t));
        assert!(!same_shape(&changed_text, &template, &t));
        // A number filled in from a value is fine.
        let template = y("settings: {max_turns: 'TOKEN0'}\n");
        let t = vec![("TOKEN0".to_string(), "5".to_string())];
        assert!(same_shape(&y("settings: {max_turns: 5}\n"), &template, &t));
    }

    #[test]
    fn inert_values() {
        assert!(inert("invoice-0001.txt"));
        assert!(inert("/home/me/In box/a_b+c@d=e.pdf"));
        assert!(inert("Ünïcödé 2026"));
        for v in [
            "a\nb", "a\"b", "a'b", "a: b", "a #b", "{x}", "[x]", "a,b", "a\\b", "a|b", "a>b",
            "a*b", "a&b", "a!b", "a%b", "a`b",
        ] {
            assert!(!inert(v), "{v:?}");
        }
    }

    #[test]
    fn reads_parameters_and_provider() {
        let info = parse(RECIPE).unwrap();
        assert_eq!(info.provider.as_deref(), Some("ollama"));
        let keys: Vec<(&str, bool)> = info
            .params
            .iter()
            .map(|p| (p.key.as_str(), p.needed))
            .collect();
        assert_eq!(keys, [("file_path", true), ("note", false), ("who", true)]);
    }

    #[test]
    fn passes_declared_fields_only_and_names_missing_ones() {
        let info = parse(RECIPE).unwrap();
        let mut fields = BTreeMap::new();
        fields.insert("file_path".to_string(), "/in/a.pdf".to_string());
        fields.insert("who".to_string(), "me".to_string());
        fields.insert("extra".to_string(), "dropped".to_string());
        let p = params_for(&info, &fields).unwrap();
        assert_eq!(
            p,
            vec![
                ("file_path".to_string(), "/in/a.pdf".to_string()),
                ("who".to_string(), "me".to_string())
            ]
        );
        fields.remove("who");
        let err = params_for(&info, &fields).unwrap_err().to_string();
        assert!(err.contains("who") && err.contains("file_path"), "{err}");
    }
}
