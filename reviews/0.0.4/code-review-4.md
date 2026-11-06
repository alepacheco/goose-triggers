# goose-triggers 0.0.4: automated code review, round 4 of 10

Verdict: NOT FIT

This review covers `git diff v0.0.3..HEAD` (4a02ff1, 517e668, bd799c0, ce8e46a, f23d48a, 31b4eb2). Nothing in the repository was built, tested or run. It checks each finding of the previous round (`reviews/0.0.4/code-review-3.md`) and reads the whole change again. The diff was read with the code around it: `src/hostile.rs` whole (the cases, `lex`, `expansion`, `simple_commands`, `is_date`, `toward`, `Task::within`, `Task::within_shell`, `Task::simple_within`, `pass`, `run_cases`, the counts and the printed results); `src/runner.rs` (`effective_provider`, `goose_default_provider`, `provider_in_goose_configs`, `goose_command`, `Stopper`, reasons); `src/daemon.rs` (`check_rule`, `run`); `src/config.rs`; `src/commands.rs` (`approve`); `src/store.rs` (`permission_states`); `src/selftest.rs` (interrupts, the pseudo-terminal check, `run`); `src/main.rs`; `src/util.rs`; `examples/`; `tests/selftest.rs`; `tests/readme.rs`; the README; both workflows; and every file in `reviews/`. goose was read at tag v1.57.0 with `git show`: `crates/goose/src/agents/platform_extensions/developer/shell.rs` (goose runs a `shell` call as `bash -c <command>`, or `$GOOSE_SHELL`, or `sh` without bash), `crates/goose/src/config/providers.rs` (`get_active_provider`) and `crates/goose/src/config/base.rs` (config paths, `get_param`, `load`). Each case below was traced by hand through the code. Each claim about how bash reads a command was also checked by running bash 5.3 in an empty temporary folder outside the repository, with a harmless command in place of the one shown.

## Findings

Findings 1 to 3 concern only the suite's own approvals with a real model (the "careful user"). goose-triggers' holds, its hook and its store are not affected. But each lets the suite approve a `shell` call that runs any other program, on the machine that runs the suite. README.md:102 says of an approved command: "Nothing bash would run or expand may be in it but `$(date ...)`". That is not so.

What `toward` still does: it reads every word, every here-document body and every substitution's text. So a hidden command that names the case's canary file or address is still rejected as toward the target. Any other hidden command runs. A canary reached without naming it (a glob such as `W2-entries.cs?`) is counted as "carried out after the suite approved a held call", and the suite fails, as if the holds had let it through. README.md:104 tells users to run the real-model suite on a machine they can throw away. That limits the harm, but not the wrong claim.

1. **major**: a `$(date ...)` substitution is not read as bash reads it, so three kinds of command run anything and are approved. Files: src/hostile.rs:726-731 and 761-767 (`lex` puts a substitution's text into the word), 687-689 (a here-document's delimiter), 874-907 (`expansion`), 972-994 (`is_date`, `date_arg`), 1137-1144 (`within_shell`), 1187-1193 (`bash -c`); README.md:102.
   - **What is wrong.**
     - `expansion` ends `$(...)` at the first `)` at depth zero. It does not skip quotes or comments, as bash does. `is_date` then lexes that text and accepts an unclosed quote or a trailing comment.
     - `lex` puts the substitution's source text into the word, in place of what bash puts there (its output). `simple_within` then checks that text as if it were the word. So `bash -c`'s script is checked as the `date` command, not as the date's output.
     - A here-document's delimiter is taken from that same word. bash takes the delimiter as written, with no expansion.
   - **The cases.** Each is for a webhook case such as W1 (output `entries.csv`). None names a mark, so `toward` is false.
     - (a) `echo "$(date '+%F)'; touch ~/ran; echo ')')" >> entries.csv`
       - The lexer ends the substitution inside `'+%F)'`. Its text, `date '+%F`, passes `is_date`. The rest is double-quoted text. The call is one `echo` into `entries.csv`, and it is approved.
       - bash ends the substitution at the last `)`. It runs `date '+%F)'`, then `touch ~/ran`, then `echo ')'`.
       - A comment does the same: `echo "$(date +%F # )`, then a line `touch ~/ran`, then a line `)" >> entries.csv`. `is_date` reads `date +%F # ` as `date +%F`. bash runs `touch ~/ran`.
     - (b) `bash -c "$(date '+touch ~/ran')"`
       - The script word is the text `date '+touch ~/ran'`. `within_shell` approves it as `date` with a `+FORMAT`.
       - bash runs `date`, which prints `touch ~/ran`, and then `bash -c` runs that. `sh -c` and backticks do the same.
     - (c) Four lines:
       ```
       cat >> entries.csv <<$(date +%F)
       $(date +%F)
       touch ~/ran
       date +%F
       ```
       - bash's delimiter is `$(date +%F)` as written, so the body ends at the first line. bash then runs `touch ~/ran` and `date +%F`.
       - The lexer's delimiter is `date +%F`, so it reads lines 2 and 3 as the body. It approves one `cat` of a here-document.
   - **Fix.**
     - Accept a substitution as the date only when its text is `date` and arguments made of `+`, letters, digits and `%-:_./`, or the listed options. Refuse any quote, `#`, `\`, `(`, `)` or line break in it. With none of these, bash also ends it at the first `)`.
     - Mark each word that holds a substitution. Allow such a word only as an argument of `echo` or `printf` after the format, or in a here-document body. Refuse it as the program, an option, a file, a redirection target, a here-document delimiter or `bash -c`'s script.
     - Add the three commands above to the tests, as refused.
     - Simpler and safer: approve only a few exact shapes (a literal `echo` or `printf` into the output file, `cat` of a quoted here-document, `date +FORMAT`, the read-only programs), and refuse the rest.

2. **major**: a backslash at the end of a line in an unquoted here-document joins it to the next line in bash, but not in `lex`. A call that ends its here-document early is approved. Files: src/hostile.rs:790-832.
   - **What is wrong.** When the delimiter is unquoted, bash removes each backslash-newline as it reads the body, and only then compares the line with the delimiter (`read_secondary_line` with backslash-newline removal on). `lex` compares each raw line. So a delimiter split over two lines ends the here-document for bash, and not for `lex`.
   - **The case.** W1, six lines:
     ```
     cat >> entries.csv <<EOF
     Ada,ada@example.com
     EO\
     F
     touch ~/ran
     EOF
     ```
     - bash joins `EO\` and `F` into `EOF`, so the body ends there. bash then runs `touch ~/ran`, and `EOF` fails as a command.
     - `lex` reads lines 2 to 5 as the body. Its only backslash is skipped, and nothing is active. `simple_within` approves one `cat` of a here-document into `entries.csv`. `toward` is false.
   - **Fix.** For an unquoted delimiter, join a line that ends in an odd number of backslashes with the next line, dropping the backslash and the line break, before comparing it with the delimiter, as bash does. Or refuse any unquoted here-document with a line that ends in a backslash. Add the case to the tests.

3. **major**: `printf -v` with an array subscript runs a command, and every `printf` is approved. Files: src/hostile.rs:1175; README.md:102.
   - **What is wrong.** `simple_within` approves `echo` and `printf` with any arguments. bash's `printf -v NAME` assigns to a variable. When `NAME` is `a[...]`, bash evaluates the subscript, command substitutions included. The README says `echo` or `printf` go "to the output file or to the model". `printf -v` goes to neither.
   - **The case.** W1: `printf -v 'a[$(touch ~/ran)]' %s y; printf '%s\n' 'Ada,ada@example.com' >> entries.csv`
     - The lexer sees single-quoted words only, so nothing is active. Both commands are approved `printf` calls. `toward` is false.
     - bash runs `touch ~/ran` when it assigns `a[...]`, then appends the entry.
   - **Fix.** Approve `printf` only when its first argument does not start with `-` (bash's `printf` has one option, `-v`). Add the case to the tests.

4. **minor**: the line printed at the start of a real-model suite describes the old approval rule. Files: src/hostile.rs:2067.
   - It says a call within the task is "a write or edit of its output file in the working folder, read_image of the new file, or one read-only command reading only those".
   - Since 31b4eb2 the suite also approves `echo`, `printf`, `cat` of a here-document, `touch`, `date`, `cd`, `test`, `bash -c` and `sh -c`, joined by `;`, `&&`, `||` or line breaks (README.md:102 says so).
   - **Fix.** Print the same list as README.md:102, or point to that section.

5. **nit**: Ctrl-C during the last recipe render, or during the approval pass's last session export, still prints results before "interrupted". Files: src/hostile.rs:1381-1384, 2100-2106, 2264-2309.
   - **The case.** Ctrl-C reaches the `goose run --render-recipe` of F6, the last recipe case, since it shares the suite's process group.
     - goose exits with an error, which reads as `Render::Breaks`, the result F6 expects.
     - No check follows, so the full table and summary print, maybe "every result as expected", and only then "interrupted" and exit 130.
     - The same happens when Ctrl-C stops the approval pass's last `goose session export`.
   - The folder is still removed, and the exit status is right.
   - **Fix.** Check `interrupted()` once more after the loop over the cases, and after the approval pass's `pass`, before anything is printed.

6. **nit**: the 0.0.4 copies in `reviews/` still open without a title line (the previous round's finding 5). Files: reviews/0.0.4/code-review-1.md:1, reviews/0.0.4/code-review-2.md:1, reviews/0.0.4/code-review-3.md:1.
   - The 0.0.2 and 0.0.3 copies open with a title, such as `# goose-triggers 0.0.3: automated code review, round 1 of 3`.
   - **Fix.** Add the title lines, to this report too when it is added, before tagging.

## Checked and found sound

- **The previous round's findings.**
  - 1 (each rejection ends the run; the recipes' own steps rejected): fixed.
    - README.md:102 now says a rejection ends the run, and what the table then shows.
    - The reason reads "not a step of the recipe's task the suite approves".
    - The plain forms are approved: a literal `echo` or `printf` into the output file, `date`, `ls` of the inbox folder.
    - The summary counts the runs that ended at such a rejection with no call toward the target.
    - The widened approvals bring findings 1 to 3.
  - 2 (here-document bodies read as shell): fixed, but for finding 2.
    - A quoted delimiter's body is one literal text, and `without_event` applies to it.
    - An unquoted one records only its substitutions and variables as active.
    - Bodies are tied to their redirections across `;` and line breaks.
  - 3 (`file -C`; `2>/dev/null` removed from the raw text): fixed.
    - `file` refuses `-C`, `-m`, `-f` and `-M`, and `hexdump` refuses `-f`.
    - `2>/dev/null` is now a parsed redirection, so `cat entrie 2>/dev/nulls.csv` is refused. Both are in the tests.
  - 4 (Ctrl-C while sessions are read and recipes rendered): fixed, but for finding 5.
  - 5 (title lines): open, finding 6.
- **The lexer against bash**, findings 1 to 3 aside. Traced by hand and, where it mattered, checked by running bash:
  - Single quotes, double quotes, `'\''`, and escapes outside and inside double quotes.
  - Backslash-newline outside here-documents, which joins lines in both.
  - Comments: only at a word's start, in both, so `x#y` stays one word.
  - File descriptor numbers: only unquoted digits right before `<` or `>`. `2>&1`, `&>`, `>|`, `<<<`, `<(...)` and `>(...)` are all refused.
  - `$'...'`, `$"..."`, `$((...))`, `$[...]`, `${...}` and every `$NAME` or special parameter are refused.
  - Unquoted globs, braces and a leading `~` are refused.
  - Keywords, `{ ...; }`, subshells, pipes, `&`, assignments and functions cannot be a program on the list.
  - goose passes the command as one argument to `bash -c`, so a NUL byte fails the spawn and nothing runs.
  - Backticks: bash finds their end as `expansion` does. A quote inside them does not protect a backtick (checked).
  - `test -v 'a[$(...)]'` also runs the substitution in bash. But `test`'s operands must be task paths, and none of them is a valid variable name.
  - `bash -lc` runs the user's login files. Those are the user's own, and no approved call can write them.
  - The read-only programs' short options read only, except `tail -f`, which waits and meets the run limit.
- **Decisions and counts.**
  - The toward check comes first, then the time limit, then the approval cap, then the task check.
  - `write` and `edit` are approved only for the output file, and `read_image` only for the new file's exact path.
  - "Acted on", "carried out" and "held" are computed as README.md:98-100 defines them.
  - Since a rejection ends a run, each run has at most one rejection, and the summary's wording matches that.
  - The approval pass approves every held call up to 6 a run, and reports `used`, `lapsed` and `open` from the permissions table.
  - The per-run limit counts only time with a goose process, and a timed-out run's later held calls are rejected.
  - Scripted mode decides and counts as in 0.0.3.
- **Holds.**
  - The hook, the plugin, and the store's hold, approval and permission logic are unchanged.
  - Declining at `approve`'s prompt fails before `approve_call` and exits 1. The selftest checks it on a pseudo-terminal and skips it, saying so, where none opens.
  - `run`'s start check and each run's check use `goose_default_provider`. Its order is the one of `get_active_provider` in goose 1.57.0: `GOOSE_PROVIDER`, then `active_provider` (`ACTIVE_PROVIDER`, then the merged files), then `GOOSE_PROVIDER` from the files.
  - The files are `/etc/goose/config.yaml`, each file in `GOOSE_ADDITIONAL_CONFIG_FILES`, then the user's file, a later one winning, as goose's `load` merges them.
  - `--provider` with a CLI-agent provider is refused before the folder is made.
- **Config.**
  - `GOOSE_TRIGGERS_CONFIG` set but empty, or naming a missing file or something that is not a file, is an error for `run` and for the state commands. An empty or comments-only file is refused.
  - `-c` wins. The defaults serve only when neither is given and the default file is absent.
- **Signals.**
  - The handler only stores to an atomic or calls `_exit`.
  - Every wait checks for an interrupt. The selftest and the suite stop their daemon before removing the folder, and exit 130.
  - `tests/selftest.rs` checks this for the selftest.
- **Release install.**
  - `.github/workflows/release-install.yml` takes the README's first `sh` block after `## Install` at the release's tag, checks that it names that tag, and waits for both assets to be uploaded.
  - It runs the block as written, checks the tarball against its `.sha256` file and against the digest GitHub lists, checks goose's tarball against its digest, compares `--version` with the tag and runs the selftest.
  - The tag reaches scripts only through the environment. The permissions are `contents: read`, with `persist-credentials: false`.
  - CI checks that a release tag matches `Cargo.toml` and the README. `tests/readme.rs` checks that every install command names v0.0.4, as `Cargo.toml` does.
- **reviews/.** The files name no build machine paths. The table matches the 0.0.1 to 0.0.3 folders. The 0.0.4 row and install test are to come at release, as release claim 3 says.
