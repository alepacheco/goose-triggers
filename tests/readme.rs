//! The README installs the version this tree builds: its Install block, the source install and
//! the examples it fetches all name `v<Cargo.toml's version>`; the Install block stops at the
//! first line that fails.

#[test]
fn the_readme_installs_this_version() {
    let readme = include_str!("../README.md");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let install = readme
        .split("## Install")
        .nth(1)
        .and_then(|s| s.split("```sh\n").nth(1))
        .and_then(|s| s.split("\n```").next())
        .expect("the README's Install block");
    let asset = format!("releases/download/{tag}/goose-triggers-{tag}-x86_64-linux.tar.gz");
    assert!(install.contains(&asset), "{install}");
    // One list joined by `&&`: nothing is unpacked or installed after a failed download or
    // checksum, also when the lines are pasted one by one.
    let lines: Vec<&str> = install.lines().collect();
    for line in &lines[..lines.len() - 1] {
        assert!(line.ends_with(" &&"), "not joined by &&: {line}");
    }
    assert!(!lines[lines.len() - 1].ends_with("&&"));
    assert!(
        readme.contains(&format!("cargo install --locked --git https://github.com/alepacheco/goose-triggers --tag {tag}\n")),
        "the source install names another tag"
    );
    assert!(
        readme.contains(&format!("goose-triggers/{tag}/examples")),
        "the examples are fetched at another tag"
    );
    // No other release named in a command.
    for line in readme
        .lines()
        .filter(|l| l.contains("releases/download/") || l.contains("--tag v"))
    {
        assert!(line.contains(&tag), "{line}");
    }
}
