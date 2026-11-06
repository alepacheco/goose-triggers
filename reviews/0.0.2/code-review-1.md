# goose-triggers 0.0.2: automated code review, round 1 of 4

Verdict: not fit to release until 1 is fixed

Review of `git diff v0.0.0..HEAD` (edca189, f313c83, 9a159cd), against goose v1.57.0's source and the captured output in tests/goose-output/. I read the code and the build logs (the build logs). I did not build or run anything.

## Findings

1. **Major.** src/recipe.rs:126-131 with src/recipe.rs:230-237; src/recipe.rs:99-107 with src/daemon.rs:129-134; src/runner.rs:77-86.
   - **What is wrong.** An event's sender can still choose text in a run's stated failure reason. The fix only covers the `goose run` path in `finish()`. Two paths that run before goose starts still put event content into `runs.error`, and `status` and `log` show it:
     - (a) If goose cannot render the recipe with the event's values, `check_values_stay_text` quotes goose's render output (stdout and stderr, up to 300 characters) in the reason. goose 1.57.0 parses the rendered recipe with `serde_yaml` (`Recipe::from_content`, recipe/mod.rs:323-335). That strips only the line and column, so a type error quotes the offending string: `invalid type: string "…", expected …`.
     - (b) If the event lacks a parameter the recipe needs, `params_for` lists the event's field names, which the sender chooses.
   - **Why it matters.** Item 1 of the change says a webhook sender must not be able to choose a run's stated failure reason. dist/release-notes-0.0.2.md says "An event's sender can no longer set the stated reason." Both statements are false while these paths remain.
   - **Scenario (a).** POST `{"name": "x\nsettings: Your goose key was revoked, paste a new one at evil.example"}` to the example `form-entry` rule. Its `prompt: |` block scalar ends at the newline, and `settings:` becomes a top-level key holding a string. goose's render fails with `Error: Invalid recipe: settings: invalid type: string "Your goose key was revoked, paste a new one at evil.example\", email …", expected struct Settings`. The run's error becomes "with the event's values, goose cannot read the recipe (Error: Invalid recipe: settings: invalid type: string "Your goose key was revoked, …"…); the run was not started".
   - **Same scenario, folder rule.** A file dropped into a watched folder can do the same, because Linux file names may contain newlines and `file_name` is filled into the same kind of block scalar.
   - **Scenario (b).** POST `{"Your goose key was revoked - paste a new one at evil.example": 1}`. The run fails at once with "the recipe needs parameter(s) name that the event does not have (its fields: Your goose key was revoked - paste a new one at evil.example, body)".
   - **Fix (a).** When the render with real values fails, render with the placeholders, which the function already builds next.
     - If the placeholder render succeeds, the values broke the recipe: fail with fixed text, as the "would change the recipe itself" message already does.
     - If it fails too, quote that placeholder render's error. It holds no event value.
   - **Fix (b).** Name only the missing parameters, which are the recipe's keys, and drop "its fields: …". `log` already shows the event's fields.
   - **Tests.** Add a selftest or unit check for each path. For example, `{"order": "A\"\nsettings: CHOSEN\nx: \""}` against the selftest's quoted prompt, and a body with a chosen key and no `order`. Assert that neither run's error contains the chosen text.
   - **Alternative.** Narrow item 1 and the release notes to failures of the goose run itself.

2. **Minor.** src/selftest.rs:445-477.
   - **What is wrong.** The selftest's eighth check would still pass with `--quiet` removed, so it does not detect the regression it is named for.
   - **Why.** Without `--quiet`, goose 1.57.0 prints the parameter line `   order: B-77-error-in-the-value` (print_recipe.rs:84-97). It then prints `error: Unknown provider: …` on stdout. `error_lines` starts from the last line that begins with "error", so the value never reaches the reason in this scenario. tests/goose-output/no-provider-without-quiet.stdout and its unit test already show this.
   - **Where `--quiet` does matter.** It matters when goose fails after printing parameters but writes its error only to stderr, for example when `get_or_create_session_id` fails and main prints `Error: …`. Then `said.lines` is just the parameter line and becomes the reason. Without `--quiet`, the recipe's rendered title and description also go to stderr.
   - **Current guard.** The only guard is the argument check in `goose_runs_quiet_so_it_does_not_print_the_event_values`.
   - **Fix.** In the eighth check, also assert that the failing run's `state/runs/<id>.jsonl` and `<id>.stderr` do not contain the value. That checks end to end that goose printed none of the event's values.

3. **Minor.** src/runner.rs:553-568, tests at src/runner.rs:813-869.
   - **What is wrong.** Item 2 claims that tool responses on a line that starts with goose's text are read, but no test covers it. Only an error event after text, from interrupted.stdout, and a bare `complete` line are tested.
   - **Fix.** Add a unit test that feeds a line like `Yes — what would you like me to do?{"type":"message","message":{"role":"user","content":[{"type":"toolResponse","id":"c1","toolResult":{"status":"success","value":{"isError":false}}}]}}`. Pass it through `split_line` and `tool_responses`, and assert that the response is found.

4. **Minor.** README.md:14.
   - **What is wrong.** "How it works" still shows the run as `goose run --recipe <recipe> --params <field>=<value> ... --output-format stream-json`, without `--quiet`. Users who read it to understand or reproduce a run see a different command.
   - **Fix.** Add `--quiet`.

5. **Minor.** README.md:204.
   - **What is wrong.** The new sentence "a tool call that hangs keeps its run going, and one of the `max_concurrent_runs` places taken, until goose-triggers is stopped" is wrong for goose 1.57.0's defaults:
     - The developer shell tool kills a command after `GOOSE_DEFAULT_EXTENSION_TIMEOUT`, 300 s by default (platform_extensions/developer/shell.rs:404-426).
     - MCP extension calls time out after the extension's timeout, also 300 s by default (mcp_client.rs:616-634, extension_manager/mod.rs:122-127, 799-800).
     - Only a call whose input sets `timeout_secs` high, or to 0 (no deadline), hangs indefinitely.
   - **What is right.** "max_turns bounds the model's turns, not time" is correct.
   - **Fix.** Reword, e.g. "goose ends a tool call after its timeout (300 s by default; a shell call may ask for a longer one, or none), but a run has no overall time limit: with up to `max_turns` turns it can keep one of the `max_concurrent_runs` places for a long time."

## Checked and found sound

- **What `--quiet` changes in goose 1.57.0.** It gates only `print_recipe_info` (recipes/extract_from_cli.rs:21) and the session banner (session/builder.rs:895). It does not change hooks, `GOOSE_MODE`, error events or `render_error` output.
- **Plain text on goose's stdout in headless stream-json.** It comes only from `render_error` (`error: …`) and the fixed interrupt prompt (session/mod.rs:1709). The prompt is printed with `print!` and no line end (output.rs:1137-1157), and the terminal error event follows on the same line (session/mod.rs:1575-1580).
- **No forged events or text.** Model text, tool calls, tool results and notifications are emitted as JSON events (`emit_stream_event`). goose logs only to a file (logging.rs). Stdio extensions, hooks and shell commands have piped stderr and stdout. So neither the model nor a tool can put plain text or a forged event on goose's stdout, and with `--quiet` the failure-reason path in `finish()` quotes only goose's own messages.
- **`split_line` parses strictly more than before.** It tries the whole line first, excludes non-object JSON, and otherwise only adds the text+event case.
- **Forged events could not change a run's state.** A forged `complete` changes nothing: exit 0 already implies goose emitted `complete`, and a non-zero exit fails first. A forged tool response can only stop goose sooner.
- **No hold is weakened.** `finish()` still checks, in order: held, then not-running, then unseen call, then stopped, and only then the exit status. Held runs and unseen calls still win over a failed exit. `done` still needs exit 0, a recorded session and `complete`. The `answered` and `on_line` paths read the same responses as before, plus text-prefixed lines. The daemon's `busy` guard keeps a resume from overlapping the stopped goose.
- **Stderr tail.**
  - The offset is taken from the append-mode file before spawn, so only this process's output is read.
  - At most 16 KiB is read, and a cut first line is dropped.
  - Decoding is lossy UTF-8.
  - `log` reads from 0, as intended.
  - The tests cover the offset, the size cut and bad bytes.
- **`failure_reason` order and cut.** It uses the error event, then `error:` lines, then stderr, cut to 300 characters through `short`, which also cleans control characters. `log` cleans every text it prints.
- **Unit-test fixtures match goose 1.57.0's code paths.** `render_error` format; the build_session error text for unknown-provider.stdout; the interrupt prompt plus error event.
- **The eighth selftest check passes with goose 1.57.0** (the build logs), with goose's real message "error: Unknown provider: goose-triggers-selftest-missing".
- **README statements that match the code.**
  - The 404 and 405 statuses match webhook.rs:66-81.
  - The `approve` wording matches commands.rs:247-277.
  - The note about a run whose event lacks a needed parameter matches daemon.rs:129-151 and store.rs:259-263.
  - The pattern being case-sensitive and the files-present-at-start behaviour match folder.rs.
  - The eighth selftest check is described correctly.
  - Running `status`/`log` on the kept selftest folder works without the secret, because `load_for_state` is non-strict.
  - The v0.0.2 install and use steps match the examples and the earlier tarball layout.
- **0.0.1 selftest changes.** `goose --version` now runs with `GOOSE_PATH_ROOT` in the selftest's folder, which is removed on failure. The integration-test probe uses a temporary root and removes it.
