# goose-triggers 0.0.2: automated code review, round 3 of 4

Verdict: fit to release

Review of `git diff v0.0.0..HEAD` (edca189, f313c83, 9a159cd, 4e63d24, ebbd761), with a close read of ebbd761. I read the code at HEAD and checked it against goose v1.57.0's source (`git show v1.57.0:...`), the fixtures in tests/goose-output/, the build logs and the draft release notes. I built and ran nothing.

Nothing I found weakens a hold. Nothing lets an event's sender or the model get a held call through, change the recipe, forge a run's state, or choose a run's failure reason. The two findings are test gaps, so neither blocks the release.

## Findings

1. **Minor (test gap).** src/commands.rs:229-236; src/selftest.rs:538-542.
   - **What is wrong.** Item 5 has two halves, and no test covers the second: `log`'s "goose's stderr, last lines" section.
     - The unit tests check `tail` on its own (offset, size cut, bad bytes).
     - The selftest checks only the first half: the "goose printed:" line of the failed run (selftest.rs:469-474).
     - That run's goose writes nothing on stderr, because `render_error` prints on stdout and then calls `process::exit(1)`. So the stderr section is never run in any test.
   - **Scenario.** Any of these changes would pass every test:
     - `log` drops the section;
     - `log` passes the file's length as `from`, so the section is always empty;
     - `log` reads `<run>.jsonl` instead of `<run>.stderr`.
   - **Fix.** Extend the last selftest check (the folder run's log) to also assert two things:
     - the output contains `goose's stderr, last lines`;
     - it contains `Error: Headless run interrupted`.
     - Why that line is there: when goose is stopped at the hold, its first process returns the "Headless run interrupted" terminal error from `headless`, and Rust's `main` prints it on stderr as `Error: …` (session/mod.rs `headless_run_error`; the fixture interrupted.stderr).

2. **Minor (test gap).** src/recipe.rs:147-157; src/runner.rs:356-366; src/selftest.rs:523-531.
   - **What is wrong.** ebbd761 splits a failed render with the event's values into two cases. No test covers the split itself.
     - (a) goose ran and refused the recipe (`RenderFailed`): a fixed reason, and goose's text goes to the timeline.
     - (b) goose-triggers' own error (spawn failure, `RENDER_TIMEOUT`): reported by its own text.
     - Case (b) has no test at all.
     - For case (a), the ninth selftest check asserts only that the timeline label `goose's error rendering the recipe with the event's values` appears. It does not assert that goose's error text follows the label.
   - **Scenarios that would pass every test.**
     - A change makes `render` return spawn errors as `RenderFailed`. A webhook event `{"order": "A\u0000B"}` then gets the reason "a value breaks it (by its YAML or JSON syntax, say)". That is review 2's finding 1 again; the real cause is `Command::spawn`'s "nul byte found in provided data".
     - A change logs an empty or wrong `goose_said`. The user then has no means to find why goose refused the recipe.
   - The leak direction is covered. If goose's error went back into the reason, check 9's `!e.contains(chosen)` would fail.
   - **Fix.**
     - Post `{"order": "A\u0000B"}` to the webhook rule in the selftest. Assert that the run fails before goose runs, and that its reason contains "could not render the recipe with the event's values" and "nul byte", and not "breaks it". The webhook keeps NUL in a field (`fields_from_body`), and std's spawn error quotes no argument.
     - In check 9, assert that the log line starting with the label also contains `CHOSEN-TEXT-9`. goose 1.57.0 reports the refusal as `Error: Invalid recipe: …invalid type: string "CHOSEN-TEXT-9"…`, from `RecipeError::Invalid` and serde's `Unexpected::Str`.
     - The `log` output also lists the event's fields, so match that one line, not the whole output.

## The second review's findings (0.0.2-review-2.md)

1. **Closed.** The reason no longer claims the cause:
   - `ValuesBreakRecipe` says "so a value breaks it (by its YAML or JSON syntax, say)".
   - goose-triggers' own errors from the render with values are reported by their text, under "goose could not render the recipe with the event's values; the run was not started" (recipe.rs:153-155). I checked each one, and none carries event content:
     - std's NUL-byte error: "nul byte found in provided data";
     - E2BIG and ENOENT;
     - "goose took over 60 s to render the recipe";
     - the `try_wait` I/O errors.
   - goose's own render error (stdout and stderr, cut to 300 characters and cleaned by `short`) goes only to the run's timeline (runner.rs:356-366). It does not go to `runs.error`, to the "failed:" timeline line or to the daemon's output: `spawn` stores `{e:#}`, which for `ValuesBreakRecipe` is its fixed Display alone, since it has no source.
   - A parse failure of goose's rendered YAML is also kept as `RenderFailed`, so it too goes only to the timeline.
2. **Closed.** selftest.rs:469-474 runs `log <fail_run>` and asserts a `goose printed:` line holding goose's error ("error: Unknown provider: goose-triggers-selftest-missing"). The run's own `error:` line and the timeline's `failed:` line lack "goose printed: ", so the check is specific. The optional stderr half remains untested; it is finding 1 above.
3. **Closed as to building and testing; the publishing step is still to do.**
   - the release build's log ran at 04:45:18-25, just before the ebbd761 commit (04:45:40). Its selftest prints ebbd761's new check texts ("goose-triggers log 4 shows what goose printed: its error", "(goose's own error in the first one's log)"). The tree is clean at HEAD.
   - release.log shows:
     - `fmt --check` and clippy with no output;
     - 45 unit tests and the integration selftest passing (45 is the count of `#[test]` at HEAD);
     - the Rust 1.85 build of 0.0.2 (this supersedes the Rust 1.85 build's log, which is from 04:13);
     - the static-pie musl build;
     - that binary's selftest: 23 checks, all passing.
   - Still to do when releasing: dist/ has no v0.0.2 tarball or `.sha256` yet. Publish the binary from that build, and add its SHA-256 where the notes say "below".

## Checked and found sound

- **ebbd761, line by line.**
  - `e.downcast::<RenderFailed>()` matches exactly the errors `render` builds from goose's output. Every other error keeps its chain under the new context.
  - When the placeholder render also fails, its error wins; it quotes only tokens and the recipe's own keys. With no parameters, the single render's error holds no event content.
  - `runner::run_goose` logs `goose_said` only for `ValuesBreakRecipe`, then returns the same error.
  - The selftest's changed expectations ("breaks it", the log label) match the new text.
- **`--quiet` in goose 1.57.0.**
  - It gates only `print_recipe_info`, where the parameters go to stdout via anstream `println!` and the title and description to stderr (extract_from_cli.rs:21). It also gates `display_session_info` (builder.rs:895).
  - It is an `OutputOptions` flag of `goose run`, so it is valid with `--resume`.
  - It changes no stream-json event, hook or `GOOSE_MODE`.
- **What else goose writes in headless stream-json.**
  - The only plain stdout lines are `render_error` ("\n  error: …\n", followed by `process::exit(1)`) and the fixed interrupt prompt. `print_markdown` prints that prompt with `print!` on a non-terminal, so the error event lands on the same line.
  - `program_status` writes only to a terminal.
  - Logging goes to a file (`console: false`).
  - Stdio extensions (`subprocess.rs`), shell (`shell.rs:443`), hooks and retry checks all get piped stderr.
  - So the model cannot put plain text on goose's stdout. The sender's values cannot reach goose's output either: the run-time recipe load is the same `load_recipe` the render check already passed, with the same parameters.
- **`split_line`.**
  - It tries the whole line first and only then the first `{"type":` (goose serializes `StreamEvent` with `#[serde(tag = "type")]`, compact). It keeps objects only, and the split is at an ASCII byte.
  - It reads a strict superset of the events 0.0.0 read.
  - A misread or forged event can only do four things:
    - stop goose sooner (`answered`, a hold's call id);
    - fail the run (an unseen successful toolResponse);
    - set `saw_complete`, which matters only with exit 0 and a recorded session; in stream-json, exit 0 means goose emitted `complete` itself;
    - set the reason.
  - With `--quiet`, no sender or model text appears as plain stdout.
  - Note: `log` also applies `split_line` to logs kept from 0.0.0 or 0.0.1, whose lines include the event's values as goose printed them without `--quiet`. A value with mid-line JSON there can display as a goose event line in `log`. This is display only, the held-calls section stays authoritative, and whole-line forgeries already displayed so in 0.0.1.
- **`finish` order.** It checks in order: held, not running, unseen call, stopped, exit status, recorded session, `complete`.
  - An unseen tool still fails the run first.
  - `done` still needs exit 0, a recorded session and `complete`.
  - Moving the exit-status check before the session check changes only the message.
- **`failure_reason`, `error_lines` and `Said`.**
  - They take the last top-level error event, then goose's plain lines from the last one starting with "error", then the stderr tail. The result goes through `short`, which cleans control and bidi characters and cuts to 300 characters.
  - goose emits one terminal error event in headless stream-json, since `handle_agent_error` is skipped there. The event is the stream error, "Headless run interrupted", or the last message's error content.
  - The fixtures match goose's code paths:
    - `render_error`'s format;
    - `build_session`'s "Error {e}.\nPlease check…";
    - the prompt followed by the error event;
    - the network-error message and its event.
- **`tail`.**
  - The offset is taken from the append-mode file after the render check and before spawn.
  - One goose process at a time per run: the `busy` guard, the join on stop, and `wait_for_old_goose`.
  - At most 16 KiB is read. A cut first line is dropped. Decoding is lossy UTF-8. A missing or shrunk file gives nothing.
  - `log` uses offset 0 and 15 lines.
- **Item 2 across every place a reason is set.**
  - `queue_event` (`params_for` names only the recipe's keys; `recipe::read` quotes the recipe).
  - `prepare_working_dir`, `check_provider`, the CLI-agent provider of the rendered recipe (equal to the placeholder render's, by `same_recipe`).
  - The spawn I/O errors, the unseen call id (cut to 80), the stop and restart reasons.
  - None carries an event's values or field names.
- **The holds are unchanged.**
  - hook.rs, store.rs, `same_recipe`/`same_shape` and `TEXT_FIELDS` are untouched.
  - The only change in `watch` is `split_line`, which reads more events, not fewer.
- **Release notes, "Since 0.0.1".** Each statement matches the code and goose 1.57.0.
  - With a rule's `provider: no-such-provider` and no model, goose takes the `get_from_registry` path and prints "error: Unknown provider: no-such-provider", as the selftest's run shows. `status`'s 100-character column keeps the provider name.
  - The 404/405, `approve`, `max_turns` (`resolve_shell_timeout`, `DEFAULT_EXTENSION_TIMEOUT` = 300, 0 meaning no deadline) and `--quiet` README changes are all in `git diff v0.0.1..HEAD -- README.md`, and match webhook.rs:65-80 and commands.rs:247-277.
- **README.** Its selftest items 8 and 9 and its `log` and `status` descriptions match the code. Version 0.0.2 is in Cargo.toml, Cargo.lock and the install URLs.
