//! A selftest or hostile suite that cannot start (here its goose cannot be run) removes its folder,
//! or with `--keep` keeps it and prints its path after the reason.

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
            assert!(
                said.contains("the path given with --goose must be a working goose binary"),
                "{said}"
            );
            assert!(!said.contains("pass --goose <path>"), "{said}");
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

#[test]
fn goose_version_names_an_explicit_binary_that_exits_unsuccessfully() {
    use std::os::unix::fs::PermissionsExt;
    let fake = std::env::temp_dir().join(format!("gt-failed-goose-{}", std::process::id()));
    std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    for cmd in ["selftest", "hostile-suite"] {
        if cmd == "hostile-suite"
            && !Command::new("curl")
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        {
            continue;
        }
        let out = Command::new(env!("CARGO_BIN_EXE_goose-triggers"))
            .arg(cmd)
            .arg("--goose")
            .arg(&fake)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{said}");
        assert!(said.contains("--version` failed"), "{said}");
        assert!(
            said.contains("the path given with --goose must be a working goose binary"),
            "{said}"
        );
        assert!(!said.contains("pass --goose <path>"), "{said}");
    }
    std::fs::remove_file(fake).unwrap();
}
