//! A selftest or hostile suite that cannot start (here its goose cannot be run) removes its folder,
//! or with `--keep` keeps it and names it after the reason.

use std::process::Command;

#[test]
fn a_selftest_or_suite_that_cannot_start_keeps_its_folder_only_with_keep() {
    let curl = Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    for cmd in ["selftest", "hostile-suite"] {
        if cmd == "hostile-suite" && !curl {
            // The suite refuses before it makes a folder without curl.
            eprintln!("hostile-suite skipped: no curl on PATH");
            continue;
        }
        for keep in [false, true] {
            let tmp = std::env::temp_dir()
                .join(format!("gt-keep-test-{}-{cmd}-{keep}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            // A goose that does not run: a file that is not executable, beside the folder
            // the command makes its own folder in.
            let fake = tmp.with_extension("goose");
            std::fs::write(&fake, "not a program\n").unwrap();
            let mut c = Command::new(env!("CARGO_BIN_EXE_goose-triggers"));
            c.arg(cmd)
                .arg("--goose")
                .arg(&fake)
                .env("TMPDIR", &tmp)
                .env_remove("GOOSE_TRIGGERS_RUN");
            if keep {
                c.arg("--keep");
            }
            let out = c.output().expect("running goose-triggers");
            let said = String::from_utf8_lossy(&out.stderr).into_owned()
                + &String::from_utf8_lossy(&out.stdout);
            let left: Vec<_> = std::fs::read_dir(&tmp)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            let _ = std::fs::remove_dir_all(&tmp);
            let _ = std::fs::remove_file(&fake);
            assert_eq!(out.status.code(), Some(1), "{cmd} keep={keep}: {said}");
            if keep {
                assert_eq!(left.len(), 1, "{cmd} --keep: {left:?}");
                let named = format!("its files, kept with --keep: {}", left[0].display());
                assert!(said.contains(&named), "{cmd} --keep: {said}");
            } else {
                assert!(left.is_empty(), "{cmd}: {left:?}");
                assert!(!said.contains("its files"), "{cmd}: {said}");
            }
        }
    }
}
