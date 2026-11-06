//! The end-to-end selftest, run by `cargo test` when a goose binary is available
//! (`goose` on PATH, or the path in `GOOSE_BIN`); skipped otherwise.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The goose binary to test with, if one runs.
fn goose() -> Option<String> {
    let goose = std::env::var("GOOSE_BIN").unwrap_or_else(|_| "goose".into());
    let probe_root =
        std::env::temp_dir().join(format!("goose-triggers-probe-{}", std::process::id()));
    let found = Command::new(&goose)
        .arg("--version")
        .env("GOOSE_PATH_ROOT", &probe_root)
        .output()
        .is_ok_and(|o| o.status.success());
    let _ = std::fs::remove_dir_all(&probe_root);
    if !found {
        eprintln!("skipped: no goose binary (put goose on PATH or set GOOSE_BIN)");
        return None;
    }
    Some(goose)
}

#[test]
fn selftest_end_to_end() {
    let Some(goose) = goose() else { return };
    let out = Command::new(env!("CARGO_BIN_EXE_goose-triggers"))
        .args(["selftest", "--goose", &goose])
        .output()
        .expect("running goose-triggers selftest");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "selftest failed:\n{text}");
    assert!(text.contains("selftest passed"), "{text}");
}

#[test]
fn an_interrupted_selftest_stops_its_daemon_and_removes_its_folder() {
    let Some(goose) = goose() else { return };
    let tmp = std::env::temp_dir().join(format!(
        "goose-triggers-interrupt-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_goose-triggers"))
        .args(["selftest", "--goose", &goose])
        .env("TMPDIR", &tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starting goose-triggers selftest");
    // Interrupt it once its `goose-triggers run` has started.
    let start = Instant::now();
    while !std::fs::read_dir(&tmp)
        .unwrap()
        .flatten()
        .any(|e| e.path().join("daemon.log").exists())
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the selftest's daemon did not start"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: SIGINT to the selftest this test started, as Ctrl-C sends it.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(out.status.code(), Some(130), "{text}");
    assert!(text.contains("interrupted"), "{text}");
    assert!(left.is_empty(), "left behind: {left:?}");
}
