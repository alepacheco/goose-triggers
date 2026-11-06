//! Runs goose for one run: a new headless recipe run, or the same session resumed after an
//! approval. Watches the store and goose's output while goose runs, and stops goose once
//! the run is held.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::Config;
use crate::daemon::{say, STOP};
use crate::hook::{ENV_DB, ENV_RUN};
use crate::store::{state, Run, Store};
use crate::util::{call_line, is_cli_agent_provider, short};

/// Once a run is held, goose gets this long to record the denial before it is stopped anyway.
const HOLD_GRACE: Duration = Duration::from_secs(10);
/// goose is stopped with SIGINT, which makes it cancel its turn, stop the tool processes it
/// started and save the session. If it is still there after this long it gets SIGTERM, and
/// after twice this long SIGKILL (both to its process group).
const STOP_GRACE: Duration = Duration::from_secs(5);
/// The approved call is repeated in the resume message when its input is at most this long;
/// a longer one is referred to (goose's session holds it), which keeps the message within
/// what one command-line argument may carry.
const RESUME_INPUT_MAX: usize = 4000;
/// A failure reason quoted from goose is cut to this many characters.
const REASON_MAX: usize = 300;
/// The lines of goose's stderr a failure reason may come from, and the bytes read for them.
const STDERR_LINES: usize = 20;
const TAIL_BYTES: u64 = 16 * 1024;

#[derive(Clone)]
pub struct Ctx {
    pub cfg: Arc<Config>,
    pub exe: PathBuf,
    pub db: PathBuf,
}

pub enum Mode {
    Start,
    /// Resume the run's session with this message.
    Resume(String),
}

/// The message that resumes a run after the user approved its held call.
pub fn resume_text(tool: &str, input: &str) -> String {
    if input.len() <= RESUME_INPUT_MAX {
        format!(
            "The user approved the call that goose-triggers held: tool `{tool}` with input {input}. Make exactly that call again now, with the same input, then carry on with the task."
        )
    } else {
        format!(
            "The user approved the call to `{tool}` that goose-triggers held, the last one you made. Make exactly that call again now, with the same input, then carry on with the task."
        )
    }
}

pub fn spawn(ctx: Ctx, run: Run, mode: Mode) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let id = run.id;
        if let Err(e) = run_goose(&ctx, &run, &mode) {
            if STOP.load(Ordering::SeqCst) {
                // goose-triggers is stopping (which may be what broke this): a run that had not
                // started stays queued or approved for the next start; a running one is
                // settled at the next start.
                say(&format!("run {id} ({}): left for the next start", run.rule));
                return;
            }
            let msg = format!("{e:#}");
            say(&format!("run {id} ({}): failed: {msg}", run.rule));
            if let Ok(store) = Store::open(&ctx.db) {
                let _ = store.transition(
                    id,
                    &[state::QUEUED, state::APPROVED, state::RUNNING],
                    state::FAILED,
                    Some(&msg),
                );
                let _ = store.log(Some(id), &format!("failed: {msg}"));
            }
        }
    })
}

/// The provider a run's goose would use: the rule's, the recipe's, `GOOSE_PROVIDER`, or
/// goose's config.
pub fn effective_provider(run: &Run) -> Option<String> {
    if let Some(p) = &run.provider {
        return Some(p.clone());
    }
    if let Some(p) = crate::recipe::read(Path::new(&run.recipe))
        .ok()
        .and_then(|i| i.provider)
    {
        return Some(p);
    }
    if let Some(p) = std::env::var("GOOSE_PROVIDER")
        .ok()
        .filter(|p| !p.is_empty())
    {
        return Some(p);
    }
    let text =
        std::fs::read_to_string(crate::plugin::goose_config_dir().join("config.yaml")).ok()?;
    let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    ["active_provider", "GOOSE_PROVIDER"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|p| p.as_str()).map(str::to_string))
}

fn check_provider(run: &Run) -> Result<()> {
    if let Some(p) = effective_provider(run) {
        if is_cli_agent_provider(&p) {
            bail!(
                "provider {p} hands the run to another agent, which runs its own tools: goose's hooks do not see those calls, so the rule's holds could not be enforced. Use a provider for which goose runs the tools (an API provider or a local model)"
            );
        }
    }
    Ok(())
}

/// Checks a held run's working folder before goose starts there: goose-triggers' plugin is
/// written afresh and enabled, and no other plugin is there, whose hooks could run commands
/// that no hold sees (a run with an unheld file tool could have written one).
fn prepare_working_dir(ctx: &Ctx, run: &Run, wd: &Path) -> Result<()> {
    crate::plugin::install(wd, &ctx.exe)?;
    crate::plugin::preflight(wd)?;
    if !run.holds.is_empty() {
        let others = crate::plugin::other_plugins(wd);
        if !others.is_empty() {
            bail!(
                "{} holds other goose plugins ({}), whose hooks could run commands no hold sees: remove them, or give the rule a working folder of its own",
                wd.join(".agents/plugins").display(),
                others.join(", ")
            );
        }
        check_provider(run)?;
    }
    Ok(())
}

fn signal_pid(pid: u32, sig: libc::c_int) {
    // SAFETY: signals the goose process this runner started.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

fn signal_group(pid: u32, sig: libc::c_int) {
    // SAFETY: goose runs in a process group of its own (its pid): this signals goose and the
    // processes of that group, nothing else.
    unsafe {
        libc::kill(-(pid as libc::pid_t), sig);
    }
}

/// The environment variable holding the webhook secret, kept from goose.
fn hidden_env(ctx: &Ctx) -> Option<&str> {
    ctx.cfg
        .webhook
        .as_ref()
        .and_then(|w| w.secret_env.as_deref())
}

fn goose_command(ctx: &Ctx, run: &Run, mode: &Mode, wd: &Path, stderr: File) -> Result<Command> {
    let mut cmd = Command::new(&ctx.cfg.goose);
    cmd.arg("run");
    match mode {
        Mode::Start => {
            cmd.arg("--recipe").arg(&run.recipe);
            for (k, v) in &run.params {
                cmd.arg("--params").arg(format!("{k}={v}"));
            }
            cmd.arg("--name")
                .arg(format!("goose-triggers {} run {}", run.rule, run.id));
        }
        Mode::Resume(text) => {
            let sid = run
                .session_id
                .as_deref()
                .context("the run has no goose session to resume")?;
            cmd.args(["--resume", "--session-id", sid, "--text", text]);
        }
    }
    if let Some(p) = &run.provider {
        cmd.arg("--provider").arg(p);
    }
    if let Some(m) = &run.model {
        cmd.arg("--model").arg(m);
    }
    if let Some(t) = run.max_turns {
        cmd.arg("--max-turns").arg(t.to_string());
    }
    // Quiet: goose prints neither the recipe's parameters (the event's values) nor its banner,
    // so what it prints besides its stream-json events is its own messages.
    cmd.args(["--quiet", "--output-format", "stream-json"]);
    cmd.current_dir(wd)
        .env("GOOSE_MODE", "auto")
        // A message starting with `/` would otherwise run one of the user's slash-command
        // recipes, whose extensions start where no hook sees them.
        .env("GOOSE_SLASH_COMMANDS_ENABLED", "false")
        .env(ENV_RUN, run.id.to_string())
        .env(ENV_DB, &ctx.db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));
    if let Some(var) = hidden_env(ctx) {
        // goose and the commands it runs do not need the webhook secret.
        cmd.env_remove(var);
    }
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe. If goose-triggers dies, goose gets SIGINT and
        // cancels its turn as it does when stopped.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGINT);
                Ok(())
            });
        }
    }
    Ok(cmd)
}

/// A tool response in goose's output.
#[derive(Debug, PartialEq, Eq)]
struct ToolResponse {
    id: String,
    /// "success": goose dispatched the call. "error": it did not (a hook denied it).
    success: bool,
    /// The result is an error: the tool failed, or goose refused an unknown tool or a call it
    /// could not parse without running its hooks.
    is_error: bool,
}

/// The tool responses in a stream-json line.
fn tool_responses(v: &Value) -> Vec<ToolResponse> {
    if v.get("type").and_then(Value::as_str) != Some("message") {
        return Vec::new();
    }
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("toolResponse"))
                .filter_map(|p| {
                    Some(ToolResponse {
                        id: p.get("id").and_then(Value::as_str)?.to_string(),
                        success: p.pointer("/toolResult/status").and_then(Value::as_str)
                            == Some("success"),
                        is_error: p
                            .pointer("/toolResult/value/isError")
                            .and_then(Value::as_bool)
                            == Some(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Kill {
    Hold,
    Stop,
    NoHook,
}

/// A goose process being stopped: SIGINT, then SIGTERM and SIGKILL to its group.
struct Stopper {
    pid: u32,
    why: Option<(Kill, Instant)>,
    sent: u8,
}

impl Stopper {
    fn new(pid: u32) -> Stopper {
        Stopper {
            pid,
            why: None,
            sent: 0,
        }
    }

    fn stop(&mut self, why: Kill) {
        if self.why.is_none() {
            signal_pid(self.pid, libc::SIGINT);
            self.why = Some((why, Instant::now()));
            self.sent = 1;
        }
    }

    fn escalate(&mut self) {
        let Some((_, at)) = self.why else { return };
        if self.sent == 1 && at.elapsed() > STOP_GRACE {
            signal_group(self.pid, libc::SIGTERM);
            self.sent = 2;
        } else if self.sent == 2 && at.elapsed() > 2 * STOP_GRACE {
            signal_group(self.pid, libc::SIGKILL);
            self.sent = 3;
        }
    }

    fn reason(&self) -> Option<Kill> {
        self.why.map(|w| w.0)
    }
}

/// Stops goose and waits for it: used when watching it fails part-way.
fn stop_and_reap(child: &mut Child) {
    let mut s = Stopper::new(child.id());
    s.stop(Kill::Stop);
    loop {
        if let Ok(Some(_)) | Err(_) = child.try_wait() {
            return;
        }
        s.escalate();
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What watching goose saw.
struct Watched {
    status: ExitStatus,
    kill: Option<Kill>,
    saw_complete: bool,
    unseen_call: Option<String>,
    said: Said,
}

fn run_goose(ctx: &Ctx, run: &Run, mode: &Mode) -> Result<()> {
    let store = Store::open(&ctx.db)?;
    let wd = PathBuf::from(&run.working_dir);
    std::fs::create_dir_all(&wd).with_context(|| format!("creating {}", wd.display()))?;
    prepare_working_dir(ctx, run, &wd)?;
    if let Mode::Start = mode {
        let rendered = crate::recipe::check_values_stay_text(
            &ctx.cfg.goose,
            Path::new(&run.recipe),
            &run.params,
            &wd,
            hidden_env(ctx),
        )?;
        if !run.holds.is_empty() && run.provider.is_none() {
            if let Some(p) = crate::recipe::rendered_provider(&rendered) {
                if is_cli_agent_provider(&p) {
                    bail!("the recipe asks for provider {p}, which hands the run to another agent that runs its own tools: goose's hooks do not see those calls, so the rule's holds could not be enforced");
                }
            }
        }
    }

    let runs_dir = ctx.cfg.runs_dir();
    crate::util::create_private_dir(&runs_dir)?;
    let private = |name: String| {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(runs_dir.join(name))
    };
    let stderr = private(format!("{}.stderr", run.id))?;
    // The file keeps every goose process of the run (start and resumes): this one writes from here.
    let stderr_from = stderr.metadata()?.len();
    let out_log = private(format!("{}.jsonl", run.id))?;
    let mut cmd = goose_command(ctx, run, mode, &wd, stderr)?;

    if STOP.load(Ordering::SeqCst) {
        // goose-triggers is stopping: leave the run to start at the next start.
        return Ok(());
    }
    let from = match mode {
        Mode::Start => state::QUEUED,
        Mode::Resume(_) => state::APPROVED,
    };
    if !store.transition(run.id, &[from], state::RUNNING, None)? {
        return Ok(());
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting goose ({})", ctx.cfg.goose))?;
    let watched = match watch(&store, run, mode, &wd, &mut child, out_log) {
        Ok(w) => w,
        Err(e) => {
            stop_and_reap(&mut child);
            let _ = store.set_pid(run.id, None);
            return Err(e);
        }
    };
    store.set_pid(run.id, None)?;
    let stderr = tail(
        &runs_dir.join(format!("{}.stderr", run.id)),
        stderr_from,
        STDERR_LINES,
    );
    finish(&store, run, mode, &wd, &stderr, watched)
}

fn watch(
    store: &Store,
    run: &Run,
    mode: &Mode,
    wd: &Path,
    child: &mut Child,
    mut out_log: File,
) -> Result<Watched> {
    let pid = child.id();
    store.set_pid(run.id, Some(pid as i64))?;
    let what = match mode {
        Mode::Start => format!("goose started (pid {pid}) in {}", wd.display()),
        Mode::Resume(_) => format!(
            "goose resumed session {} (pid {pid})",
            run.session_id.as_deref().unwrap_or("?")
        ),
    };
    store.log(Some(run.id), &what)?;
    say(&format!("run {} ({}): {what}", run.id, run.rule));

    let stdout = child.stdout.take().context("goose's output")?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(out_log, "{line}");
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut stopper = Stopper::new(pid);
    let mut hold: Option<(Instant, Option<String>)> = None;
    let mut saw_complete = false;
    let mut said = Said::default();
    let mut unseen_call: Option<String> = None;
    let mut checked = Instant::now() - Duration::from_secs(1);

    // Decisions the hook had made for this run before this goose started: one made since
    // shows goose loaded the plugin.
    let decisions_before = store.decision_count(run.id)?;
    let new_session = matches!(mode, Mode::Start);

    // One line of goose's output. After goose has exited, nothing is signalled.
    let mut on_line = |line: &str,
                       hold: &Option<(Instant, Option<String>)>,
                       stopper: &mut Stopper,
                       exited: bool|
     -> Result<()> {
        let (text, event) = split_line(line);
        said.take(text, event.as_ref());
        let Some(v) = event else {
            return Ok(());
        };
        if v.get("type").and_then(Value::as_str) == Some("complete") {
            saw_complete = true;
        }
        for r in tool_responses(&v) {
            if r.success && !store.hook_saw(run.id, &r.id)? {
                // A call the hook never saw. goose refuses an unknown tool or a call it cannot
                // parse with an error result and no hook; any other unseen call ran without
                // the hook, so the plugin is not active and nothing would be held.
                let plugin_shown = store.decision_count(run.id)? > decisions_before
                    || (new_session && store.run(run.id)?.is_some_and(|c| c.session_id.is_some()));
                if !r.is_error || !plugin_shown {
                    unseen_call.get_or_insert(r.id.clone());
                    if !exited {
                        stopper.stop(Kill::NoHook);
                    }
                }
            }
            if !exited && hold.as_ref().and_then(|h| h.1.as_deref()) == Some(r.id.as_str()) {
                // goose has recorded the denial; stop it now.
                stopper.stop(Kill::Hold);
            }
        }
        Ok(())
    };

    // Responses seen before the hold was noticed.
    let mut answered: Vec<String> = Vec::new();
    let status = loop {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(100)) {
            if hold.is_none() {
                if let (_, Some(v)) = split_line(&line) {
                    answered.extend(tool_responses(&v).into_iter().map(|r| r.id));
                }
            }
            on_line(&line, &hold, &mut stopper, false)?;
        }
        if let Some(st) = child.try_wait()? {
            // goose exited: take what is left of its output.
            while let Ok(line) = rx.recv_timeout(Duration::from_millis(300)) {
                on_line(&line, &hold, &mut stopper, true)?;
            }
            break st;
        }
        if checked.elapsed() < Duration::from_millis(200) {
            continue;
        }
        checked = Instant::now();
        let cur = store.run(run.id)?.context("the run left the store")?;
        if hold.is_none() && cur.state == state::HELD {
            let call = store.pending_call(run.id)?;
            let call_id = call.as_ref().and_then(|c| c.tool_call_id.clone());
            if let Some(c) = &call {
                say(&format!(
                    "run {} ({}): held: {} {}",
                    run.id,
                    run.rule,
                    c.tool_name,
                    call_line(&c.tool_input, 120)
                ));
            }
            if call_id.as_ref().is_none_or(|cid| answered.contains(cid)) {
                stopper.stop(Kill::Hold);
            }
            hold = Some((Instant::now(), call_id));
        }
        if hold
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() > HOLD_GRACE)
        {
            stopper.stop(Kill::Hold);
        }
        if STOP.load(Ordering::SeqCst) {
            stopper.stop(Kill::Stop);
        }
        stopper.escalate();
    };
    Ok(Watched {
        status,
        kill: stopper.reason(),
        saw_complete,
        unseen_call,
        said,
    })
}

/// One line of goose's output: the text goose printed itself, and the stream-json event on
/// it. goose prints its interrupt prompt without a line end, so an event can follow text on
/// the same line.
pub fn split_line(line: &str) -> (&str, Option<Value>) {
    let event = |s: &str| {
        serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_object)
    };
    if let Some(v) = event(line) {
        return ("", Some(v));
    }
    if let Some(at) = line.find("{\"type\":") {
        if let Some(v) = event(&line[at..]) {
            return (line[..at].trim(), Some(v));
        }
    }
    (line.trim(), None)
}

/// What goose said while it ran: its last error event, and its last lines of plain text
/// (with `--quiet`, its own messages, such as an error before the run starts).
#[derive(Default, Debug)]
struct Said {
    error: Option<String>,
    lines: Vec<String>,
}

impl Said {
    const KEEP: usize = 20;

    fn take(&mut self, text: &str, event: Option<&Value>) {
        if !text.is_empty() {
            self.lines.push(text.to_string());
            if self.lines.len() > Self::KEEP {
                self.lines.remove(0);
            }
        }
        if let Some(v) = event.filter(|v| v.get("type").and_then(Value::as_str) == Some("error")) {
            self.error = v.get("error").or_else(|| v.get("message")).map(|e| {
                e.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string())
            });
        }
    }
}

/// goose's words in plain lines: from its last line that starts with "error" on (goose
/// starts its errors so, and may explain them on the lines after), else its last three lines.
fn error_lines(lines: &[String]) -> Option<String> {
    let from = lines
        .iter()
        .rposition(|l| l.to_ascii_lowercase().starts_with("error"))
        .unwrap_or(lines.len().saturating_sub(3));
    let text = lines[from..].join(" / ");
    (!text.is_empty()).then_some(text)
}

/// Why goose failed, in goose's own words: its error event, else the error it printed, else
/// the end of what this goose process wrote on stderr. Shortened to `REASON_MAX` characters,
/// keeping its start, where goose names the error.
fn failure_reason(said: &Said, stderr: &[String]) -> Option<String> {
    said.error
        .clone()
        .or_else(|| error_lines(&said.lines))
        .or_else(|| error_lines(stderr))
        .map(|r| short(&r, REASON_MAX))
}

/// The last `n` lines that are not empty of what a file holds from byte `from` on, read from
/// at most its last `TAIL_BYTES`; bytes that are not UTF-8 show as U+FFFD.
pub fn tail(path: &Path, from: u64, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = from.max(len.saturating_sub(TAIL_BYTES));
    let mut buf = Vec::new();
    if start >= len
        || f.seek(SeekFrom::Start(start)).is_err()
        || f.take(len - start).read_to_end(&mut buf).is_err()
    {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > from {
        // Cut in the middle of the file: its first line may be the end of a longer one.
        lines.next();
    }
    let lines: Vec<String> = lines
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

fn finish(
    store: &Store,
    run: &Run,
    mode: &Mode,
    wd: &Path,
    stderr: &[String],
    w: Watched,
) -> Result<()> {
    let cur = store.run(run.id)?.context("the run left the store")?;
    let id = run.id;
    let say_log = |what: &str| -> Result<()> {
        store.log(Some(id), what)?;
        say(&format!("run {id} ({}): {what}", run.rule));
        Ok(())
    };
    // Each outcome is logged only if the run was still running when it was recorded.
    let fail = |msg: &str| -> Result<()> {
        if store.transition(id, &[state::RUNNING], state::FAILED, Some(msg))? {
            say_log(&format!("failed: {msg}"))?;
        }
        Ok(())
    };
    if cur.state == state::HELD {
        let how = match w.kill {
            Some(Kill::Hold) => "goose-triggers stopped goose at the hold",
            Some(Kill::Stop) => "goose-triggers was stopped; the run stays held",
            _ => "goose ended its turn at the hold",
        };
        return say_log(&format!(
            "{how}; the run waits for the user: goose-triggers approve {id} | reject {id}"
        ));
    }
    if cur.state != state::RUNNING {
        return Ok(());
    }
    if let Some(call) = &w.unseen_call {
        return fail(&format!(
            "goose ran a tool (call {}) that goose-triggers' hook never saw, so nothing could be held; the run was stopped. Check that goose loads plugins from {}",
            short(call, 80),
            wd.join(".agents/plugins").display()
        ));
    }
    if w.kill == Some(Kill::Stop) {
        if matches!(mode, Mode::Resume(_)) && store.has_open_permission(id)? {
            // The approved call was not made yet: resume again at the next start.
            if !store.transition(id, &[state::RUNNING], state::APPROVED, None)? {
                return Ok(());
            }
            return say_log("goose-triggers was stopped before the approved call was made; the run resumes when it starts again");
        }
        return fail("goose-triggers was stopped while the run was going");
    }
    if !w.status.success() {
        return fail(&format!(
            "goose exited with {}{}; see goose-triggers log {id}",
            w.status,
            failure_reason(&w.said, stderr)
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        ));
    }
    if cur.session_id.is_none() {
        return fail(&format!(
            "goose did not run goose-triggers' plugin hooks (no session was recorded); check that goose loads plugins from {}",
            wd.join(".agents/plugins").display()
        ));
    }
    if w.saw_complete {
        if store.transition(id, &[state::RUNNING], state::DONE, None)? {
            say_log("done")?;
        }
        return Ok(());
    }
    fail(&format!(
        "goose exited ({}) without finishing its turn{}; see goose-triggers log {id}",
        w.status,
        w.said
            .error
            .map(|e| format!(": {}", short(&e, REASON_MAX)))
            .unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_tool_responses_and_whether_they_ran() {
        let v = json!({"type": "message", "message": {"role": "user", "content": [
            {"type": "toolResponse", "id": "call_7", "toolResult": {"status": "error", "error": "denied"}},
            {"type": "toolResponse", "id": "call_8", "toolResult": {"status": "success", "value": {"isError": false}}},
            {"type": "toolResponse", "id": "call_9", "toolResult": {"status": "success", "value": {"isError": true}}}
        ]}});
        let r = |id: &str, success: bool, is_error: bool| ToolResponse {
            id: id.into(),
            success,
            is_error,
        };
        assert_eq!(
            tool_responses(&v),
            vec![
                r("call_7", false, false),
                r("call_8", true, false),
                r("call_9", true, true)
            ]
        );
        let v = json!({"type": "message", "message": {"role": "assistant", "content": [
            {"type": "toolRequest", "id": "call_7"}
        ]}});
        assert!(tool_responses(&v).is_empty());
        assert!(tool_responses(&json!({"type": "complete"})).is_empty());
    }

    /// Feeds goose's output, line by line, to what `watch` keeps of it.
    fn said_of(stdout: &str) -> Said {
        let mut said = Said::default();
        for line in stdout.lines() {
            let (text, event) = split_line(line);
            said.take(text, event.as_ref());
        }
        said
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    // The fixtures are goose 1.57.0's own stdout and stderr in `goose run --quiet
    // --output-format stream-json`, as goose-triggers runs it, and one without `--quiet`.
    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../tests/goose-output/", $name))
        };
    }

    #[test]
    fn failure_reason_is_the_error_goose_printed_not_the_event_values() {
        // No provider configured: goose prints its error on stdout and nothing on stderr.
        let r = failure_reason(&said_of(fixture!("no-provider.stdout")), &[]);
        assert_eq!(
            r.as_deref(),
            Some("error: No provider configured. Run 'goose configure' first.")
        );
        // Without --quiet goose first prints the recipe's parameters, the event's values.
        let r = failure_reason(&said_of(fixture!("no-provider-without-quiet.stdout")), &[]);
        assert_eq!(
            r.as_deref(),
            Some("error: No provider configured. Run 'goose configure' first.")
        );
        // An error explained on the lines after it keeps its first line.
        let r = failure_reason(&said_of(fixture!("unknown-provider.stdout")), &[]).unwrap();
        assert!(
            r.starts_with("error: Error Unknown provider: nosuch. / Please check"),
            "{r}"
        );
        assert!(r.chars().count() <= REASON_MAX, "{r}");
    }

    #[test]
    fn failure_reason_prefers_goose_s_error_event() {
        let r = failure_reason(
            &said_of(fixture!("network-error.stdout")),
            &lines(fixture!("network-error.stderr")),
        )
        .unwrap();
        assert!(
            r.starts_with("Network error: Could not connect to 127.0.0.1:9"),
            "{r}"
        );
        // goose's interrupt prompt has no line end: its error event follows on the same line.
        let said = said_of(fixture!("interrupted.stdout"));
        assert_eq!(said.error.as_deref(), Some("Headless run interrupted"));
        assert_eq!(said.lines, ["Yes — what would you like me to do?"]);
    }

    #[test]
    fn a_kept_0_0_1_run_log_gives_goose_s_error_not_its_banner() {
        // goose 1.57.0's first process of a selftest run under goose-triggers 0.0.1 (no
        // --quiet), stopped at the hold: parameters, banner, two events, then the interrupt
        // prompt with the error event after it.
        let said = said_of(fixture!("held-0.0.1.stdout"));
        assert_eq!(said.error.as_deref(), Some("Headless run interrupted"));
        assert_eq!(
            failure_reason(&said, &[]).as_deref(),
            Some("Headless run interrupted")
        );
        assert!(said.lines.iter().any(|l| l.contains("order: A-1001")));
    }

    #[test]
    fn failure_reason_falls_back_to_stderr_and_the_last_lines() {
        let said = Said::default();
        assert_eq!(failure_reason(&said, &[]), None);
        let r = failure_reason(&said, &lines(fixture!("interrupted.stderr")));
        assert_eq!(r.as_deref(), Some("Error: Headless run interrupted"));
        let panic = lines("thread 'main' panicked at src/main.rs:3:5:\nsomething broke\nnote: run with `RUST_BACKTRACE=1`\n");
        let r = failure_reason(&said, &panic).unwrap();
        assert!(r.starts_with("thread 'main' panicked"), "{r}");
        let long = vec![format!("error: {}", "x".repeat(1000))];
        let r = failure_reason(&said, &long).unwrap();
        assert!(r.starts_with("error: xxx") && r.chars().count() == REASON_MAX);
    }

    #[test]
    fn a_tool_response_after_goose_s_text_is_read() {
        let line = r#"Yes — what would you like me to do?{"type":"message","message":{"role":"user","content":[{"type":"toolResponse","id":"c1","toolResult":{"status":"success","value":{"isError":false}}}]}}"#;
        let (text, event) = split_line(line);
        assert_eq!(text, "Yes — what would you like me to do?");
        assert_eq!(
            tool_responses(&event.unwrap()),
            vec![ToolResponse {
                id: "c1".into(),
                success: true,
                is_error: false
            }]
        );
    }

    #[test]
    fn split_line_reads_plain_text_and_events() {
        assert_eq!(split_line("  error: boom  "), ("error: boom", None));
        let (t, v) = split_line(r#"{"type":"complete"}"#);
        assert_eq!((t, v.unwrap()["type"].as_str()), ("", Some("complete")));
        // Text that is JSON but no event stays text.
        assert_eq!(split_line("42"), ("42", None));
        assert_eq!(
            split_line(r#"a {"type": broken"#),
            (r#"a {"type": broken"#, None)
        );
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gt-runner-{}-{}-{name}",
            std::process::id(),
            crate::util::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn tail_reads_only_this_process_s_stderr() {
        let p = temp("1.stderr");
        std::fs::write(&p, "Error: Headless run interrupted\n").unwrap();
        let from = std::fs::metadata(&p).unwrap().len();
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"Error: provider down\n\n").unwrap();
        assert_eq!(tail(&p, from, 20), ["Error: provider down"]);
        assert_eq!(
            tail(&p, 0, 20),
            ["Error: Headless run interrupted", "Error: provider down"]
        );
        assert_eq!(tail(&p, 0, 1), ["Error: provider down"]);
        let len = std::fs::metadata(&p).unwrap().len();
        assert!(tail(&p, len, 20).is_empty());
        assert!(tail(&p.with_extension("none"), 0, 20).is_empty());
    }

    #[test]
    fn tail_survives_bytes_that_are_not_utf8_and_big_files() {
        let p = temp("2.stderr");
        std::fs::write(&p, b"Error: bad \xff\xfe bytes\n").unwrap();
        let t = tail(&p, 0, 20);
        assert_eq!(t.len(), 1);
        assert!(t[0].starts_with("Error: bad ") && t[0].contains('\u{fffd}'));
        // Only the end of a big file is read; the line it was cut in is dropped.
        let mut big = "y".repeat(2 * TAIL_BYTES as usize);
        big.push_str("\nError: the last words\n");
        std::fs::write(&p, big).unwrap();
        assert_eq!(tail(&p, 0, 20), ["Error: the last words"]);
    }

    #[test]
    fn goose_runs_quiet_so_it_does_not_print_the_event_values() {
        let dir = temp("cfg").parent().unwrap().to_path_buf();
        std::fs::write(dir.join("r.yaml"), "title: t\n").unwrap();
        let cfg = Config::parse(
            "rules:\n  - {name: form, webhook: true, recipe: r.yaml}\nwebhook: {secret: 0123456789abcdef}\n",
            &dir.join("c.yaml"),
        )
        .unwrap();
        let ctx = Ctx {
            cfg: Arc::new(cfg),
            exe: PathBuf::from("/usr/bin/goose-triggers"),
            db: dir.join("state.db"),
        };
        let run = Run {
            id: 7,
            rule: "form".into(),
            event_id: 1,
            state: state::QUEUED.into(),
            recipe: dir.join("r.yaml").to_string_lossy().into_owned(),
            params: vec![("name".into(), "Test Person".into())],
            holds: vec!["shell".into()],
            working_dir: dir.to_string_lossy().into_owned(),
            provider: None,
            model: None,
            max_turns: None,
            session_id: Some("20261106_7".into()),
            pid: None,
            error: None,
            created_at: 0,
            updated_at: 0,
        };
        for mode in [Mode::Start, Mode::Resume("go on".into())] {
            let stderr = File::create(dir.join("7.stderr")).unwrap();
            let cmd = goose_command(&ctx, &run, &mode, &dir, stderr).unwrap();
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(args.iter().any(|a| a == "--quiet"), "{args:?}");
            assert!(
                args.windows(2)
                    .any(|w| w[0] == "--output-format" && w[1] == "stream-json"),
                "{args:?}"
            );
        }
    }

    #[test]
    fn resume_text_names_the_call_or_refers_to_a_long_one() {
        let t = resume_text("shell", r#"{"command":"ls"}"#);
        assert!(t.contains("`shell`") && t.contains(r#"{"command":"ls"}"#));
        let long = format!(r#"{{"content":"{}"}}"#, "x".repeat(RESUME_INPUT_MAX));
        let t = resume_text("write", &long);
        assert!(t.contains("`write`") && !t.contains("xxxx") && t.len() < 300);
    }
}
