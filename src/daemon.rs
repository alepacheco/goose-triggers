//! `goose-triggers run`: watches the rules' sources, queues a run for each event, starts
//! queued runs and resumes approved ones, until it gets SIGINT or SIGTERM.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::config::{Config, Rule, Source};
use crate::folder::FolderWatch;
use crate::runner::{self, Ctx, Mode};
use crate::store::{matches_hold, state, NewRun, Store};
use crate::util::{fmt_time, now, short};

/// Set by SIGINT or SIGTERM: everything winds down.
pub static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// One line of the daemon's output.
pub fn say(what: &str) {
    eprintln!("[{}] {}", fmt_time(now()), crate::util::clean(what));
}

/// Whether `pid` is still the goose process goose-triggers started for `run` (its
/// environment names the run), so a reused pid is never signalled.
fn our_goose(pid: i64, run: i64) -> bool {
    let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let want = format!("{}={run}", crate::hook::ENV_RUN);
    env.split(|b| *b == 0).any(|kv| kv == want.as_bytes())
}

/// After an unclean stop, a goose that goose-triggers started may still be winding down
/// (it got SIGINT when goose-triggers died): wait for it, then make sure it is gone.
fn wait_for_old_goose(pid: i64, run: i64) {
    let start = Instant::now();
    while our_goose(pid, run) {
        if start.elapsed() > Duration::from_secs(15) {
            // SAFETY: the process is ours, as its environment shows.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Holds the state folder's lock while `goose-triggers run` runs: one daemon per state folder.
pub fn lock(state_dir: &Path) -> Result<File> {
    crate::util::create_private_dir(state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;
    let path = state_dir.join("daemon.lock");
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: flock on a file descriptor this process owns.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!(
            "another `goose-triggers run` is using {} already",
            state_dir.display()
        );
    }
    Ok(f)
}

/// Whether a `goose-triggers run` holds the state folder now.
pub fn is_running(state_dir: &Path) -> bool {
    let Ok(f) = File::open(state_dir.join("daemon.lock")) else {
        return false;
    };
    // SAFETY: as above; a shared lock taken and dropped at once.
    let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0;
    if free {
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
    }
    !free
}

/// Stores an event and queues its run (a failed run, saying why, when the event lacks a
/// parameter the recipe needs). `None` when the event was stored before.
/// Tools that reach other tools or start new ones out of the hook's sight: code mode runs
/// tool calls inside JavaScript, the extension manager turns on extensions mid-run, and
/// delegate starts a subagent with a recipe's extensions. A rule that holds anything holds
/// these too.
pub const IMPLICIT_HOLDS: &[&str] = &["*execute_javascript", "*manage_extensions", "*delegate"];

/// A run's hold patterns: the rule's, and the implicit ones when it has any.
pub fn run_holds(rule_holds: &[String]) -> Vec<String> {
    let mut holds = rule_holds.to_vec();
    if !holds.is_empty() {
        for h in IMPLICIT_HOLDS {
            if !holds.iter().any(|x| x == h) {
                holds.push(h.to_string());
            }
        }
    }
    holds
}

pub fn queue_event(
    store: &Store,
    rule: &Rule,
    source: &str,
    fields: &BTreeMap<String, String>,
    dedupe: Option<&str>,
) -> Result<Option<(i64, i64)>> {
    let Some(event) = store.add_event(&rule.name, source, fields, dedupe)? else {
        return Ok(None);
    };
    let (params, error) = match crate::recipe::read(&rule.recipe)
        .and_then(|info| crate::recipe::params_for(&info, fields))
    {
        Ok(p) => (p, None),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    };
    let recipe = rule.recipe.to_string_lossy();
    // The real path: goose keys its plugin settings by the folder it runs in.
    let wd = std::fs::canonicalize(&rule.working_dir).unwrap_or_else(|_| rule.working_dir.clone());
    let wd = wd.to_string_lossy();
    let holds = run_holds(&rule.hold);
    let run = store.add_run(&NewRun {
        rule: &rule.name,
        event_id: event,
        recipe: &recipe,
        params: &params,
        holds: &holds,
        working_dir: &wd,
        provider: rule.provider.as_deref(),
        model: rule.model.as_deref(),
        max_turns: rule.max_turns,
        error: error.as_deref(),
    })?;
    let shown: Vec<String> = fields
        .iter()
        .filter(|(k, _)| k.as_str() != "body" || fields.len() == 1)
        .map(|(k, v)| format!("{}={}", crate::util::clean(k), short(v, 60)))
        .collect();
    let what = format!("{source} event {event}: {}", shown.join(" "));
    store.log(Some(run), &what)?;
    match &error {
        None => say(&format!("run {run} ({}): queued for {what}", rule.name)),
        Some(e) => {
            store.log(Some(run), &format!("failed: {e}"))?;
            say(&format!("run {run} ({}): not started: {e}", rule.name));
        }
    }
    Ok(Some((event, run)))
}

fn check_rule(rule: &Rule, exe: &Path) -> Result<()> {
    std::fs::create_dir_all(&rule.working_dir).with_context(|| {
        format!(
            "rule {}: creating {}",
            rule.name,
            rule.working_dir.display()
        )
    })?;
    crate::plugin::install(&rule.working_dir, exe)
        .with_context(|| format!("rule {}", rule.name))?;
    crate::plugin::preflight(&rule.working_dir).with_context(|| format!("rule {}", rule.name))?;
    let info = crate::recipe::read(&rule.recipe).with_context(|| format!("rule {}", rule.name))?;
    if !rule.hold.is_empty() {
        let provider = rule.provider.clone().or(info.provider).or_else(|| {
            std::env::var("GOOSE_PROVIDER")
                .ok()
                .filter(|p| !p.is_empty())
        });
        if let Some(p) = provider.filter(|p| crate::util::is_cli_agent_provider(p)) {
            bail!(
                "rule {}: provider {p} runs its own tools, so goose's hooks never see them and the rule's holds could not be enforced",
                rule.name
            );
        }
        let unheld: Vec<&str> = ["shell", "write", "edit"]
            .into_iter()
            .filter(|t| !matches_hold(&rule.hold, t))
            .collect();
        if !unheld.is_empty() {
            say(&format!(
                "warning: rule {} holds {:?} but not {}: a run can run commands or write files through those, so it could do what a held tool does",
                rule.name,
                rule.hold,
                unheld.join(", ")
            ));
        }
        let others = crate::plugin::other_plugins(&rule.working_dir);
        if !others.is_empty() {
            bail!(
                "rule {}: {} holds other goose plugins ({}), whose hooks could run commands no hold sees: remove them, or give the rule a working folder of its own",
                rule.name,
                rule.working_dir.join(".agents/plugins").display(),
                others.join(", ")
            );
        }
    }
    Ok(())
}

type Active = Vec<(i64, JoinHandle<()>)>;

/// Starts goose for approved runs (resumed) and then queued ones, oldest first, while fewer
/// than `max_concurrent_runs` go.
fn schedule(store: &Store, ctx: &Ctx, active: &mut Active) -> Result<()> {
    let max = ctx.cfg.max_concurrent_runs;
    let busy = |active: &Active, id: i64| active.iter().any(|(a, _)| *a == id);
    for r in store.runs_in(state::APPROVED)? {
        if active.len() >= max {
            return Ok(());
        }
        if busy(active, r.id) {
            continue;
        }
        let Some(call) = store
            .calls(r.id)?
            .into_iter()
            .rev()
            .find(|c| c.state == "approved")
        else {
            continue;
        };
        let text = runner::resume_text(&call.tool_name, &call.tool_input);
        active.push((r.id, runner::spawn(ctx.clone(), r, Mode::Resume(text))));
    }
    for r in store.runs_in(state::QUEUED)? {
        if active.len() >= max {
            return Ok(());
        }
        if busy(active, r.id) {
            continue;
        }
        active.push((r.id, runner::spawn(ctx.clone(), r, Mode::Start)));
    }
    Ok(())
}

pub fn run(cfg: Config) -> Result<()> {
    if cfg.rules.is_empty() {
        bail!("the config has no rules");
    }
    let _lock = lock(&cfg.state_dir)?;
    crate::util::create_private_dir(&cfg.runs_dir())?;
    let store = Store::open(&cfg.db_path())?;
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("finding the goose-triggers binary")?;
    for rule in &cfg.rules {
        check_rule(rule, &exe)?;
    }
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }

    for r in store.runs_in(state::RUNNING)? {
        if let Some(pid) = r.pid {
            wait_for_old_goose(pid, r.id);
        }
        if store.has_open_permission(r.id)? {
            // A resumed run that had not made its approved call yet: resume it again.
            store.transition(r.id, &[state::RUNNING], state::APPROVED, None)?;
            let what = "goose-triggers stopped before the approved call was made; the run resumes";
            store.log(Some(r.id), what)?;
            say(&format!("run {} ({}): {what}", r.id, r.rule));
            continue;
        }
        let msg = "goose-triggers stopped while this run was going";
        store.transition(r.id, &[state::RUNNING], state::FAILED, Some(msg))?;
        store.log(Some(r.id), &format!("failed: {msg}"))?;
        say(&format!("run {} ({}): failed: {msg}", r.id, r.rule));
    }
    let held = store.runs_in(state::HELD)?;
    if !held.is_empty() {
        let ids: Vec<String> = held.iter().map(|r| r.id.to_string()).collect();
        say(&format!(
            "{} held run(s) wait for a decision: {} (goose-triggers status)",
            held.len(),
            ids.join(", ")
        ));
    }

    let cfg = Arc::new(cfg);
    let ctx = Ctx {
        cfg: cfg.clone(),
        exe,
        db: cfg.db_path(),
    };
    let webhook = crate::webhook::serve(cfg.clone(), &STOP)?;
    let mut watches = Vec::new();
    for rule in &cfg.rules {
        match &rule.source {
            Source::Folder { path, pattern } => {
                watches.push(FolderWatch::new(&rule.name, path, pattern)?);
                say(&format!(
                    "rule {}: new files matching {pattern:?} in {}",
                    rule.name,
                    path.display()
                ));
            }
            Source::Webhook => {
                let listen = cfg
                    .webhook
                    .as_ref()
                    .map(|w| w.listen.as_str())
                    .unwrap_or("?");
                say(&format!(
                    "rule {}: POST http://{listen}/hooks/{}",
                    rule.name, rule.name
                ));
            }
        }
    }
    say(&format!("state in {}", cfg.state_dir.display()));

    let mut active: Active = Vec::new();
    let mut last_poll: Option<Instant> = None;
    while !STOP.load(Ordering::SeqCst) {
        if last_poll.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            last_poll = Some(Instant::now());
            for w in &mut watches {
                let rule = cfg
                    .rule(&w.rule)
                    .expect("watched rules come from the config");
                match w.poll(&store) {
                    Ok(files) => {
                        for f in files {
                            let fields = f.fields(&w.folder);
                            let queued = queue_event(
                                &store,
                                rule,
                                "folder",
                                &fields,
                                Some(&f.dedupe_key(&rule.name)),
                            )
                            .and_then(|_| w.mark_seen(&store, &f.name));
                            if let Err(e) = queued {
                                // Not marked seen: the next reads take the file again.
                                say(&format!("rule {}: {}: {e:#}", rule.name, f.name));
                            }
                        }
                    }
                    Err(e) => say(&format!("rule {}: {e:#}", rule.name)),
                }
            }
        }

        active.retain(|(_, h)| !h.is_finished());
        if let Err(e) = schedule(&store, &ctx, &mut active) {
            say(&format!("reading the store: {e:#}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    say("stopping");
    for (_, h) in active {
        let _ = h.join();
    }
    if let Some(h) = webhook {
        let _ = h.join();
    }
    say("stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_with_holds_also_hold_code_mode_and_the_extension_manager() {
        assert!(run_holds(&[]).is_empty());
        let h = run_holds(&["shell".to_string()]);
        assert!(matches_hold(&h, "shell"));
        assert!(matches_hold(&h, "execute_javascript"));
        assert!(matches_hold(&h, "code_execution__execute_javascript"));
        assert!(matches_hold(&h, "manage_extensions"));
        assert!(matches_hold(&h, "extensionmanager__manage_extensions"));
        assert!(matches_hold(&h, "delegate"));
        assert!(!matches_hold(&h, "tree"));
    }
}
