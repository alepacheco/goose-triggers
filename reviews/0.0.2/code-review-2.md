# goose-triggers 0.0.2: automated code review, round 2 of 4

Verdict: fit to release

Review of `git diff v0.0.0..HEAD` (edca189, f313c83, 9a159cd, 4e63d24). I read the code at HEAD and checked it against goose v1.57.0's source, the fixtures in tests/goose-output/, the debug build's log, the draft release notes and the earlier review. I did not build or run anything. None of the findings weakens a hold or lets an event's sender or the model choose a run's failure reason, so none blocks the release.

## Findings

1. **Minor.** src/recipe.rs:147-149.
   - **What is wrong.** If the render with the event's values fails and the render with placeholders succeeds, the run's reason always says "a value carries YAML or JSON syntax that breaks it". The code does not know that cause: `render` returns the same `Err` for every kind of failure. goose's own render error is now recorded nowhere, so the user has no means to find the real cause.
   - **Scenario.** POST `{"order": "A\u0000B"}` to a webhook rule whose recipe declares `order`.
     - `Command::spawn` refuses the argument ("running goose: nul byte found in provided data") before goose starts.
     - The placeholder render then succeeds, and the run fails with "a value carries YAML or JSON syntax that breaks it". The value carried neither.
     - In 0.0.1 the reason quoted the real error.
   - **Other causes that get the same wrong reason.** The render with values runs past `RENDER_TIMEOUT` or fails to spawn for a moment, and the placeholder render then succeeds. Or a filter or condition in the recipe's template refuses that value.
   - **Fix.**
     - Word the reason without claiming the cause, e.g. "goose cannot read the recipe with the event's values, though it reads it with placeholders: a value breaks it (YAML or JSON syntax, for example)".
     - Report goose-triggers' own errors (spawn, timeout) by their text, which holds no event content.
     - Optionally, record goose's render error as a separate timeline line. Only `log` would show it, and `log` already prints the event's fields. Keep it out of `runs.error`.

2. **Minor (test gap).** src/commands.rs:215-236; src/selftest.rs:528-532.
   - **What is wrong.** Item 5 (`log` shows what goose printed besides its events, and goose's last stderr lines) has no test. The only `log` check is the selftest's last one. It reads the finished folder run and asserts only "approved call made" and the done text.
   - **Scenario.** Any of these regressions would pass every test:
     - `log` stops printing the `goose printed:` lines;
     - `split_line` returns empty text for plain lines;
     - the stderr section reads the wrong range.
   - **Fix.** After the eighth check, run `log <fail_run>` and assert that it contains `goose printed: error: Unknown provider: goose-triggers-selftest-missing`. Optionally, assert that the folder run's log shows goose's stderr line `Error: Headless run interrupted`, written when goose was stopped at the hold.

3. **Minor (release step).** the release build's log, the Rust 1.85 build's log.
   - **What is wrong.** Both logs come from 9a159cd, before 4e63d24. release.log's selftest reports 21 checks and lacks the ninth.
   - **What has run at HEAD.** Only the debug build's unit tests and selftest: the debug build's log, 45 unit tests and 22 selftest checks, all passing.
   - **Consequence.** The static musl binary to be published, and the Rust 1.85 build, have not yet been built and selftested from 4e63d24.
   - **Fix.** Run the release build script at HEAD and publish that binary. Its selftest must report 22 checks. Then add its SHA-256 where the notes say "below".

## The earlier review (0.0.2-review-1.md)

1. **Closed.**
   - (a) With parameters, `check_values_stay_text` no longer quotes goose's error from the render with the event's values. It quotes goose's error only when the render with placeholder tokens fails, and that render holds only the tokens and the recipe's own keys. With no parameters it quotes the single render, which holds no event value.
   - (b) `params_for` now names only the missing recipe keys.
   - Tests: a unit test (src/recipe.rs:426-433) and the selftest's ninth check. That check is meaningful:
     - `json!` evaluates the key expression `chosen` (serde_json's `($($key)+).into()`), so the old "its fields: …" text would fail it;
     - goose's YAML type error for `settings: CHOSEN-TEXT-9` quotes the string, so it would fail the check too.
   - The release notes now claim only what holds: goose-triggers quotes none of the event's text. Finding 1 above concerns the wording of the new fixed reason, not a leak.
2. **Closed.** The eighth check now also asserts that `<run>.jsonl` and `<run>.stderr` lack the value (src/selftest.rs:469-479). Without `--quiet`, goose 1.57.0 prints `   order: B-77-error-in-the-value` on stdout before the provider error (`print_recipe_info` → `print_parameters_with_values`, anstream `println!`), so the check would now fail.
3. **Closed.** `a_tool_response_after_goose_s_text_is_read` (src/runner.rs:859-872) feeds the interrupt prompt followed by a toolResponse event through `split_line` and `tool_responses`.
4. **Closed.** README.md:14 shows `--quiet`.
5. **Closed.** README.md:205 now matches goose 1.57.0:
   - the shell timeout defaults to `GOOSE_DEFAULT_EXTENSION_TIMEOUT`, else `DEFAULT_EXTENSION_TIMEOUT` = 300;
   - `timeout_secs` overrides it, and 0 means no deadline (shell.rs:408-426, config/extensions.rs:10).

## Checked and found sound

- **What `--quiet` changes in goose 1.57.0.**
  - It gates only `print_recipe_info` (extract_from_cli.rs:21): the parameters go to stdout, the title and description to stderr. It also gates the session banner (builder.rs:895).
  - Stream-json events, hooks and `GOOSE_MODE` are unaffected, and `goose run --resume` accepts the flag.
  - The selftest passes with it at HEAD, holds included.
- **What goose writes besides its events.**
  - In headless stream-json, goose's only non-JSON stdout is `render_error` ("\n  error: …\n") and the fixed interrupt prompt. `print_markdown` prints that prompt with `print!` on a non-terminal, and the terminal error event follows on the same line.
  - Model text, tool calls and results, and MCP notifications are all JSON events.
  - goose logs only to a file (`console: false`).
  - Shell commands, stdio extensions and hook commands get piped stdout and stderr.
  - So neither an event's sender nor the model can put text into `Said.lines` or the stderr tail. An unheld shell could, but a hold does not claim to cover that.
- **No event values reach goose's error at run time.**
  - `goose run` loads the recipe with the same `load_recipe` as `--render-recipe`, so a value that breaks loading is refused at the render check first.
  - The terminal error is one of three things: the stream error, "Headless run interrupted", or the last message's error content (from `ProviderError`). goose's own code quotes no prompt in any of them.
- **The recipe gate is unchanged.** A run starts only if both renders succeed and `same_recipe` holds. `rendered_provider` can only name a provider equal to the placeholder render's.
- **Where the reason is stored.** `queue_event` stores the same reason in `runs.error`, the timeline's "failed:" line and the daemon's output. The event's own fields appear only in the timeline's event line and in `log`'s event section, as before.
- **`split_line`.**
  - It parses the whole line first. Otherwise it only adds the case of text followed by an object, and the split index is a character boundary.
  - Non-object JSON stays text.
  - A misread or forged event could only stop goose sooner or fail the run. A forged `complete` needs exit 0 and a recorded session, and exit 0 means goose emitted `complete`.
- **`finish` order.** It checks in order: held, not running, unseen call, stopped, exit status, recorded session, `complete`.
  - Moving the exit-status check ahead of the session check changes only the message.
  - An unseen tool still fails the run first.
  - `done` still needs exit 0, a recorded session and `complete`.
  - The `answered` and `on_line` paths now also read events that follow text.
- **The stderr tail.**
  - The offset is taken from the append-mode file before spawn.
  - No two goose processes of one run overlap: the `busy` guard, the join of active runs on stop, and `wait_for_old_goose` at start.
  - At most 16 KiB is read, and a cut first line is dropped.
  - Decoding is lossy UTF-8.
  - Tests cover the offset, the size cut and bad bytes.
- **`failure_reason`.**
  - It takes the error event, then goose's lines from its last "error" line on, then the stderr tail, and cuts the result to 300 characters through `short`.
  - `status`, `log` and the timeline print every text through `short` or `clean`, which escape control and bidi characters.
  - The fixtures match goose's code paths: `render_error`'s format, `build_session`'s "Error {e}. Please check…", the interrupt prompt plus the error event, and the network error event.
- **README.**
  - The 404 and 405 statuses match webhook.rs:66-81.
  - The `approve` wording matches commands.rs:247-277.
  - The note on a missing parameter matches daemon.rs:129-165.
  - The eighth and ninth checks are described correctly.
  - The install URLs and tarball layout match the 0.0.1 tarball (`goose-triggers-vX-x86_64-linux/goose-triggers`, `.sha256` in `sha256sum -c` format).
  - `-c` is a global flag.
- **Release notes, "Since 0.0.1".** Each bullet matches the code.
  - With `provider: no-such-provider` set, goose prints "error: Unknown provider: no-such-provider", or `build_session`'s "Error Unknown provider…" when the rule also sets a model.
  - `status`'s 100-character column shows that message.
- **0.0.1 changes.**
  - `goose --version` runs with `GOOSE_PATH_ROOT` in the selftest's own folder, which is removed if goose cannot be run.
  - The integration test's probe uses its own temporary root and removes it.
- **Version.** Cargo.toml and Cargo.lock say 0.0.2.
