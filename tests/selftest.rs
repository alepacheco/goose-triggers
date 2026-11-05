//! The end-to-end selftest, run by `cargo test` when a goose binary is available
//! (`goose` on PATH, or the path in `GOOSE_BIN`); skipped otherwise.

use std::process::Command;

#[test]
fn selftest_end_to_end() {
    let goose = std::env::var("GOOSE_BIN").unwrap_or_else(|_| "goose".into());
    let found = Command::new(&goose)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: no goose binary (put goose on PATH or set GOOSE_BIN)");
        return;
    }
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
