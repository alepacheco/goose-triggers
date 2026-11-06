# goose-triggers 0.0.2: automated code review, round 4 of 4

Verdict: fit to release

Review of b29c6f5 (HEAD: src/selftest.rs, README.md). I read it with selftest.rs as a whole, recipe.rs (`check_values_stay_text`, `render`), runner.rs (`run_goose`, `spawn`, `finish`, `split_line`, `tail`), commands.rs (`log`, `describe`), webhook.rs (`fields_from_body`), daemon.rs (`queue_event`), the debug build's log and the release build's log. I checked goose claims against v1.57.0 (`git show v1.57.0:...`). I built and ran nothing.

The commit changes nothing outside the selftest. `selftest::run` is reached only from `Cmd::Selftest` (main.rs:96), and the README edit only describes the new check. The one finding is a test gap, which does not block the release.

## Findings

1. **Minor (test gap).** src/selftest.rs:583 (the last check, lines 578-585).
   - **What is wrong.** The new `log.contains("Headless run interrupted")` does not test the stderr section. The same text is in the goose section of the same `log` output.
     - When goose 1.57.0 is interrupted in stream-json mode, it emits `{"type":"error","error":"Headless run interrupted"}` on stdout (session/mod.rs:1569-1580, `headless_run_error`), on the line of its interrupt prompt. That line goes to `<run>.jsonl`, which is append-only, so run 1's first process's lines stay there after the resume.
     - `log` prints that event as `goose error: {"error":"Headless run interrupted","type":"error"}` (commands.rs:156 and 215-228).
     - A kept selftest folder shows the line: a kept selftest folder's state/runs/1.jsonl.
     - So only the header half (`goose's stderr, last lines`) depends on the stderr section. The check name "goose's stderr included" claims more than the check shows.
     - Review 3 asked for `Error: Headless run interrupted`. Only the stderr line holds that text (Rust prints goose's `main` `Err` as `Error: …`). The commit dropped the `Error: ` prefix, and with it what made the check specific.
   - **Scenario.** A change makes `log` tail `<run>.jsonl`, or any other non-empty file, under the stderr header, so the section no longer shows goose's stderr. Every test still passes:
     - the header is printed, because the tail is not empty;
     - "Headless run interrupted" is still found in the goose section.

     The other two scenarios from review 3 are caught. If the section is dropped, or `from` is the file's length, the tail is empty and `log` prints no header (commands.rs:231).
   - **Fix.** Assert the text inside the section. The section is the last thing `log` prints (commands.rs:229-236), and `log` writes only to stdout:
     ```rust
     && log
         .split_once("goose's stderr, last lines")
         .is_some_and(|(_, s)| s.contains("Error: Headless run interrupted"))
     ```
     (`log.contains("Error: Headless run interrupted")` alone would also be specific today.) This adds no dependency on goose beyond the one the check already has. With `--quiet`, run 1's stderr holds only that line: the resumed process ends with `complete` and no terminal error, and nothing else goose prints on stderr there is ungated.

## The third review's test gaps (0.0.2-review-3.md)

1. **`log`'s "goose's stderr, last lines" section: partly closed.**
   - Now tested: the section is printed with at least one line. This catches a dropped section and an empty tail (a wrong `from`).
   - Not tested: what the section shows. Its text assertion is met by the goose section (finding 1). A section built from the wrong file passes.
2. **The two cases of a failed render with the event's values: closed.**
   - **Case (b), goose-triggers' own error (selftest.rs:543-576).** I traced the NUL-byte event:
     - `fields_from_body` keeps U+0000 in `order`.
     - The event and the run's params are stored as JSON text, so the NUL survives the store (`\u0000`).
     - `params_for` passes it on.
     - In `render`, `Command::arg("order=A-10\0B")` makes std's `spawn` fail before fork with InvalidInput "nul byte found in provided data", which quotes no argument.
     - That error is wrapped as "running goose", is not a `RenderFailed`, and so gets the "goose could not render the recipe with the event's values; the run was not started" context.
     - The placeholder render runs with ASCII tokens and succeeds.
     - `spawn` stores `{e:#}`. The text is the one both logs print: "goose could not render the recipe with the event's values; the run was not started: running goose: nul byte found in provided data".

     The check would fail if spawn errors were turned into `RenderFailed`, or if the downcast branch always built `ValuesBreakRecipe`. The reason would then be the fixed "…so a value breaks it…" text, which fails all three text conditions. If the webhook or store dropped the NUL, the run would reach goose and be held, and the `FAILED` condition would fail. The text comes from goose-triggers' own std, fixed when it is built, so the check needs no goose wording.
   - **Case (a), goose's error in the log (selftest.rs:523-530).** The line with the label must also hold `CHOSEN-TEXT-9`.
     - `short` and `clean` turn goose's newlines into `\n`, so goose's whole error is on the label's line.
     - Two other lines hold `CHOSEN-TEXT-9`, and neither carries the label: the event's field line (`order = …`) and `queue_event`'s timeline line (`webhook event N: order=…`).
     - The `error:` and `failed:` lines carry `ValuesBreakRecipe`'s text. That text says "goose's error is in the run's log", not the label.
     - goose's text is `Error: Invalid recipe: …invalid type: string "CHOSEN-TEXT-9"…`: cli.rs:2246-2250 prints it with `eprintln!` then `exit(1)` (build_recipe/mod.rs:15; recipe/mod.rs:323-336 strips the location). It contains no path, so it stays far below the 300-character cut whatever the temporary folder's path is.

     The check fails if the line is not logged, is logged empty, or is logged with other text. The leak direction is still covered by `!e.contains(chosen)`.

## Checked and found sound

- **The logs are from HEAD's code.**
  - Timestamps, in order: selftest.rs 05:00:23, debug binary 05:00:24, check.log 05:00:41, release binary 05:01:12, release.log 05:01:19, commit 05:01:25. The tree is clean.
  - Both selftests print the new checks' texts and pass 24 checks with goose 1.57.0, from the debug build and from the static-pie musl release binary.
  - release.log also shows `fmt --check` and clippy clean, 45 unit tests passing, and the Rust 1.85 build passing.
- **Timing and order.**
  - The NUL run fails in the render check while still queued, before any goose process. `wait_for` accepts a FAILED, DONE or HELD state with no pid, within 90 s.
  - Each webhook run is waited on before the next is posted. The NUL run comes after every earlier run of `selftest-webhook` has ended, and nothing later looks up the rule's latest run (`run_of` is not used after it).
  - The stderr check runs after run 1 is done. The stderr line is written by run 1's first goose process before it exits.
  - One dependency: goose must leave through its own cancel path within `STOP_GRACE` (5 s) of SIGINT. If it had to be SIGTERM'd, there would be neither the stderr line nor the jsonl error event, and the check would fail rather than pass. goose cancels its model request at once, and neither log shows a problem.
- **goose's wording.**
  - Two checks depend on goose 1.57.0's behaviour:
    - check 9 needs goose to quote the value (serde's `Unexpected::Str` format, which is stable);
    - the last check needs "Headless run interrupted" and some stderr.
  - Another goose version could make these checks fail when goose-triggers is fine. It cannot make them pass when something is wrong.
  - The release notes say the release was tested with goose 1.57.0.
- **README item 9.** It is accurate. The NUL reason is goose-triggers' own (std's spawn error under goose-triggers' context), names no event text, and the run fails before goose starts. The README and the 0.0.2 notes give no check count, and the notes' "eighth and ninth checks" is still right.
- **Still to do when releasing (unchanged from review 3).** dist/ has no v0.0.2 tarball or `.sha256` yet. Publish the binary from the release build, and add its SHA-256 where the notes say "below".
