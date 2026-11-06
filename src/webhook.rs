//! Webhook sources: `POST /hooks/<rule name>` on a local HTTP listener, with the shared
//! secret as `Authorization: Bearer <secret>`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// At most this many requests are handled at once; more get 503.
const MAX_REQUESTS: usize = 16;
/// A client gets this long to send its whole request, and for each write of the reply.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The request line and headers may take at most this much.
const MAX_HEAD: usize = 16 << 10;

/// Writes a reply and closes the connection, without reading what the client still sends.
fn respond(mut stream: &TcpStream, r: Reply) {
    let reason = match r.status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let body = r.body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        r.status,
        body.len()
    );
    let _ = stream.flush();
    // Closing with unread request bytes would reset the connection before the client reads
    // the reply: stop writing, take briefly what the client still sends, then close.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let start = std::time::Instant::now();
    let mut sink = [0u8; 8192];
    let mut taken = 0;
    while taken < 4 * MAX_BODY && start.elapsed() < Duration::from_secs(1) {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => taken += n,
        }
    }
}

/// Reads a connection with one deadline for the whole request, however slowly the client
/// sends it.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: std::time::Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self
            .until
            .saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the request took too long",
            ));
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

/// The request line and headers: (method, target, headers with lower-case names).
type Head = (String, String, Vec<(String, String)>);

fn read_head(reader: &mut BufReader<Deadline>) -> std::result::Result<Head, u16> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        let n = reader
            .by_ref()
            .take((MAX_HEAD - total + 1) as u64)
            .read_line(&mut line)
            .map_err(|e| {
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) {
                    408u16
                } else {
                    400u16
                }
            })?;
        total += n;
        if n == 0 || total > MAX_HEAD {
            return Err(if total > MAX_HEAD { 431 } else { 400 });
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let mut parts = lines.first().ok_or(400u16)?.split_whitespace();
    let method = parts.next().ok_or(400u16)?.to_string();
    let target = parts.next().ok_or(400u16)?.to_string();
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok((method, target, headers))
}

/// One connection, one request, on a thread of its own: reads time out, and a body is read
/// only after the rule and the secret are checked, and only as far as its 64 KiB cap.
fn serve_one(cfg: &Config, secret: &str, stream: TcpStream) {
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut reader = BufReader::new(Deadline {
        stream: &stream,
        until: std::time::Instant::now() + IO_TIMEOUT,
    });
    let (method, target, headers) = match read_head(&mut reader) {
        Ok(h) => h,
        Err(status) => {
            respond(
                &stream,
                reply(status, json!({"error": "could not read the request"})),
            );
            return;
        }
    };
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let length = match (header("transfer-encoding"), header("content-length")) {
        (Some(_), _) => {
            respond(
                &stream,
                reply(
                    411,
                    json!({"error": "send a Content-Length, not a chunked body"}),
                ),
            );
            return;
        }
        (None, None) => 0,
        (None, Some(v)) => match v.parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                respond(&stream, reply(400, json!({"error": "bad Content-Length"})));
                return;
            }
        },
    };
    let authorized = header("authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .is_some_and(|given| same_secret(given.as_bytes(), secret.as_bytes()));
    if length > 0
        && length <= MAX_BODY as u64
        && authorized
        && header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue"))
    {
        let _ = (&stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = reader.take(length.min(MAX_BODY as u64 + 1));
    let store = match Store::open(&cfg.db_path()) {
        Ok(s) => s,
        Err(e) => {
            respond(&stream, reply(500, json!({"error": format!("{e:#}")})));
            return;
        }
    };
    let mut queue = |rule: &crate::config::Rule, fields: BTreeMap<String, String>| {
        crate::daemon::queue_event(&store, rule, "webhook", &fields, None)?
            .ok_or_else(|| anyhow!("the event was not stored"))
    };
    let r = if length > MAX_BODY as u64 {
        reply(413, json!({"error": "body over 64 KiB"}))
    } else {
        handle(
            cfg,
            secret,
            &method,
            &target,
            header("authorization"),
            &mut body,
            &mut queue,
        )
    };
    if r.status == 401 {
        crate::daemon::say(&format!(
            "webhook {target}: refused, wrong or missing secret"
        ));
    }
    respond(&stream, r);
}

/// Counts a request while it is served, however its thread ends.
struct Busy(Arc<AtomicUsize>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
    let listener = TcpListener::bind(&wh.listen)
        .map_err(|e| anyhow!("webhook listener on {}: {e}", wh.listen))?;
    listener.set_nonblocking(true)?;
    let busy = Arc::new(AtomicUsize::new(0));
    let handle = std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    crate::daemon::say(&format!("webhook listener: {e}"));
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            let _ = stream.set_nonblocking(false);
            let guard = Busy(busy.clone());
            let full = busy.fetch_add(1, Ordering::SeqCst) >= MAX_REQUESTS;
            let (cfg, secret) = (cfg.clone(), wh.secret.clone());
            std::thread::spawn(move || {
                let _guard = guard;
                if full {
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    respond(&stream, reply(503, json!({"error": "busy, try again"})));
                } else {
                    serve_one(&cfg, &secret, stream);
                }
            });
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

    /// Sends raw bytes to serve_one and returns the reply's status line, without the client
    /// ever sending the body it announced.
    fn exchange(c: &Config, request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        let c = c.clone();
        let t = std::thread::spawn(move || serve_one(&c, "0123456789abcdef", server));
        client.write_all(request.as_bytes()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut out = String::new();
        let _ = client.read_to_string(&mut out);
        t.join().unwrap();
        out.lines().next().unwrap_or("").to_string()
    }

    #[test]
    fn announced_bodies_are_not_waited_for_or_drained() {
        let c = cfg();
        let start = std::time::Instant::now();
        let huge = "POST /hooks/form HTTP/1.1\r\nContent-Length: 100000000000000000\r\n\r\n";
        assert_eq!(exchange(&c, huge), "HTTP/1.1 413 Payload Too Large");
        let stalled = "POST /hooks/form HTTP/1.1\r\nContent-Length: 5000\r\n\r\n";
        assert_eq!(exchange(&c, stalled), "HTTP/1.1 401 Unauthorized");
        let chunked = "POST /hooks/form HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(exchange(&c, chunked), "HTTP/1.1 411 Length Required");
        assert!(start.elapsed() < Duration::from_secs(3));
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
