# goose-triggers 0.0.6: clean-install test by an automated tester

Verdict: PASS

This test installed goose-triggers 0.0.6 and used it from scratch. It followed only the README at the `v0.0.6` tag (the same README as in the release tarball) and the tool's `--help`. Each group of steps ran in a fresh, clean Debian bookworm-slim container with goose 1.57.0, as a normal user (uid 1000, with no entry in `/etc/passwd`) with `HOME=/tmp`. The container had no model provider and no API key. No step ran as root, and nothing was installed but goose-triggers. No model was used. Where a step needed goose to get answers (a held run, approve, reject, resume, a run's session opened again), the test pointed goose's `openai` provider at a small scripted stand-in for an OpenAI-compatible chat-completions endpoint on 127.0.0.1. This test wrote the stand-in in Perl. It answers with one fixed tool call while the last message is the user's, and with the text `done` after a tool result, streamed or not, as goose asks. It can also wait before it answers, or refuse every request with 401. It holds no model, so nothing here shows what a real model does. Steps that needed a terminal ran under `script`. The 0.0.5 install test's finding 1 (goose-triggers' hook blocked calls in goose sessions it did not start) is fixed: 228 such calls ran and none was blocked, while goose-triggers' own runs were still held at every marked call. The 0.0.5 test's findings 2 to 8 are fixed too. This test found three cosmetic points and no bug.

## Steps and results

1. **Install block, verbatim** (run with `bash -x` in an empty folder). PASS.
   - Both downloads worked. `sha256sum -c` printed `goose-triggers-v0.0.6-x86_64-linux.tar.gz: OK`. `goose-triggers --version` printed `goose-triggers 0.0.6` (exit 0).
   - The tarball (2,367,373 bytes) holds a static x86-64 binary (4,901,752 bytes), `LICENSE`, `README.md` (identical to the README at the tag) and `examples/` (identical to the three files Use step 2 downloads). Its SHA-256 matches the digest GitHub lists for the asset.
   - **Corrupted download.** A `curl` wrapper put first on `PATH` appended one byte to the tarball after the download. Then the block was pasted, unchanged, into an interactive bash, line by line. `sha256sum -c` printed `FAILED`, and the block stopped there (status 1). Nothing was extracted, `~/.local/bin` stayed empty, and `goose-triggers` was not on `PATH`.
   - `--help`, each command's `--help` and `--version` work (exit 0).
2. **`selftest`, `selftest --keep`, and the kept folder.** PASS.
   - `selftest` passed 33 checks in 17 s (exit 0). The terminal check ran (`approve` answered `n`). Nothing was left in `/tmp`, and nothing was made under `HOME`. It also passes with `GOOSE_TRIGGERS_CONFIG=/nonexistent`. `-c` with `selftest` or `hostile-suite` exits 2.
   - `--keep` ends with `its files: /tmp/goose-triggers-selftest-<n>-<m>`. `-c <folder>/config.yaml status --all` lists 8 runs: done, rejected and failed, each with its call or reason. `log 1` shows the event, the held call in full, the timeline and goose's messages. `approve 1 --yes` and `reject 1` answer `run 1 is done, not held` (exit 1), as the README says.
3. **`hostile-suite`, as the README shows it.** PASS.
   - It ended `every result as expected` in 13 s (exit 0) and left nothing in `/tmp`. With no holds, 13 of 13 canaries tripped. With `[shell, write, edit, read_image]`, 0 of 13 were carried out, and all 13 were held at their first injected call. The structure check refused 2 of 2 values that would change the recipe and 1 of 1 that would break it, in both passes. goose offered `analyze, edit, load_skill, read_image, shell, tree, write`.
   - `--keep` kept `results.md`, `holds-off/`, `holds-on/` and `goose-root/`.
   - `--holds shell,write,edit`: W9 and F7 were carried out through `read_image` (`MORE THAN READS`). The suite said `NOT every result as expected` (exit 1) and kept its folder. `--holds 'shell write edit read_image'` passes.
   - **With the stand-in as the suite's "real model"** (`--provider openai --model gpt-4o-mini --approvals 1 --run-minutes 2`, `OPENAI_HOST` in the environment): the provider check said `it answers (0 s)`, both passes and the approval pass ran, and the suite ended `every result as expected` (exit 0). The stand-in makes the same harmless call for every event, so the counts show nothing about injections.
4. **Goose sessions not started by goose-triggers, in a rule's working folder (0.0.5 finding 1).** PASS.
   - `goose-triggers run` was started once and stopped. The working folder `/tmp/forms` then held `.agents/plugins/goose-triggers/` (`plugin.json` says `0.0.6`). Its hooks now read their input before they exit outside goose-triggers' runs (`{ cat >/dev/null; exit 0; }`).
   - **New sessions** (`goose run --text ...` in `/tmp/forms`, through the stand-in). Each made one `shell` call or one `write` of 70,000 characters. The checks covered `goose-triggers run` stopped and running, on an idle container and with its two cores kept busy by four busy loops:

     | goose-triggers run | cores | `shell` calls that ran | 70,000-character `write`s that ran |
     |---|---|---|---|
     | stopped | idle | 20 of 20 | 10 of 10 |
     | stopped | busy | 20 of 20 | 10 + 30 of 40 |
     | running | idle | 10 of 10 | 6 of 6 |
     | running | busy | 10 + 100 of 110 | 6 of 6 |

     That is 222 calls. Each one ran (each `write` made a 70,000-byte file), and no goose output said `blocked`.
   - **Run sessions opened again.** First, runs held at a 70,000-character `write` were rejected. Then their sessions were opened on a terminal with `goose session --resume --session-id <id>`, as Use step 5 says, and a message was typed. The stand-in's next call ran each time. These cases were tried:
     - `run` stopped, idle: a 70,000-character `write` (70,000-byte file) and a `shell` call;
     - `run` stopped, cores busy: the same `write`;
     - `run` running, cores busy: the same `write`.
   - `goose run --resume --session-id <id> --text ...` also ran a `write` of that size in a rejected run's session. A `shell` call also ran in a still-held run's session, and that run stayed held.
   - **goose-triggers' own runs are still held at the marked calls.** Webhook runs were held at `shell` (idle and with busy cores) and at a 70,000-character `write`. One more was held at `shell` after the 130-call batch. None of these calls ran. `status` shows each call and `log` shows it in full.
   - The first try at opening a session on a terminal typed its message into goose's own first-start question (`Share anonymous usage data ...?`), so no call was made. Later tries saved goose's answer to that question first.
5. **Use, steps 1 to 5, verbatim, with no provider.** PASS.
   - Step 1 fails as expected: `No provider configured. Run 'goose configure' first.` (exit 1).
   - Step 2: the three downloads work. `webhook-secret` has mode 600 and holds 32 characters.
   - Step 3: `run` starts. It warns, for each rule, that neither the rule nor its recipe names a provider and goose has none.
   - Step 4: the `curl` got `{"event":1,"run":1}`. A PDF dropped into `~/Inbox` started run 2.
   - Step 5: `status` and `log 1` show both runs failed with goose's own reason. `approve 1` and `reject 1` answer `run 1 is failed, not held` (exit 1). `goose session list` names the sessions `goose-triggers <rule> run <n>`.
6. **Use, steps 1 to 5, verbatim, with the stand-in as goose's provider** (goose's `config.yaml` names `openai`, with `OPENAI_HOST` pointing at the stand-in). PASS.
   - Step 1 answers. The webhook run (1) and the PDF run (2) were each held at `shell`. `status` shows the calls, and `log 1` shows the event, the held call in full, the timeline and goose's messages.
   - `approve 1` with no terminal shows the call and refuses without `--yes` (exit 1).
   - `approve 2` on a terminal, answered `n`: `not approved; run 2 stays held` (exit 1).
   - `approve 1` on a terminal, answered `y`: approved. The running `goose-triggers run` resumed the same session, the call ran once, and run 1 is `done`. `approve 1` and `reject 1` then answer `run 1 is done, not held` (exit 1).
   - `reject 2` ended run 2 without running its call. Rejecting it again exits 1.
   - A run approved while `run` was stopped resumed at the next start and was done.
   - At the default `max_concurrent_runs` of 2, a third event waited as `queued`.
   - The approval of a `write` let through the same call made again. goose passed the input to the hook with its keys in the same order both times, although the stand-in sent them in another order the second time.
7. **0.0.5 findings 2 to 8 and the README's new statements.** PASS (details under the next heading).
   - **The upgrade paragraph.** 0.0.5's `run` wrote its plugin (`0.0.5`) into two working folders. 0.0.6 was installed over the binary while that `run` went on. A new event's run was still held, and the plugin stayed 0.0.5's. With one rule removed from the config, 0.0.6's `run` started. It wrote its plugin (`0.0.6`) into the remaining rule's folder and left the old one in the other folder, as the README says. In that other folder, a goose session's own 70,000-character `write` was blocked (`the hook did not receive the request payload`), which is what the README warns of. In the 0.0.6 folder the same `write` ran.
8. **Start refusals in the Config reference.** PASS. Each of these was refused with a clear message (exit 1):
   - an unknown key (top level, in a rule), and malformed values (`max_concurrent_runs: two`, `hold: shell`);
   - two rules with the same name; a name with a space;
   - both sources, or none;
   - a missing folder or recipe;
   - the pattern `[abc`, and patterns with `/`;
   - a webhook rule with no `webhook:` section; no secret; a 15-character secret; `secret_env` unset; both `secret` and `secret_env`;
   - `max_concurrent_runs: 0`; `rules: []`;
   - `goose: /bin/false`;
   - `provider: claude-code` on a rule with holds;
   - another plugin in the working folder; `disabledPlugins: ["goose-triggers"]` in the working folder's `.config/goose/settings.local.json`;
   - recipes that are not valid YAML, have no title, or declare a file parameter;
   - a listen address in use;
   - an empty or missing `-c` file;
   - a second `run` on the same state folder.

   A rule that holds only `shell` starts with the README's warning.
9. **goose's default provider, as goose reads it.** PASS. With a rule with holds and no provider, `run` refused CLI-agent providers set as goose's default in each of these ways:
   - `GOOSE_PROVIDER` in the environment;
   - `GOOSE_PROVIDER:` or `active_provider:` in goose's `config.yaml` (`gemini-cli`, `claude-code`, `gemini-acp`);
   - a file in `GOOSE_ADDITIONAL_CONFIG_FILES`.

   Where `config.yaml` holds both keys, goose-triggers took the same provider that goose itself used (`goose info -v`, and `goose run`). When goose's default was changed to `claude-code` while `run` ran, two runs failed with that reason. These were a new run, before goose started, and a held run when it was approved and resumed. The held call did not run. Adding `disabledPlugins` while `run` ran made the next run fail before goose started.

   One thing to know: goose 1.57.0 rewrites its own `config.yaml` during a run. It turns `GOOSE_PROVIDER` into `active_provider` and `providers:`, and it adds `plugins: {/tmp/forms/.agents/plugins/goose-triggers: {enabled: true}}`. That is goose's doing.
10. **Webhook answers.** PASS.
    - Without the right secret, every path under `/hooks/` gets 401 (`POST /hooks/form`, `GET /hooks/zzz`, a wrong secret, the secret without `Bearer`).
    - With the secret, these answers came back:
      - 202 `{"event":N,"run":N}`;
      - 404 for an unknown rule and for `/nope`;
      - 405 for GET and PUT;
      - 411 for a chunked body;
      - 413 at 65,537 bytes, while 65,536 bytes are accepted;
      - 400 for `Content-Length: abc`, and for a body that ends before its length;
      - 408 after 10.3 s for stalled headers, and after 10.2 s for a stalled body.
    - `GET` and `HEAD /health` get 200.
11. **Event fields and the folder source.** PASS.
    - Numbers, booleans, arrays and objects arrive as JSON text: `n = 42`, `b = true`, `arr = [1,"x"]`, `obj = {"k":"v"}`. `café` passes through. A body that is not JSON gives only `body`. A JSON `body` field is kept.
    - `new.pdf`, `name with spaces.pdf`, a file moved in with `mv`, and a PDF dropped while `run` was stopped (taken at the next start) started runs. A PDF there before the first watch, `.hidden.pdf`, `upper.PDF` and `sub/deep.pdf` did not.
    - SIGINT and SIGTERM stop `run` with exit 0.
12. **Ctrl-C.** PASS.
    - SIGINT was sent to the process group of `selftest`, `selftest --keep`, `hostile-suite` and `hostile-suite --keep`, at 3 s and at 8 s. Each exited 130 within 0.5 s and left no goose process.
    - Without `--keep`, each said `interrupted; its temporary folder is removed` and left none. With `--keep`, each said `interrupted; its files: ...` and kept it.
    - A real `^C` typed on a terminal stopped `selftest`, and its folder was removed.

## The 0.0.5 install test's findings

1. **Hook blocked calls in goose sessions goose-triggers did not start:** fixed. In 0.0.5, 1 of 20 small calls was blocked on an idle container, 15 of 20 with busy cores, and every `write` of 70,000 characters. Here, 222 new sessions and 6 calls in reopened run sessions all ran, idle and busy, with `run` stopped and running, on a terminal and not (step 4). goose-triggers' own runs were still held at every marked call.
2. **Install block went on after a failed checksum:** fixed. The lines are joined by `&&`, and the README says so. A corrupted download, pasted line by line, stopped at `sha256sum -c` (step 1).
3. **JSON `null` not documented:** fixed. The README says `null` arrives as empty text. `{"name": null, ...}` gave `name = ` (empty), and the run went on with an empty value. It did not fail as missing.
4. **A pattern with `/` accepted:** fixed. `sub/*.pdf`, `/x.pdf`, `*.pdf/` and `a/b` are refused at start: `pattern "sub/*.pdf" has a '/', but it is matched against the names of files directly in the folder, so it would never match: make the subfolder the rule's folder instead` (exit 1).
5. **With no home folder, only `run` refused:** fixed. With `HOME` unset or empty and no entry in `/etc/passwd`, these refuse with `no home folder: ... Set HOME` (exit 1): `status`, `status --all`, `log`, `log 1`, `approve 3 --yes` and `reject 3`, when they need the default config, the default `state_dir`, or a `~/` path. They work with a config (from `-c` or `GOOSE_TRIGGERS_CONFIG`) whose paths are absolute: `approve 3 --yes` and `reject 3` acted on a held run. `run` with such a config refuses unless `GOOSE_PATH_ROOT` is set, and then it starts.
6. **The suite's kept folder on a refusal:** fixed. The README now says when a folder is kept. These cases keep one with `--keep` and remove it without: `selftest --goose /bin/false`, `hostile-suite --goose /bin/false`, and a provider check refused with 401 (that one keeps goose's `goose-root` with its logs). The path is printed after the reason. These leave none, with or without `--keep`: a `--goose` path that does not exist, a CLI-agent provider, `curl` missing from `PATH`, and a bad option (exit 2). See finding 1 for the README's wording.
7. **`--provider ''` not a command-line error:** fixed. `--provider ''` and `--model ''` exit 2 (`a value is required ...`). See finding 3 for a value that is only a space.
8. **A hard-to-read log line:** fixed. It now reads `webhook: refused (401) a request under /hooks/ to no webhook rule's path: wrong or missing secret`, and `webhook /hooks/form: refused (401): wrong or missing secret` for a rule's path.

## Findings

1. **Cosmetic, README wording: "names it after the reason".**
   - What this test did: `selftest --goose /bin/false --keep` and `hostile-suite --goose /bin/false --keep`.
   - What happened: each printed the reason, then `its files, kept with --keep: /tmp/goose-triggers-selftest-1004-1794118341` (or `-hostile-...`). The folder has the usual name.
   - What the README says: a run that cannot start "with `--keep` keeps it and names it after the reason". This reads as if the folder's name came from the reason. "says where, after the reason" would match what happens.
2. **Cosmetic: a `--goose` path that does not run is answered with "pass --goose <path>".**
   - What this test did: `selftest --goose /bin/false` and `selftest --goose /tmp`.
   - What happened: ``goose-triggers: `/usr/bin/false --version` failed (exit status: 1): pass --goose <path>``, and ``running `/tmp --version`: Permission denied (os error 13): install goose (...) or pass --goose <path>``. The user already passed one.
   - What the README says: nothing about the message. "pass the goose binary to --goose" or "--goose /tmp is not a goose binary" would be clearer.
3. **Cosmetic: `--provider ' '` (a space) is not refused as empty.**
   - What this test did: `hostile-suite --provider ' ' --model x`.
   - What happened: the suite ran its provider check and said `goose got no answer from x through its   provider (exit status: 1) ... Unknown provider: .` (exit 1). An empty `--provider ''` exits 2.
   - What the README says: "`--provider` and `--model` may not be empty."

## Not checked

- The install from source (`cargo install ...`) and `cargo test`. The image has no Rust toolchain, and none was installed.
- Anything a real model does: the stand-in makes one fixed call. Two provider checks (`hostile-suite --provider openai` with only a placeholder key in the environment) reached api.openai.com, which refused them with 401. The README says the suite takes the host from the environment, and this test had set it only in goose's `config.yaml`.
- Whether goose-triggers compares an approved call as JSON with key order aside: goose gave the hook the input with its keys in the same order both times (step 6).
- goose's default provider set in `/etc/goose/config.yaml`, which needs root.
- A run that goose makes without loading the plugin (the README says it is stopped and marked failed). This test found no way to make goose skip the plugin while goose-triggers' own checks pass.
- `delegate`, `execute_javascript` and `manage_extensions` holds (earlier install tests checked them).
- goose being sent SIGTERM and then SIGKILL when it does not stop within 5 s of SIGINT. goose-triggers killed with SIGKILL.
- The selftest's terminal check where no pseudo-terminal can be opened.
- `status | head` giving exit status 141 under `pipefail`: `status --all | head -1` gave 0 here, because its short output was written before `head` exited (as with `cat`). The selftest's own check of this passed.
- The GitHub workflows (CI and Release install).
