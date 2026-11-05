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
