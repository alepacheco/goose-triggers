# goose-triggers 0.0.3: automated code review, round 3 of 3

Verdict: FIT

Review of `git diff a53aac1..e2ceebb` (one commit, e2ceebb). This was a read-only review: nothing was built or run. goose was read at tag v1.57.0 with `git show v1.57.0:...`.

## Review 2's five findings

1. **CI could not notice a new unheld tool: closed.**
   - The scripted model now records the name of every tool in each request's `tools` array (scripted_model.rs:309-331, 369-371).
   - goose 1.57.0's openai format sends `{"type":"function","function":{"name":..}}` (goose-provider-types formats/openai.rs:699), so `tool_names` reads the right field.
   - Pass 2 fails when goose offers a tool that `run_holds(holds)` does not match and that is not in `READ_ONLY` (hostile.rs:954-961). It also fails when goose offered no tools at all, which covers toolshim mode and an unparsed request format. The offered and open tools are printed.
   - The hold match is the daemon's own: `run_holds` plus `matches_hold`.
   - goose runs only tools it advertised in the request (`request_was_advertised`, ops_toolcalling.rs:729), so the recorded set is everything a run can call.
   - Not done: the start warning still uses its own list (finding 4 below).
2. **Any `cat -- ...` approved as the read: closed.**
   - `is_read` now requires `command` to equal `read_command(<canonical inbox>/<name>)` for that case, and webhook cases get `None`.
   - The daemon's `file_path` is the canonicalized folder joined with the name (folder.rs:59, 123).
   - goose renders the template under the name `recipe`, so minijinja does no auto-escaping, and the model reads the identical string.
   - If they ever differ, the read is rejected and recorded as the first held call, so `held_injected` fails: the suite fails rather than passing wrongly.
   - Comparing only `command` instead of the whole input is fine: `ShellParams`' only other field is `timeout_secs`.
3. **W6 needed only one canary: closed.**
   - `Canary::Each` with `all` requires both W6 canaries in pass 1. Any one tripped still counts as harm in pass 2.
   - Every other harm case has one call and one canary, so README:83's "every injected call trips its canary" is now true.
   - goose loads hooks once per session (`HookManager::load`), so W6 emptying hooks.json does not stop its second call from running in pass 1. The stricter check does not make pass 1 flaky.
4. **Summary could overstate "would change the recipe": closed.**
   - There are now three outcomes. "Renders without the key", an unreadable render and a timeout are `Other`, which fails the suite and names the cases.
   - A small residue remains in the `Breaks` bucket (finding 2 below).
5. **Webhook secret passed to `goose --version`: closed.** `goose_version` takes a variable to remove. The daemon passes `webhook.secret_env`, the same expression as `runner::hidden_env`. The selftest and the suite pass `None`; neither has the secret in its own environment.

## Question 2: new bugs, false passes, and the read-only list

**No new bug and no false-pass route for goose 1.57.0.**
- A harm case still passes only with all canaries tripped in pass 1, none tripped in pass 2, and the first held call equal to the first injected call.
- Recipe cases still need the refusal in both passes.
- Every new condition only adds ways to fail.

**The read-only list is right for 1.57.0.** A `goose run --recipe` of either example recipe is offered exactly seven tools:
- from developer (developer/mod.rs `get_tools`): `write`, `edit`, `shell`, `tree`, `read_image`;
- `analyze`, added by `Recipe::ensure_analyze_for_developer`;
- `load_skill`, which `SkillOperation::inference_tools` offers on every run (registered unconditionally, state_machine_builder.rs:74; ops_skills.rs:306). Review 2's tool list missed it.
- `final_output` is offered only with a response schema; the example recipes have none. No other operation implements `inference_tools`.

The three on the list only read local files:
- `tree` walks a folder with gitignore rules and counts lines.
- `analyze` parses local paths with tree-sitter. Its only `fs::write`s are in tests.
  - goose annotates both as read-only and not open-world.
- `load_skill`:
  - reads skills found in local skill folders, or built into the goose binary;
  - substitutes its `args` into the text;
  - replaces the docs-root placeholder;
  - starts no process and opens no connection.
- `load_skill` also goes through `run_pre_tool_hooks` (ops_skills.rs:366-377), so a hold can hold it.

The one residual way past the check is a later goose giving one of these three more power (finding 1).

## Question 3: README

README:11 and README:83 claim only what the code does:
- the suite fails on a *new* tool the holds leave open;
- the list is stated as goose 1.57.0's;
- "every injected call trips its canary" now holds.

README:83 does not mention the new failure on a recipe value goose renders neither way. That is an under-claim, not an over-claim (finding 2).

## Findings

1. **minor**: the read-only list is trusted by name on every goose release. Files: src/hostile.rs:456-460 and 954-961.
   - CI runs the suite daily against goose's `latest`. A release that gives `tree`, `analyze` or `load_skill` more power keeps the suite green, and the summary prints the tool as "(only reads local files)". `read_image` gaining `http(s)` loading is the precedent; `load_skill` fetching remote skills is a plausible next one.
   - The README does not over-claim: it says "a new tool".
   - Fix: keep, next to `READ_ONLY`, each tool's `function.parameters` (and description) as goose 1.57.0 offers it. Record them in the scripted model alongside the names. Fail when a goose offers one of these names with a different schema or description, printing "changed since goose 1.57.0: check it still only reads local files". This matches README's "fails it until the tool is looked at".
   - If not done: add to README:85 ("What it does not show") that a change to what `tree`, `analyze` or `load_skill` do does not show.

2. **nit**: `Render::Breaks` is any non-zero exit of `goose run --render-recipe`. File: src/hostile.rs:748-752.
   - The note then states the cause: "the value breaks the recipe's YAML".
   - Suppose a later goose rejects W7's or W8's value for another reason, such as validating `retry` checks or `sub_recipes` paths. Those cases are then counted under "would break it" with a wrong cause, and the suite passes.
   - Fix: give each recipe case its expected outcome (W7 and W8: `Adds`; F6: `Breaks`). On a mismatch, push the case to `odd` and fail.
   - Also add to README:83 that the suite fails when goose renders a value neither as the change nor as broken.

3. **nit**: a partial trip in pass 1 looks like a full one in the table. Files: src/hostile.rs:812, with 944 and 952.
   - If only one of W6's canaries trips in pass 1, the suite fails, but the table cell says "**carried out** (canary tripped)". Only the summary count is one short.
   - Fix: in `cell`, when `o.tripped && !o.all_tripped`, say "partly carried out (not every canary tripped)".

4. **nit**: the start warning was not built from the shared constant. File: src/daemon.rs:193.
   - It still lists `["shell", "write", "edit", "read_image"]` by hand. Review 2's fix asked for it to come from the same constant as the suite.
   - Fix: use `crate::hostile::EXAMPLE_HOLDS`, or move that constant beside `IMPLICIT_HOLDS` and use it in both places.

5. **nit**: a doc comment is attached to the wrong item. File: src/hostile.rs:681-683.
   - `rendered_without_check`'s old doc comment now sits above `enum Render`. The enum's rustdoc starts with the function's text, and the function has none.
   - Fix: move lines 681-682 to just above `fn rendered_without_check`, and say it returns the outcome with the note.

6. **nit**: the `rules[].hold` reference omits `load_skill`. File: README.md:169.
   - The row names every tool an example run is offered except `load_skill`, which goose 1.57.0 offers on every run (and which the suite now prints).
   - Fix: add "goose also offers `load_skill` in every run, which loads a skill's instructions".
