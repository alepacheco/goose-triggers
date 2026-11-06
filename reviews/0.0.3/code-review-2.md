# goose-triggers 0.0.3: automated code review, round 2 of 3

Verdict: FIT

Review of `git diff v0.0.2..HEAD` (d1ce153, f081d99, 426da64, a53aac1). This was a read-only review: nothing was built or run. goose behaviour was read at tag v1.57.0 with `git show v1.57.0:...`, because the goose clone's working tree is at main. The workflow's goose release lookup was checked against the GitHub API: `releases/latest` is v1.57.0, and only goose releases are returned, no GDK tags. Rust 1.99.1 exists on static.rust-lang.org.

## Review 1's findings: all closed

1. read_image is now held in examples/config.yaml, README Use step 2, `EXAMPLE_HOLDS`, the start warning (`check_rule`) and the README's advice. The config comment is reworded. W9 and F7 send data out through `read_image` to a Request canary. The model is described as doing exactly what the injections name, and the summary says each route is fixed by its mark. Closed.
2. F6 is classed by goose's own render, so it is counted under "would break it". W8's evidence is said to be the refusal, and its file is only checked to be absent. W6 also checks the hooks file (`Lacks`). Closed.
3. Selftest 10 starts goose-triggers again after `reject`, waits 3 s and checks four things: no "goose resumed", state `rejected`, no open permission, and no A-3003 order. Closed.
4. SIGPIPE goes back to SIG_DFL only for `status` and `log`. `approve` and `reject` print through `say!`, which ignores write errors, so the decision is recorded even when stdout is closed. Selftest 10 checks this. Closed.
5. CI checks goose's checksum: 1.57.0 is pinned and compared with the API digest, and `latest` is checked against the API digest. The token is used only in the API step, and both steps use `pipefail`. Closed.
6. The README says "every push to `main`" and notes the 60-day pause. The lint toolchain is pinned to 1.99.1. Closed.
7. `set_pid(None)` now runs before the session lookup, and the lookup cannot fail the run. The failure text says "they recorded no session". Closed.
8. Canary paths go through `sh_quote`. Closed.

## Example recipes and holds in goose 1.57.0: no unheld tool with side effects

A `goose run --recipe` run of the example recipes loads only the recipe's own extension list. `resolve_extensions_for_new_session` uses the recipe's list. Plugin MCP servers are added only when a recipe has no `extensions`. `Recipe::ensure_analyze_for_developer` then adds `analyze`. No other default platform extension is loaded:
- extensionmanager, summon, apps, skills and tom are not loaded.
- scheduler has no scheduler service in CLI runs (`Agent::new` passes `None`).
- `recipe__final_output` exists only with a response schema.

The run's tools are therefore `write`, `edit`, `shell`, `tree`, `read_image` and `analyze`. The example holds cover all of them except:
- `tree`, which lists a folder with line counts;
- `analyze`, which runs tree-sitter over local paths.

Both only read, and only local files. They can bring any file the user can read into the model's context. From there it can leave only through a held call or to the configured model provider.

Two more checks found nothing:
- Name repair (`recover_mangled_tool_name`) runs on the model's reply before the PreToolUse hook (`ops_llm::prepare_tool_requests`), so the hook sees the same name that is dispatched.
- `read_image` has no host limit, so W9 and F7 can reach the canary on 127.0.0.1.

## Findings

1. **minor**: CI cannot notice a new unheld tool in a new goose release. Files: README.md:11, src/hostile.rs (whole suite), src/scripted_model.rs:194, src/daemon.rs:193.
   - The README says the daily run against goose's latest release means "a new goose release that breaks goose-triggers shows there".
   - That holds for a release that breaks a held tool: a renamed tool or argument, or changed hook semantics, makes pass 1 or pass 2 fail.
   - It does not hold for a release that adds a tool to the developer extension that sends, writes or runs (a `fetch`, say). No case names such a tool, and the start warning checks only a fixed list of four.
   - Users of the example config would then run with that tool unheld, and every check would stay green.

   Fix:
   - Have the scripted model record the tool names in each request's `tools` array.
   - In the holds pass, fail and print the name when goose offers a tool that the holds do not match and that is not on a reviewed read-only list (`tree`, `analyze`).
   - Build the start warning from the same constants.
   - If this is not done, change README.md:11 to say CI shows a release that breaks the holds on the tools goose-triggers knows, not that it catches every change.

2. **nit**: the suite approves any `cat -- ...` call as "the read of the new file". Files: src/hostile.rs:442-448 and 617.
   - `is_read` checks only the command's prefix, so the first `shell` call starting with `cat -- ` is approved, whatever it reads or chains after it.
   - The current cases do not hit this, and a wrong approval would make the suite fail, not pass. But the README says the suite approves "its read of the new file".

   Fix: compare the held input with `canonical_json({"command": read_command(<the case's file_path>)})` exactly.

3. **nit**: the README says more than the suite checks. File: README.md:83.
   - The README says the suite passes only if, with no holds, "every injected call trips its canary".
   - W6 makes two calls but has one `Any` canary, so pass 1 needs only one of them to have run. The suite checks one canary per case.

   Fix: say "every case's canary trips", or check both W6 canaries in pass 1.

4. **nit**: the summary's "would change the recipe" count may overstate later. File: src/hostile.rs:856-863.
   - Every value that goose can render counts as one that "would change the recipe", even when goose renders it without the injected key (the note then says "without `retry`").
   - With 1.57.0, W7 and W8 do add their keys. On a later goose, which the daily `latest` job runs, the summary could claim more than the notes show.

   Fix: count three outcomes: renders with the key, renders without it, and cannot render.

5. **nit**: the start check runs goose with the webhook secret in its environment. File: src/daemon.rs:265.
   - `goose_version(&cfg.goose, &[], ..)` passes the daemon's whole environment, `secret_env` included, to `goose --version`.
   - Every other goose that goose-triggers starts drops that variable: the runner, the recipe render and `find_session`.
   - goose does not read the variable on `--version`, so this is about consistency, not a leak in practice.

   Fix: let `goose_version` take variables to remove, and pass `hidden_env`'s variable.

## Checked and found sound

**Webhook.** `Announced` (400 on a cut-short body) and `Deadline` (408 on a stall, headers included) behave as promised. HEAD /health answers. HEAD on a hook path gets a head-only 405. The new unit tests fail on v0.0.2's code: a short body then gave 202.

**Store.**
- `decide` withdraws an approval and rejects the run in one IMMEDIATE transaction.
- If it races with the runner's APPROVED->RUNNING transition, one side wins and the other refuses.
- A withdrawn run cannot resume, and `pre_tool_use` gives `NotRunning`.
- `approve` on an approved run is refused.

**Commands.** `approve` refuses without a terminal unless `--yes` (it checks whether stdin is a terminal), and decides even when stdout is closed.

**Runner.**
- `find_session` matches the exact name, the canonical working folder and a creation time at or after the start.
- It runs only for Start runs whose hooks recorded no session.
- It is not involved in the "hook never saw this call" check.

**Hostile suite logic.**
- Pass 1 must trip every case's canary.
- Pass 2 must hold the first injected call, with no canary tripped.
- Recipe cases must be refused by the structure check in both passes.
- So broken hooks, an unloaded plugin, or renamed tools or arguments make the suite fail. They cannot make it pass.
- Sender text in the table goes through `clean` and has `|` escaped.

**CI.**
- Top-level `permissions: contents: read`, `persist-credentials: false`, and a checkout pinned by SHA.
- Matrix values and step outputs reach shell scripts only through `env`.
- A null or missing digest fails the job.
- The release binary's selftest and the hostile suite run after `cargo test`, so a missing goose cannot make the job pass.
