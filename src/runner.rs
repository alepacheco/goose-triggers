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
    cmd.args(["--output-format", "stream-json"]);
    cmd.current_dir(wd)
        .env("GOOSE_MODE", "auto")
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

/// The tool responses in a stream-json line: (tool call id, whether the tool ran) for each.
fn tool_responses(v: &Value) -> Vec<(String, bool)> {
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
                    let id = p.get("id").and_then(Value::as_str)?.to_string();
                    // A tool that ran answers "success" without isError. A denial, an unknown
                    // tool or a call goose could not parse answers "error", or "success" with
                    // isError, and goose's hooks are not run for the last two.
                    let ran = p.pointer("/toolResult/status").and_then(Value::as_str)
                        == Some("success")
                        && p.pointer("/toolResult/value/isError")
                            .and_then(Value::as_bool)
                            != Some(true);
                    Some((id, ran))
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
    last_error: Option<String>,
    unseen_call: Option<String>,
}

fn run_goose(ctx: &Ctx, run: &Run, mode: &Mode) -> Result<()> {
    let store = Store::open(&ctx.db)?;
    let wd = PathBuf::from(&run.working_dir);
    std::fs::create_dir_all(&wd).with_context(|| format!("creating {}", wd.display()))?;
    prepare_working_dir(ctx, run, &wd)?;
    if let Mode::Start = mode {
        crate::recipe::check_values_stay_text(
            &ctx.cfg.goose,
            Path::new(&run.recipe),
            &run.params,
            &wd,
            hidden_env(ctx),
        )?;
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
    finish(&store, run, mode, &wd, watched)
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
    let mut last_error: Option<String> = None;
    let mut unseen_call: Option<String> = None;
    let mut checked = Instant::now() - Duration::from_secs(1);

    // One line of goose's output.
    let mut on_line = |line: &str,
                       hold: &Option<(Instant, Option<String>)>,
                       stopper: &mut Stopper|
     -> Result<()> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Ok(());
        };
        match v.get("type").and_then(Value::as_str) {
            Some("complete") => saw_complete = true,
            Some("error") => {
                last_error = v.get("error").or_else(|| v.get("message")).map(|e| {
                    e.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| e.to_string())
                })
            }
            _ => {}
        }
        for (id, ran) in tool_responses(&v) {
            if ran && !store.hook_saw(run.id, &id)? {
                // goose ran a tool that goose-triggers' hook never saw: the plugin is not
                // active, so nothing would be held.
                unseen_call.get_or_insert(id.clone());
                stopper.stop(Kill::NoHook);
            }
            if hold.as_ref().and_then(|h| h.1.as_deref()) == Some(id.as_str()) {
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
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    answered.extend(tool_responses(&v).into_iter().map(|(id, _)| id));
                }
            }
            on_line(&line, &hold, &mut stopper)?;
        }
        if let Some(st) = child.try_wait()? {
            // goose exited: take what is left of its output.
            while let Ok(line) = rx.recv_timeout(Duration::from_millis(300)) {
                on_line(&line, &hold, &mut stopper)?;
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
        last_error,
        unseen_call,
    })
}

fn finish(store: &Store, run: &Run, mode: &Mode, wd: &Path, w: Watched) -> Result<()> {
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
            "goose ran a tool (call {call}) that goose-triggers' hook never saw, so nothing could be held; the run was stopped. Check that goose loads plugins from {}",
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
    if cur.session_id.is_none() {
        return fail(&format!(
            "goose did not run goose-triggers' plugin hooks (no session was recorded); check that goose loads plugins from {}",
            wd.join(".agents/plugins").display()
        ));
    }
    if w.status.success() && w.saw_complete {
        if store.transition(id, &[state::RUNNING], state::DONE, None)? {
            say_log("done")?;
        }
        return Ok(());
    }
    fail(&format!(
        "goose exited with {}{}; see goose-triggers log {id}",
        w.status,
        w.last_error
            .map(|e| format!(": {}", short(&e, 300)))
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
        assert_eq!(
            tool_responses(&v),
            vec![
                ("call_7".to_string(), false),
                ("call_8".to_string(), true),
                ("call_9".to_string(), false)
            ]
        );
        let v = json!({"type": "message", "message": {"role": "assistant", "content": [
            {"type": "toolRequest", "id": "call_7"}
        ]}});
        assert!(tool_responses(&v).is_empty());
        assert!(tool_responses(&json!({"type": "complete"})).is_empty());
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
