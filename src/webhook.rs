//! Webhook sources: `POST /hooks/<rule name>` on a local HTTP listener, with the shared
//! secret as `Authorization: Bearer <secret>`.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::config::{Config, Source};
use crate::store::Store;
use crate::util::same_secret;

/// Bodies are capped at 64 KiB, so that each field passed to goose fits in one argument.
pub const MAX_BODY: usize = 64 << 10;

/// Stores an event for a rule and queues its run: (event id, run id).
pub type Queue<'a> =
    dyn FnMut(&crate::config::Rule, BTreeMap<String, String>) -> Result<(i64, i64)> + 'a;

pub struct Reply {
    pub status: u16,
    pub body: Value,
}

fn reply(status: u16, body: Value) -> Reply {
    Reply { status, body }
}

/// The event's fields from a request body: a JSON object's top-level fields (text as is,
/// anything else as JSON text), and `body`, the whole body as text, unless the JSON has a
/// `body` field of its own.
pub fn fields_from_body(body: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) {
        for (k, v) in map {
            let text = match v {
                Value::String(s) => s,
                Value::Null => String::new(),
                other => other.to_string(),
            };
            fields.insert(k, text);
        }
    }
    fields
        .entry("body".into())
        .or_insert_with(|| body.to_string());
    fields
}

/// Handles one request; `queue` stores the event and its run, returning their ids.
pub fn handle(
    cfg: &Config,
    secret: &str,
    method: &str,
    url: &str,
    authorization: Option<&str>,
    body: &mut dyn Read,
    queue: &mut Queue,
) -> Reply {
    let path = url.split('?').next().unwrap_or("");
    if method == "GET" && path == "/health" {
        return reply(200, json!({"ok": true}));
    }
    let Some(name) = path.strip_prefix("/hooks/") else {
        return reply(404, json!({"error": "not found"}));
    };
    let Some(rule) = cfg
        .rule(name)
        .filter(|r| matches!(r.source, Source::Webhook))
    else {
        return reply(404, json!({"error": "no webhook rule by that name"}));
    };
    if method != "POST" {
        return reply(405, json!({"error": "use POST"}));
    }
    let given = authorization
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or("");
    if !same_secret(given.as_bytes(), secret.as_bytes()) {
        return reply(401, json!({"error": "missing or wrong shared secret"}));
    }
    let mut buf = Vec::new();
    if body
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return reply(400, json!({"error": "could not read the body"}));
    }
    if buf.len() > MAX_BODY {
        return reply(413, json!({"error": "body over 64 KiB"}));
    }
    let Ok(text) = String::from_utf8(buf) else {
        return reply(400, json!({"error": "the body is not UTF-8 text"}));
    };
    match queue(rule, fields_from_body(&text)) {
        Ok((event, run)) => reply(202, json!({"event": event, "run": run})),
        Err(e) => reply(500, json!({"error": format!("{e:#}")})),
    }
}

/// Starts the listener; it serves until `stop` is set.
pub fn serve(cfg: Arc<Config>, stop: &'static AtomicBool) -> Result<Option<JoinHandle<()>>> {
    let Some(wh) = cfg.webhook.clone() else {
        return Ok(None);
    };
    if !cfg
        .rules
        .iter()
        .any(|r| matches!(r.source, Source::Webhook))
    {
        return Ok(None);
    }
    let server = tiny_http::Server::http(&wh.listen)
        .map_err(|e| anyhow!("webhook listener on {}: {e}", wh.listen))?;
    let handle = std::thread::spawn(move || {
        let store = match Store::open(&cfg.db_path()) {
            Ok(s) => s,
            Err(e) => {
                crate::daemon::say(&format!("webhook listener stopped: {e:#}"));
                return;
            }
        };
        while !stop.load(Ordering::SeqCst) {
            let mut req = match server.recv_timeout(Duration::from_millis(300)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(e) => {
                    crate::daemon::say(&format!("webhook listener: {e}"));
                    continue;
                }
            };
            let method = req.method().as_str().to_string();
            let url = req.url().to_string();
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string());
            let mut queue = |rule: &crate::config::Rule, fields: BTreeMap<String, String>| {
                crate::daemon::queue_event(&store, rule, "webhook", &fields, None)?
                    .ok_or_else(|| anyhow!("the event was not stored"))
            };
            let r = handle(
                &cfg,
                &wh.secret,
                &method,
                &url,
                auth.as_deref(),
                req.as_reader(),
                &mut queue,
            );
            if r.status == 401 {
                crate::daemon::say(&format!("webhook {url}: refused, wrong or missing secret"));
            }
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap();
            let response = tiny_http::Response::from_string(r.body.to_string())
                .with_status_code(r.status)
                .with_header(header);
            let _ = req.respond(response);
        }
    });
    Ok(Some(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let dir = std::env::temp_dir().join(format!(
            "gt-webhook-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        std::fs::create_dir_all(dir.join("in")).unwrap();
        std::fs::write(dir.join("r.yaml"), "title: t\n").unwrap();
        let text = "webhook: {secret: 0123456789abcdef}\nrules:\n  - {name: form, webhook: true, recipe: r.yaml}\n  - {name: files, folder: in, recipe: r.yaml}\n";
        Config::parse(text, &dir.join("c.yaml")).unwrap()
    }

    fn call(c: &Config, method: &str, url: &str, auth: Option<&str>, body: &str) -> (u16, usize) {
        let mut n = 0;
        let mut queue = |_: &crate::config::Rule, _: BTreeMap<String, String>| {
            n += 1;
            Ok((1, 2))
        };
        let r = handle(
            c,
            "0123456789abcdef",
            method,
            url,
            auth,
            &mut body.as_bytes(),
            &mut queue,
        );
        (r.status, n)
    }

    #[test]
    fn checks_rule_method_and_secret() {
        let c = cfg();
        let ok = Some("Bearer 0123456789abcdef");
        assert_eq!(call(&c, "POST", "/hooks/form", ok, "{}"), (202, 1));
        assert_eq!(call(&c, "POST", "/hooks/form", None, "{}"), (401, 0));
        assert_eq!(
            call(
                &c,
                "POST",
                "/hooks/form",
                Some("Bearer 0123456789abcdeX"),
                "{}"
            ),
            (401, 0)
        );
        assert_eq!(call(&c, "POST", "/hooks/files", ok, "{}"), (404, 0));
        assert_eq!(call(&c, "POST", "/hooks/nope", ok, "{}"), (404, 0));
        assert_eq!(call(&c, "GET", "/hooks/form", ok, ""), (405, 0));
        assert_eq!(call(&c, "GET", "/health", None, ""), (200, 0));
        let big = "x".repeat(MAX_BODY + 1);
        assert_eq!(call(&c, "POST", "/hooks/form", ok, &big), (413, 0));
    }

    #[test]
    fn json_fields_become_event_fields() {
        let f = fields_from_body(
            r#"{"order": "A-1", "total": 12.5, "paid": true, "items": [1], "none": null}"#,
        );
        assert_eq!(f["order"], "A-1");
        assert_eq!(f["total"], "12.5");
        assert_eq!(f["paid"], "true");
        assert_eq!(f["items"], "[1]");
        assert_eq!(f["none"], "");
        assert!(f["body"].contains("A-1"));
        let f = fields_from_body("plain text");
        assert_eq!(f.len(), 1);
        assert_eq!(f["body"], "plain text");
        let f = fields_from_body(r#"{"body": "own"}"#);
        assert_eq!(f["body"], "own");
    }
}
