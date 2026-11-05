//! The SQLite store: events, runs, held calls, one-time permissions and each run's timeline.
//! Everything that must survive a restart lives here.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};

use crate::util::now;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    rule TEXT NOT NULL,
    source TEXT NOT NULL,
    fields TEXT NOT NULL,
    dedupe TEXT UNIQUE,
    received_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
    id INTEGER PRIMARY KEY,
    rule TEXT NOT NULL,
    event_id INTEGER NOT NULL REFERENCES events(id),
    state TEXT NOT NULL,
    recipe TEXT NOT NULL,
    params TEXT NOT NULL,
    holds TEXT NOT NULL,
    working_dir TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    max_turns INTEGER,
    session_id TEXT,
    pid INTEGER,
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_state ON runs(state);
CREATE TABLE IF NOT EXISTS calls (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    tool_name TEXT NOT NULL,
    tool_input TEXT NOT NULL,
    tool_call_id TEXT,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    decided_at INTEGER
);
CREATE TABLE IF NOT EXISTS permissions (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    call_id INTEGER NOT NULL REFERENCES calls(id),
    tool_name TEXT NOT NULL,
    tool_input TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    closed_at INTEGER
);
CREATE TABLE IF NOT EXISTS timeline (
    id INTEGER PRIMARY KEY,
    run_id INTEGER,
    at INTEGER NOT NULL,
    what TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decisions (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    tool_call_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    decision TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS decisions_call ON decisions(run_id, tool_call_id);
CREATE TABLE IF NOT EXISTS folder_seen (
    rule TEXT NOT NULL,
    folder TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (rule, folder, name)
);
CREATE TABLE IF NOT EXISTS folder_baseline (
    rule TEXT NOT NULL,
    folder TEXT NOT NULL,
    PRIMARY KEY (rule, folder)
);
"#;

/// A run's states. queued -> running -> done | failed | held; held -> approved -> running
/// (the same goose session, resumed) or held -> rejected.
pub mod state {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const HELD: &str = "held";
    pub const APPROVED: &str = "approved";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
    pub const REJECTED: &str = "rejected";
}

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub id: i64,
    pub source: String,
    pub fields: BTreeMap<String, String>,
    pub received_at: i64,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: i64,
    pub rule: String,
    pub event_id: i64,
    pub state: String,
    pub recipe: String,
    pub params: Vec<(String, String)>,
    pub holds: Vec<String>,
    pub working_dir: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<i64>,
    pub session_id: Option<String>,
    pub pid: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub id: i64,
    pub tool_name: String,
    /// The call's input as canonical JSON text.
    pub tool_input: String,
    pub tool_call_id: Option<String>,
    pub state: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub run_id: Option<i64>,
    pub at: i64,
    pub what: String,
}

pub struct NewRun<'a> {
    pub rule: &'a str,
    pub event_id: i64,
    pub recipe: &'a str,
    pub params: &'a [(String, String)],
    pub holds: &'a [String],
    pub working_dir: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub max_turns: Option<u32>,
    /// Set when the run cannot start (its event lacks a parameter the recipe requires).
    pub error: Option<&'a str>,
}

/// What the PreToolUse hook decided for one call.
#[derive(Debug, PartialEq, Eq)]
pub enum PreTool {
    /// The tool is not one the rule holds.
    Allowed,
    /// The call matches the one-time permission an approval gave; it is now used.
    AllowedByApproval,
    /// The call was held: stored as pending, and the run is now held.
    Held(i64),
    /// The run is held already: no further call goes through until the user decides.
    RunHeld,
    /// The run is not running (its state).
    NotRunning(String),
}

/// Whether a tool name matches one of a rule's hold patterns.
pub fn matches_hold(patterns: &[String], tool: &str) -> bool {
    patterns.iter().any(|p| {
        glob::Pattern::new(p)
            .map(|pat| pat.matches(tool))
            .unwrap_or(p == tool)
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            crate::util::create_private_dir(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    fn immediate(&self) -> Result<Transaction<'_>> {
        Ok(Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        )?)
    }

    // ---- events and runs ----

    /// Stores an event; `None` when one with the same `dedupe` key is stored already.
    pub fn add_event(
        &self,
        rule: &str,
        source: &str,
        fields: &BTreeMap<String, String>,
        dedupe: Option<&str>,
    ) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO events (rule, source, fields, dedupe, received_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![rule, source, serde_json::to_string(fields)?, dedupe, now()],
        )?;
        Ok((n == 1).then(|| self.conn.last_insert_rowid()))
    }

    pub fn event(&self, id: i64) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT id, source, fields, received_at FROM events WHERE id = ?1",
                [id],
                |r| {
                    let fields: String = r.get(2)?;
                    Ok(Event {
                        id: r.get(0)?,
                        source: r.get(1)?,
                        fields: serde_json::from_str(&fields).unwrap_or_default(),
                        received_at: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn add_run(&self, n: &NewRun) -> Result<i64> {
        let t = now();
        let state = if n.error.is_some() {
            state::FAILED
        } else {
            state::QUEUED
        };
        self.conn.execute(
            "INSERT INTO runs (rule, event_id, state, recipe, params, holds, working_dir, provider, model, max_turns, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                n.rule,
                n.event_id,
                state,
                n.recipe,
                serde_json::to_string(n.params)?,
                serde_json::to_string(n.holds)?,
                n.working_dir,
                n.provider,
                n.model,
                n.max_turns,
                n.error,
                t
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn run_from_row(r: &Row) -> rusqlite::Result<Run> {
        let params: String = r.get(5)?;
        let holds: String = r.get(6)?;
        Ok(Run {
            id: r.get(0)?,
            rule: r.get(1)?,
            event_id: r.get(2)?,
            state: r.get(3)?,
            recipe: r.get(4)?,
            params: serde_json::from_str(&params).unwrap_or_default(),
            holds: serde_json::from_str(&holds).unwrap_or_default(),
            working_dir: r.get(7)?,
            provider: r.get(8)?,
            model: r.get(9)?,
            max_turns: r.get(10)?,
            session_id: r.get(11)?,
            pid: r.get(12)?,
            error: r.get(13)?,
            created_at: r.get(14)?,
            updated_at: r.get(15)?,
        })
    }

    const RUN_COLUMNS: &'static str = "id, rule, event_id, state, recipe, params, holds, working_dir, provider, model, max_turns, session_id, pid, error, created_at, updated_at";

    pub fn run(&self, id: i64) -> Result<Option<Run>> {
        let sql = format!("SELECT {} FROM runs WHERE id = ?1", Self::RUN_COLUMNS);
        self.conn
            .query_row(&sql, [id], Self::run_from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Runs in a state, oldest first.
    pub fn runs_in(&self, st: &str) -> Result<Vec<Run>> {
        let sql = format!(
            "SELECT {} FROM runs WHERE state = ?1 ORDER BY id",
            Self::RUN_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([st], Self::run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every held run, then the `recent` latest others (all of them when `recent` is None);
    /// newest first.
    pub fn list_runs(&self, recent: Option<usize>) -> Result<Vec<Run>> {
        let sql = format!(
            "SELECT {} FROM runs WHERE state = 'held' OR id IN (SELECT id FROM runs WHERE state != 'held' ORDER BY id DESC LIMIT ?1) ORDER BY id DESC",
            Self::RUN_COLUMNS
        );
        let limit = recent.map(|n| n as i64).unwrap_or(-1);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit], Self::run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Moves a run from one of `from` to `to`; false when it was in none of them.
    pub fn transition(
        &self,
        id: i64,
        from: &[&str],
        to: &str,
        error: Option<&str>,
    ) -> Result<bool> {
        let tx = self.immediate()?;
        let cur: Option<String> = tx
            .query_row("SELECT state FROM runs WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        let Some(cur) = cur else { return Ok(false) };
        if !from.contains(&cur.as_str()) {
            return Ok(false);
        }
        tx.execute(
            "UPDATE runs SET state = ?2, error = COALESCE(?3, error), updated_at = ?4 WHERE id = ?1",
            params![id, to, error, now()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn set_pid(&self, id: i64, pid: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE runs SET pid = ?2 WHERE id = ?1", params![id, pid])?;
        Ok(())
    }

    /// Records the goose session a run started; the first one stays (a subagent's session
    /// started later is not the run's).
    pub fn set_session_if_unset(&self, id: i64, session_id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE runs SET session_id = ?2, updated_at = ?3 WHERE id = ?1 AND session_id IS NULL",
            params![id, session_id, now()],
        )?;
        Ok(n == 1)
    }

    // ---- held calls and approvals ----

    fn call_from_row(r: &Row) -> rusqlite::Result<Call> {
        Ok(Call {
            id: r.get(0)?,
            tool_name: r.get(2)?,
            tool_input: r.get(3)?,
            tool_call_id: r.get(4)?,
            state: r.get(5)?,
            created_at: r.get(6)?,
        })
    }

    /// The call a held run waits on.
    pub fn pending_call(&self, run_id: i64) -> Result<Option<Call>> {
        self.conn
            .query_row(
                "SELECT id, run_id, tool_name, tool_input, tool_call_id, state, created_at FROM calls
                 WHERE run_id = ?1 AND state = 'pending' ORDER BY id DESC LIMIT 1",
                [run_id],
                Self::call_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn calls(&self, run_id: i64) -> Result<Vec<Call>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, tool_name, tool_input, tool_call_id, state, created_at FROM calls WHERE run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([run_id], Self::call_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The PreToolUse decision for one call of a run, made in one transaction: a held tool is
    /// allowed only by an unused one-time permission for exactly this name and input;
    /// otherwise the call is stored as pending and the run becomes held.
    pub fn pre_tool_use(
        &self,
        run_id: i64,
        tool_name: &str,
        tool_input: &str,
        tool_call_id: Option<&str>,
    ) -> Result<PreTool> {
        let tx = self.immediate()?;
        let outcome = decide_call(&tx, run_id, tool_name, tool_input, tool_call_id)?;
        if let Some(cid) = tool_call_id {
            // Every call the hook saw: the runner checks that each tool goose ran was seen.
            let what = match &outcome {
                PreTool::Allowed => "allowed",
                PreTool::AllowedByApproval => "approved",
                PreTool::Held(_) => "held",
                PreTool::RunHeld | PreTool::NotRunning(_) => "denied",
            };
            tx.execute(
                "INSERT INTO decisions (run_id, tool_call_id, tool_name, decision, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, cid, tool_name, what, now()],
            )?;
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// Whether the hook saw a call (by goose's tool call id) in a run.
    pub fn hook_saw(&self, run_id: i64, tool_call_id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM decisions WHERE run_id = ?1 AND tool_call_id = ?2",
            params![run_id, tool_call_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Whether a run holds an approval not yet used.
    pub fn has_open_permission(&self, run_id: i64) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM permissions WHERE run_id = ?1 AND state = 'open'",
            [run_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Approves the call a held run waits on: a one-time permission for that tool name and
    /// input, and the run is queued to resume.
    pub fn approve(&self, run_id: i64) -> Result<Call> {
        self.decide(run_id, true)
    }

    /// Rejects the call a held run waits on: the run ends.
    pub fn reject(&self, run_id: i64) -> Result<Call> {
        self.decide(run_id, false)
    }

    fn decide(&self, run_id: i64, approve: bool) -> Result<Call> {
        let tx = self.immediate()?;
        let st: Option<String> = tx
            .query_row("SELECT state FROM runs WHERE id = ?1", [run_id], |r| {
                r.get(0)
            })
            .optional()?;
        match st.as_deref() {
            None => bail!("there is no run {run_id}"),
            Some(state::HELD) => {}
            Some(other) => bail!("run {run_id} is {other}, not held"),
        }
        let call = tx
            .query_row(
                "SELECT id, run_id, tool_name, tool_input, tool_call_id, state, created_at FROM calls
                 WHERE run_id = ?1 AND state = 'pending' ORDER BY id DESC LIMIT 1",
                [run_id],
                Self::call_from_row,
            )
            .optional()?
            .with_context(|| format!("run {run_id} is held but has no pending call"))?;
        let t = now();
        let (call_state, run_state) = if approve {
            ("approved", state::APPROVED)
        } else {
            ("rejected", state::REJECTED)
        };
        tx.execute(
            "UPDATE calls SET state = ?2, decided_at = ?3 WHERE id = ?1",
            params![call.id, call_state, t],
        )?;
        if approve {
            tx.execute(
                "INSERT INTO permissions (run_id, call_id, tool_name, tool_input, state, created_at) VALUES (?1, ?2, ?3, ?4, 'open', ?5)",
                params![run_id, call.id, call.tool_name, call.tool_input, t],
            )?;
        }
        tx.execute(
            "UPDATE runs SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![run_id, run_state, t],
        )?;
        log_in(
            &tx,
            Some(run_id),
            &format!(
                "{call_state} by the user: {} {}",
                call.tool_name, call.tool_input
            ),
        )?;
        tx.commit()?;
        Ok(call)
    }

    // ---- timeline ----

    pub fn log(&self, run_id: Option<i64>, what: &str) -> Result<()> {
        log_in(&self.conn, run_id, what)
    }

    pub fn timeline(&self, run_id: i64) -> Result<Vec<LogLine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT run_id, at, what FROM timeline WHERE run_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([run_id], |r| {
            Ok(LogLine {
                run_id: r.get(0)?,
                at: r.get(1)?,
                what: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The latest `limit` timeline lines of all runs, oldest first.
    pub fn recent_timeline(&self, limit: usize) -> Result<Vec<LogLine>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, at, what FROM (SELECT id, run_id, at, what FROM timeline ORDER BY id DESC LIMIT ?1) ORDER BY id",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(LogLine {
                run_id: r.get(0)?,
                at: r.get(1)?,
                what: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- folder sources ----

    pub fn baseline_done(&self, rule: &str, folder: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM folder_baseline WHERE rule = ?1 AND folder = ?2",
            params![rule, folder],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Records the files a folder holds when a rule first watches it: they are not new.
    pub fn set_baseline(&self, rule: &str, folder: &str, names: &[String]) -> Result<()> {
        let tx = self.immediate()?;
        for name in names {
            tx.execute(
                "INSERT OR IGNORE INTO folder_seen (rule, folder, name) VALUES (?1, ?2, ?3)",
                params![rule, folder, name],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO folder_baseline (rule, folder) VALUES (?1, ?2)",
            params![rule, folder],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn seen_names(&self, rule: &str, folder: &str) -> Result<HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM folder_seen WHERE rule = ?1 AND folder = ?2")?;
        let rows = stmt.query_map(params![rule, folder], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_seen(&self, rule: &str, folder: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO folder_seen (rule, folder, name) VALUES (?1, ?2, ?3)",
            params![rule, folder, name],
        )?;
        Ok(())
    }

    pub fn remove_seen(&self, rule: &str, folder: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM folder_seen WHERE rule = ?1 AND folder = ?2 AND name = ?3",
            params![rule, folder, name],
        )?;
        Ok(())
    }
}

/// The hold decision for one call, inside the caller's transaction.
fn decide_call(
    tx: &Connection,
    run_id: i64,
    tool_name: &str,
    tool_input: &str,
    tool_call_id: Option<&str>,
) -> Result<PreTool> {
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT state, holds FROM runs WHERE id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((st, holds)) = row else {
        bail!("run {run_id} is not in the store");
    };
    if st == state::HELD {
        return Ok(PreTool::RunHeld);
    }
    if st != state::RUNNING {
        return Ok(PreTool::NotRunning(st));
    }
    let holds: Vec<String> = serde_json::from_str(&holds).unwrap_or_default();
    if !matches_hold(&holds, tool_name) {
        return Ok(PreTool::Allowed);
    }
    let t = now();
    let permission: Option<i64> = tx
        .query_row(
            "SELECT id FROM permissions WHERE run_id = ?1 AND tool_name = ?2 AND tool_input = ?3 AND state = 'open' ORDER BY id LIMIT 1",
            params![run_id, tool_name, tool_input],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(pid) = permission {
        tx.execute(
            "UPDATE permissions SET state = 'used', closed_at = ?2 WHERE id = ?1",
            params![pid, t],
        )?;
        log_in(
            tx,
            Some(run_id),
            &format!("approved call made: {tool_name} {tool_input}"),
        )?;
        return Ok(PreTool::AllowedByApproval);
    }
    // An approval is for one call: once the run stops at another, earlier ones lapse.
    tx.execute(
        "UPDATE permissions SET state = 'lapsed', closed_at = ?2 WHERE run_id = ?1 AND state = 'open'",
        params![run_id, t],
    )?;
    tx.execute(
        "INSERT INTO calls (run_id, tool_name, tool_input, tool_call_id, state, created_at) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        params![run_id, tool_name, tool_input, tool_call_id, t],
    )?;
    let call_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE runs SET state = 'held', updated_at = ?2 WHERE id = ?1",
        params![run_id, t],
    )?;
    log_in(tx, Some(run_id), &format!("held: {tool_name} {tool_input}"))?;
    Ok(PreTool::Held(call_id))
}

fn log_in(conn: &Connection, run_id: Option<i64>, what: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO timeline (run_id, at, what) VALUES (?1, ?2, ?3)",
        params![run_id, now(), what],
    )?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn temp_store(tag: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "gt-store-{tag}-{}-{}",
            std::process::id(),
            crate::util::now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("state.db");
        (Store::open(&path).unwrap(), path)
    }

    pub fn running_run(s: &Store, holds: &[&str]) -> i64 {
        let ev = s
            .add_event("r", "webhook", &BTreeMap::new(), None)
            .unwrap()
            .unwrap();
        let holds: Vec<String> = holds.iter().map(|h| h.to_string()).collect();
        let id = s
            .add_run(&NewRun {
                rule: "r",
                event_id: ev,
                recipe: "/r.yaml",
                params: &[],
                holds: &holds,
                working_dir: "/w",
                provider: None,
                model: None,
                max_turns: None,
                error: None,
            })
            .unwrap();
        assert!(s
            .transition(id, &[state::QUEUED], state::RUNNING, None)
            .unwrap());
        id
    }

    #[test]
    fn hold_approve_resume_and_differing_call() {
        let (s, _) = temp_store("hold");
        let run = running_run(&s, &["shell", "mail__*"]);
        let input = r#"{"command":"echo hi"}"#;

        assert_eq!(
            s.pre_tool_use(run, "tree", "{}", None).unwrap(),
            PreTool::Allowed
        );
        let held = s.pre_tool_use(run, "shell", input, Some("call_1")).unwrap();
        assert!(s.hook_saw(run, "call_1").unwrap());
        assert!(!s.hook_saw(run, "call_x").unwrap());
        let PreTool::Held(call_id) = held else {
            panic!("{held:?}")
        };
        assert_eq!(s.run(run).unwrap().unwrap().state, state::HELD);
        // Nothing else goes through while the run is held, held tool or not.
        assert_eq!(
            s.pre_tool_use(run, "tree", "{}", None).unwrap(),
            PreTool::RunHeld
        );
        let pending = s.pending_call(run).unwrap().unwrap();
        assert_eq!((pending.id, pending.tool_name.as_str()), (call_id, "shell"));
        assert_eq!(pending.tool_call_id.as_deref(), Some("call_1"));

        // Not running: approval cannot be made twice, and only for a held run.
        s.approve(run).unwrap();
        assert!(s.has_open_permission(run).unwrap());
        assert!(s.approve(run).is_err());
        assert_eq!(s.run(run).unwrap().unwrap().state, state::APPROVED);
        assert!(matches!(
            s.pre_tool_use(run, "shell", input, None).unwrap(),
            PreTool::NotRunning(_)
        ));
        assert!(s
            .transition(run, &[state::APPROVED], state::RUNNING, None)
            .unwrap());

        // The approved call goes through once; the same call again is held again.
        assert_eq!(
            s.pre_tool_use(run, "shell", input, Some("call_2")).unwrap(),
            PreTool::AllowedByApproval
        );
        assert!(matches!(
            s.pre_tool_use(run, "shell", input, Some("call_3")).unwrap(),
            PreTool::Held(_)
        ));
        s.approve(run).unwrap();
        assert!(s
            .transition(run, &[state::APPROVED], state::RUNNING, None)
            .unwrap());

        // A call that differs from the approved one is held again, and the approval lapses.
        let other = r#"{"command":"echo other"}"#;
        assert!(matches!(
            s.pre_tool_use(run, "shell", other, None).unwrap(),
            PreTool::Held(_)
        ));
        s.reject(run).unwrap();
        assert_eq!(s.run(run).unwrap().unwrap().state, state::REJECTED);
        assert!(s.reject(run).is_err());
        let states: Vec<String> = s.calls(run).unwrap().into_iter().map(|c| c.state).collect();
        assert_eq!(states, ["approved", "approved", "rejected"]);
        let lapsed: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM permissions WHERE run_id = ?1 AND state = 'lapsed'",
                [run],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lapsed, 1);
    }

    #[test]
    fn holds_survive_reopening_the_store() {
        let (s, path) = temp_store("reopen");
        let run = running_run(&s, &["shell"]);
        s.pre_tool_use(run, "shell", "{}", None).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.runs_in(state::HELD).unwrap().len(), 1);
        assert_eq!(s.pending_call(run).unwrap().unwrap().tool_name, "shell");
    }

    #[test]
    fn hold_patterns_use_wildcards() {
        let p = vec!["shell".to_string(), "gmail__*".to_string()];
        assert!(matches_hold(&p, "shell"));
        assert!(matches_hold(&p, "gmail__send"));
        assert!(!matches_hold(&p, "shell2"));
        assert!(!matches_hold(&p, "write"));
        assert!(!matches_hold(&[], "shell"));
    }

    #[test]
    fn events_are_deduplicated_by_key() {
        let (s, _) = temp_store("dedupe");
        let f = BTreeMap::new();
        assert!(s.add_event("r", "folder", &f, Some("k")).unwrap().is_some());
        assert!(s.add_event("r", "folder", &f, Some("k")).unwrap().is_none());
        assert!(s.add_event("r", "webhook", &f, None).unwrap().is_some());
        assert!(s.add_event("r", "webhook", &f, None).unwrap().is_some());
    }
}
