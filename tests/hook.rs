//! `goose-triggers hook` outside goose-triggers' runs: it takes all of what goose writes to it
//! (goose counts a payload it could not write as a failed hook, and blocks the call) and
//! allows the call.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn hook_outside_runs_reads_its_whole_input_and_allows() {
    let payload = format!(
        r#"{{"event":"PreToolUse","tool_name":"write","tool_input":{{"content":"{}"}}}}"#,
        "x".repeat(1 << 20)
    );
    for run in [None, Some("")] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_goose-triggers"));
        c.arg("hook")
            .env_remove("GOOSE_TRIGGERS_DB")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match run {
            Some(r) => c.env("GOOSE_TRIGGERS_RUN", r),
            None => c.env_remove("GOOSE_TRIGGERS_RUN"),
        };
        let mut child = c.spawn().expect("starting goose-triggers hook");
        let mut stdin = child.stdin.take().unwrap();
        let p = payload.clone();
        let writer = std::thread::spawn(move || stdin.write_all(p.as_bytes()).is_ok());
        let delivered = writer.join().unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            delivered,
            "GOOSE_TRIGGERS_RUN {run:?}: the payload was not taken whole"
        );
        assert_eq!(out.status.code(), Some(0), "GOOSE_TRIGGERS_RUN {run:?}");
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "GOOSE_TRIGGERS_RUN {run:?}: {out:?}"
        );
    }
}
