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
    input_type: Option<String>,
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
    /// An `input_type: file` parameter: goose reads the file at the path given and pastes its
    /// contents into the recipe's text.
    pub file: bool,
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
            file: p.input_type.as_deref() == Some("file"),
            key: p.key,
        })
        .collect();
    Ok(RecipeInfo {
        params,
        provider: raw.settings.and_then(|s| s.goose_provider),
    })
}

fn path_is_json(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('{') && !t.starts_with("{%") && !t.starts_with("{{")
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
            Some(_) if p.file => bail!(
                "the recipe's parameter {} is a file parameter: goose would paste the contents of the file it names into the recipe itself, so goose-triggers does not fill file parameters from events. Declare it as a string and have the recipe read the file with its tools",
                p.key
            ),
            Some(v) => out.push((p.key.clone(), v.clone())),
            None if p.needed => missing.push(p.key.clone()),
            None => {}
        }
    }
    if !missing.is_empty() {
        // Only the recipe's own names: the event's field names are the sender's text
        // (`goose-triggers log <run>` shows the event).
        bail!(
            "the recipe needs parameter(s) {} that the event does not have",
            missing.join(", ")
        );
    }
    Ok(out)
}

/// goose fills `{{ param }}` into the recipe file's text before it parses it, so a value
/// with YAML or JSON syntax in it could add keys to the recipe (extensions, retry commands)
/// that no hold would see, and a value placed in an extension's command or the provider
/// setting would choose what goose runs. This renders the recipe through goose with the
/// event's values and, when there are any, with a placeholder for each; the run goes ahead
/// only if both have the same structure and the values fill in nothing but `TEXT_FIELDS`.
/// Returns the recipe as rendered with the event's values.
pub fn check_values_stay_text(
    goose: &str,
    recipe: &Path,
    params: &[(String, String)],
    wd: &Path,
    hide_env: Option<&str>,
) -> Result<serde_yaml_ng::Value> {
    let real = render(goose, recipe, params, wd, hide_env);
    if params.is_empty() {
        return real.map_err(|e| {
            anyhow::anyhow!("goose cannot read the recipe ({e:#}); the run was not started")
        });
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tokens = Vec::new();
    let mut placeholders = Vec::new();
    for (i, (k, v)) in params.iter().enumerate() {
        let t = format!("goosetriggersvalue{i}x{nonce}");
        tokens.push((t.clone(), v.clone()));
        placeholders.push((k.clone(), t));
    }
    // goose's errors may quote the values (a YAML type error quotes the string), so a
    // failure is told in goose's words only from the render with placeholders.
    let template = render(goose, recipe, &placeholders, wd, hide_env).map_err(|e| {
        anyhow::anyhow!(
            "goose cannot read the recipe, even with placeholder values ({e:#}); the run was not started"
        )
    })?;
    let real = match real {
        Ok(r) => r,
        Err(e) => {
            return Err(match e.downcast::<RenderFailed>() {
                Ok(RenderFailed(goose_said)) => ValuesBreakRecipe { goose_said }.into(),
                // goose-triggers' own error (goose did not start, or took too long): no value in it.
                Err(e) => e.context(
                    "goose could not render the recipe with the event's values; the run was not started",
                ),
            });
        }
    };
    if !same_recipe(&real, &template, &tokens) {
        bail!("the event's values would change the recipe itself, not only fill in its text (a value carries YAML or JSON syntax, or fills in a field other than {}); the run was not started", TEXT_FIELDS.join(", "));
    }
    Ok(real)
}

/// At `goose-triggers run`'s start: goose reads the recipe as a recipe (a title, a
/// description, a prompt or instructions, as goose itself requires), rendered with a
/// placeholder for each parameter an event can fill, so a file that is no recipe is refused at
/// start rather than at each event. A recipe with a required file parameter is left to each
/// event, which is refused for it.
pub fn check_readable(
    goose: &str,
    recipe: &Path,
    info: &RecipeInfo,
    wd: &Path,
    hide_env: Option<&str>,
) -> Result<()> {
    if info.params.iter().any(|p| p.file && p.needed) {
        return Ok(());
    }
    let placeholders: Vec<(String, String)> = info
        .params
        .iter()
        .filter(|p| !p.file)
        .map(|p| (p.key.clone(), "goosetriggersplaceholder".to_string()))
        .collect();
    render(goose, recipe, &placeholders, wd, hide_env)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("goose cannot read the recipe {} ({e:#})", recipe.display()))
}

/// goose's own output when it could not render a recipe, which may quote the values it was
/// given.
#[derive(Debug)]
struct RenderFailed(String);

impl std::fmt::Display for RenderFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenderFailed {}

/// goose cannot render the recipe with the event's values, though it can with placeholders.
/// Its text, the run's failure reason, quotes nothing of the event; goose's own error, which
/// may, is kept apart for the run's timeline.
#[derive(Debug)]
pub struct ValuesBreakRecipe {
    pub goose_said: String,
}

impl std::fmt::Display for ValuesBreakRecipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("with the event's values goose cannot read the recipe, which it reads with placeholder values, so a value breaks it (by its YAML or JSON syntax, say); goose's error is in the run's log; the run was not started")
    }
}

impl std::error::Error for ValuesBreakRecipe {}

/// The provider a rendered recipe asks for.
pub fn rendered_provider(recipe: &serde_yaml_ng::Value) -> Option<String> {
    recipe
        .get("settings")?
        .get("goose_provider")?
        .as_str()
        .map(str::to_string)
}

/// How long goose may take to render a recipe.
const RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn render(
    goose: &str,
    recipe: &Path,
    params: &[(String, String)],
    wd: &Path,
    hide_env: Option<&str>,
) -> Result<serde_yaml_ng::Value> {
    let mut cmd = std::process::Command::new(goose);
    cmd.arg("run").arg("--recipe").arg(recipe);
    for (k, v) in params {
        cmd.arg("--params").arg(format!("{k}={v}"));
    }
    cmd.arg("--render-recipe")
        .current_dir(wd)
        .env_remove(crate::hook::ENV_RUN);
    if let Some(var) = hide_env {
        cmd.env_remove(var);
    }
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a Ctrl-C meant for goose-triggers does not end it.
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    let Some(output) = crate::util::output_within(&mut cmd, RENDER_TIMEOUT)
        .with_context(|| format!("running {goose}"))?
    else {
        bail!(
            "goose took over {} s to render the recipe",
            RENDER_TIMEOUT.as_secs()
        );
    };
    let std::process::Output {
        status,
        stdout: out,
        stderr: err,
    } = output;
    if !status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        );
        return Err(RenderFailed(crate::util::short(text.trim(), 300)).into());
    }
    serde_yaml_ng::from_slice(&out)
        .map_err(|e| RenderFailed(format!("goose's rendered recipe is not YAML: {e}")).into())
}

/// The recipe fields an event's value may fill in: text the model reads, nothing goose runs
/// or configures from.
pub const TEXT_FIELDS: &[&str] = &[
    "prompt",
    "instructions",
    "title",
    "description",
    "activities",
    "author",
];

fn is_scalar(v: &serde_yaml_ng::Value) -> bool {
    use serde_yaml_ng::Value as Y;
    matches!(v, Y::String(_) | Y::Number(_) | Y::Bool(_) | Y::Null)
}

/// Whether the recipe rendered with the event's values (`real`) is the one rendered with
/// placeholder tokens (`template`), except for text the tokens filled in, and only in
/// `TEXT_FIELDS`.
pub fn same_recipe(
    real: &serde_yaml_ng::Value,
    template: &serde_yaml_ng::Value,
    tokens: &[(String, String)],
) -> bool {
    use serde_yaml_ng::Value as Y;
    let (Y::Mapping(a), Y::Mapping(b)) = (real, template) else {
        return same_shape(real, template, tokens, false);
    };
    a.len() == b.len()
        && b.iter().all(|(k, vb)| {
            let text = k.as_str().is_some_and(|k| TEXT_FIELDS.contains(&k));
            a.get(k).is_some_and(|va| same_shape(va, vb, tokens, text))
        })
}

/// Whether `real` is `template` with no difference but text filled in from the tokens, where
/// `text` allows that. Keys must match exactly; a string holding a token may come out as any
/// text (the structure around it is already the same).
pub fn same_shape(
    real: &serde_yaml_ng::Value,
    template: &serde_yaml_ng::Value,
    tokens: &[(String, String)],
    text: bool,
) -> bool {
    use serde_yaml_ng::Value as Y;
    match (real, template) {
        (Y::Mapping(a), Y::Mapping(b)) => {
            a.len() == b.len()
                && b.iter()
                    .all(|(k, vb)| a.get(k).is_some_and(|va| same_shape(va, vb, tokens, text)))
        }
        (Y::Sequence(a), Y::Sequence(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_shape(x, y, tokens, text))
        }
        (Y::Tagged(a), Y::Tagged(b)) => {
            a.tag == b.tag && same_shape(&a.value, &b.value, tokens, text)
        }
        (a, Y::String(b)) if tokens.iter().any(|(t, _)| b.contains(t.as_str())) => {
            text && is_scalar(a)
        }
        // In a text field, a filter or a condition may reshape the text a value filled in.
        (a, b) if text && is_scalar(a) && is_scalar(b) => true,
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
        assert!(same_recipe(&real, &template, &t));
        // Text the YAML parser reshaped (line breaks, folding) is still text.
        let real = y("title: t\nprompt: \"see a\\n b now\"\nextensions: [{type: builtin, name: developer}]\n");
        assert!(same_recipe(&real, &template, &t));
        // A number that came from a value, in a text field.
        assert!(same_recipe(&y("prompt: 5\n"), &y("prompt: 'TOKEN0'\n"), &t));
        // Text a filter or a condition reshaped (`{{ x | upper }}`, `{% if %}`).
        assert!(same_recipe(
            &y("prompt: 'For SALES'\n"),
            &y("prompt: 'For GOOSETRIGGERS'\n"),
            &t
        ));
    }

    #[test]
    fn values_that_add_keys_or_items_or_fill_other_fields_are_refused() {
        let t = vec![("TOKEN0".to_string(), "x".to_string())];
        let template = y("prompt: 'TOKEN0'\nextensions: [{type: builtin, name: developer}]\n");
        let added_key = y("prompt: 'x'\nretry: {checks: [{type: shell, command: touch p}]}\nextensions: [{type: builtin, name: developer}]\n");
        let added_item = y("prompt: 'x'\nextensions: [{type: builtin, name: developer}, {type: stdio, name: e, cmd: sh}]\n");
        assert!(!same_recipe(&added_key, &template, &t));
        assert!(!same_recipe(&added_item, &template, &t));
        // Outside the text fields nothing may change, token or not.
        let template = y("prompt: 'TOKEN0'\nsettings: {max_turns: 3}\n");
        assert!(!same_recipe(
            &y("prompt: 'x'\nsettings: {max_turns: 4}\n"),
            &template,
            &t
        ));
        // A value may not fill in what goose runs or configures from.
        let template = y("prompt: p\nextensions: [{type: stdio, name: e, cmd: 'TOKEN0'}]\n");
        let real = y("prompt: p\nextensions: [{type: stdio, name: e, cmd: x}]\n");
        assert!(!same_recipe(&real, &template, &t));
        let template = y("prompt: p\nsettings: {goose_provider: 'TOKEN0'}\n");
        assert!(!same_recipe(
            &y("prompt: p\nsettings: {goose_provider: x}\n"),
            &template,
            &t
        ));
    }

    #[test]
    fn file_parameters_are_not_filled_from_events() {
        let info = parse("parameters:\n  - {key: doc, input_type: file, requirement: required, description: d}\nprompt: '{{ doc }}'\n").unwrap();
        assert!(info.params[0].file);
        let mut fields = BTreeMap::new();
        fields.insert("doc".to_string(), "/home/me/.ssh/id_rsa".to_string());
        let err = params_for(&info, &fields).unwrap_err().to_string();
        assert!(err.contains("file parameter"), "{err}");
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
        fields.insert("Your key was revoked, see evil.example".into(), "1".into());
        let err = params_for(&info, &fields).unwrap_err().to_string();
        // It names the recipe's missing parameter, never the event's own field names.
        assert!(
            err.contains("who") && !err.contains("extra") && !err.contains("evil.example"),
            "{err}"
        );
    }
}
