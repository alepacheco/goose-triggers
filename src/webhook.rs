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

/// Where a request goes, before its body is read: `/health`'s answer, or the webhook rule it
/// posts to, or why not. The secret is checked first for every path under `/hooks/`, so a
/// caller without it learns nothing of the rules (which names exist, which methods they take).
fn route<'a>(
    cfg: &'a Config,
    secret: &str,
    method: &str,
    url: &str,
    authorization: Option<&str>,
) -> std::result::Result<&'a crate::config::Rule, Reply> {
    let path = url.split('?').next().unwrap_or("");
    if (method == "GET" || method == "HEAD") && path == "/health" {
        return Err(reply(200, json!({"ok": true})));
    }
    let Some(name) = path.strip_prefix("/hooks/") else {
        return Err(reply(404, json!({"error": "not found"})));
    };
    let given = authorization
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or("");
    if !same_secret(given.as_bytes(), secret.as_bytes()) {
        return Err(reply(
            401,
            json!({"error": "missing or wrong shared secret"}),
        ));
    }
    let Some(rule) = cfg
        .rule(name)
        .filter(|r| matches!(r.source, Source::Webhook))
    else {
        return Err(reply(404, json!({"error": "no webhook rule by that name"})));
    };
    if method != "POST" {
        return Err(reply(405, json!({"error": "use POST"})));
    }
    Ok(rule)
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
    let rule = match route(cfg, secret, method, url, authorization) {
        Ok(rule) => rule,
        Err(r) => return r,
    };
    let mut buf = Vec::new();
    if let Err(e) = body.take(MAX_BODY as u64 + 1).read_to_end(&mut buf) {
        return if matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ) {
            reply(
                408,
                json!({"error": "the request did not arrive whole in time"}),
            )
        } else {
            reply(400, json!({"error": "could not read the body"}))
        };
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

/// Writes a reply (only its head when `head_only`, the answer to a HEAD request) and closes
/// the connection, without reading what the client still sends.
fn respond(mut stream: &TcpStream, r: Reply, head_only: bool) {
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
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        r.status,
        body.len(),
        if head_only { "" } else { body.as_str() }
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

/// The body a request announced: reading it fails if the connection ends before all of it
/// came.
struct Announced<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Announced<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let max = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the body ended before its Content-Length",
            ));
        }
        self.left -= n as u64;
        Ok(n)
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

/// One connection, one request, on a thread of its own: the whole request must arrive
/// within `deadline`, and a body is read only after the rule and the secret are checked, and
/// only as far as its 64 KiB cap.
fn serve_one(cfg: &Config, secret: &str, stream: TcpStream, deadline: Duration) {
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut reader = BufReader::new(Deadline {
        stream: &stream,
        until: std::time::Instant::now() + deadline,
    });
    let (method, target, headers) = match read_head(&mut reader) {
        Ok(h) => h,
        Err(status) => {
            respond(
                &stream,
                reply(status, json!({"error": "could not read the request"})),
                false,
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
    // The route and the secret before anything about the body.
    if let Err(r) = route(cfg, secret, &method, &target, header("authorization")) {
        if r.status == 401 {
            say_refused(cfg, &target);
        }
        respond(&stream, r, method == "HEAD");
        return;
    }
    let length = match (header("transfer-encoding"), header("content-length")) {
        (Some(_), _) => {
            respond(
                &stream,
                reply(
                    411,
                    json!({"error": "send a Content-Length, not a chunked body"}),
                ),
                false,
            );
            return;
        }
        (None, None) => 0,
        (None, Some(v)) => match v.parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                respond(
                    &stream,
                    reply(400, json!({"error": "bad Content-Length"})),
                    false,
                );
                return;
            }
        },
    };
    if length > 0
        && length <= MAX_BODY as u64
        && header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue"))
    {
        let _ = (&stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = Announced {
        inner: reader,
        left: length.min(MAX_BODY as u64 + 1),
    };
    let store = match Store::open(&cfg.db_path()) {
        Ok(s) => s,
        Err(e) => {
            respond(
                &stream,
                reply(500, json!({"error": format!("{e:#}")})),
                false,
            );
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
    respond(&stream, r, method == "HEAD");
}

/// The daemon's line for a request refused for its secret. The path is the sender's text: it
/// is named only when it is a webhook rule's.
fn say_refused(cfg: &Config, target: &str) {
    let path = target.split('?').next().unwrap_or("");
    let rule = path
        .strip_prefix("/hooks/")
        .and_then(|n| cfg.rule(n))
        .filter(|r| matches!(r.source, Source::Webhook));
    crate::daemon::say(&match rule {
        Some(r) => format!("webhook /hooks/{}: refused (401): wrong or missing secret", r.name),
        None => "webhook: refused (401) a request under /hooks/ to no webhook rule's path: wrong or missing secret".to_string(),
    });
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
                    respond(
                        &stream,
                        reply(503, json!({"error": "busy, try again"})),
                        false,
                    );
                } else {
                    serve_one(&cfg, &secret, stream, IO_TIMEOUT);
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
        // A folder (and store) of its own for each test, as they run at once.
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gt-webhook-{}-{}-{}",
            std::process::id(),
            crate::util::now(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("in")).unwrap();
        std::fs::write(dir.join("r.yaml"), "title: t\n").unwrap();
        // state_dir in the test's folder: the default is the user's own state folder.
        let text = "state_dir: state\nwebhook: {secret: 0123456789abcdef}\nrules:\n  - {name: form, webhook: true, recipe: r.yaml}\n  - {name: files, folder: in, recipe: r.yaml}\n";
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
        assert_eq!(call(&c, "GET", "/nope", None, ""), (404, 0));
        let big = "x".repeat(MAX_BODY + 1);
        assert_eq!(call(&c, "POST", "/hooks/form", ok, &big), (413, 0));
    }

    #[test]
    fn without_the_secret_no_rule_name_or_method_is_told() {
        let c = cfg();
        // A rule's name, a folder rule's, none, and a wrong method: all the same 401.
        for (method, path) in [
            ("POST", "/hooks/form"),
            ("POST", "/hooks/files"),
            ("POST", "/hooks/nope"),
            ("GET", "/hooks/form"),
            ("PUT", "/hooks/nope"),
        ] {
            assert_eq!(
                call(&c, method, path, None, "{}"),
                (401, 0),
                "{method} {path}"
            );
        }
        // Nor the body's size, nor how it is sent.
        let big = format!(
            "POST /hooks/form HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(exchange(&c, &big).starts_with("HTTP/1.1 401"));
        let chunked = "POST /hooks/nope HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(exchange(&c, chunked).starts_with("HTTP/1.1 401"));
        let ok = "Authorization: Bearer 0123456789abcdef\r\n";
        let big = format!(
            "POST /hooks/form HTTP/1.1\r\n{ok}Content-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(exchange(&c, &big).starts_with("HTTP/1.1 413"));
        let chunked =
            format!("POST /hooks/form HTTP/1.1\r\n{ok}Transfer-Encoding: chunked\r\n\r\n");
        assert!(exchange(&c, &chunked).starts_with("HTTP/1.1 411"));
    }

    /// Sends raw bytes to serve_one, which gives the request `deadline` to arrive, and
    /// returns the whole reply. With `close`, the client then stops sending (the connection's
    /// write side ends); else it sends nothing more but keeps the connection open.
    fn exchange_with(c: &Config, request: &str, deadline: Duration, close: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        let c = c.clone();
        let t = std::thread::spawn(move || serve_one(&c, "0123456789abcdef", server, deadline));
        client.write_all(request.as_bytes()).unwrap();
        if close {
            client.shutdown(std::net::Shutdown::Write).unwrap();
        }
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut out = String::new();
        let _ = client.read_to_string(&mut out);
        t.join().unwrap();
        out
    }

    /// The reply's status line, the client never sending the body it announced.
    fn exchange(c: &Config, request: &str) -> String {
        let out = exchange_with(c, request, IO_TIMEOUT, false);
        out.lines().next().unwrap_or("").to_string()
    }

    #[test]
    fn announced_bodies_are_not_waited_for_or_drained() {
        let c = cfg();
        let start = std::time::Instant::now();
        let ok = "Authorization: Bearer 0123456789abcdef\r\n";
        let huge =
            format!("POST /hooks/form HTTP/1.1\r\n{ok}Content-Length: 100000000000000000\r\n\r\n");
        assert_eq!(exchange(&c, &huge), "HTTP/1.1 413 Payload Too Large");
        let stalled = "POST /hooks/form HTTP/1.1\r\nContent-Length: 5000\r\n\r\n";
        assert_eq!(exchange(&c, stalled), "HTTP/1.1 401 Unauthorized");
        let chunked =
            format!("POST /hooks/form HTTP/1.1\r\n{ok}Transfer-Encoding: chunked\r\n\r\n");
        assert_eq!(exchange(&c, &chunked), "HTTP/1.1 411 Length Required");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_body_that_stalls_gets_408_and_one_cut_short_400() {
        let c = cfg();
        let head = "POST /hooks/form HTTP/1.1\r\nAuthorization: Bearer 0123456789abcdef\r\nContent-Length: 20\r\n\r\n{\"name\":";
        let first = |out: String| out.lines().next().unwrap_or("").to_string();
        let start = std::time::Instant::now();
        let stalled = exchange_with(&c, head, Duration::from_millis(500), false);
        assert_eq!(first(stalled), "HTTP/1.1 408 Request Timeout");
        assert!(start.elapsed() < Duration::from_secs(3));
        let cut = exchange_with(&c, head, Duration::from_secs(5), true);
        assert_eq!(first(cut), "HTTP/1.1 400 Bad Request");
        // Headers that stall are 408 too.
        let partial = "POST /hooks/form HTTP/1.1\r\nContent-Le";
        let out = exchange_with(&c, partial, Duration::from_millis(500), false);
        assert_eq!(first(out), "HTTP/1.1 408 Request Timeout");
    }

    #[test]
    fn health_answers_get_and_head() {
        let c = cfg();
        let get = exchange_with(&c, "GET /health HTTP/1.1\r\n\r\n", IO_TIMEOUT, true);
        assert!(
            get.starts_with("HTTP/1.1 200 OK") && get.ends_with(r#"{"ok":true}"#),
            "{get}"
        );
        let head = exchange_with(&c, "HEAD /health HTTP/1.1\r\n\r\n", IO_TIMEOUT, true);
        assert!(
            head.starts_with("HTTP/1.1 200 OK") && head.ends_with("\r\n\r\n"),
            "{head}"
        );
        assert!(head.contains("Content-Length: 11\r\n"), "{head}");
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
