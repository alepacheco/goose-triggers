# goose-triggers 0.0.4: clean-install test by an automated tester

Verdict: PASS

This test installed goose-triggers 0.0.4 and used it from scratch, following only the README at the `v0.0.4` tag (the same README as in the release tarball), in a clean Debian bookworm-slim container with goose 1.57.0, no provider, no API key. Each group of steps ran in a fresh container as a normal user with `HOME=/tmp` (one group ran as root, only to write goose's system config `/etc/goose/config.yaml`). No model was used. Where a step needed a run that is really held (approve, reject, a held `delegate` call, a check as a run resumes), the test pointed goose's `openai` provider at a small scripted stand-in for an OpenAI-compatible endpoint on 127.0.0.1, written for this test: it answers with one fixed tool call, then a fixed text, and holds no model; so nothing here says what a real model does. The findings of the 0.0.3 install test that this test re-checked are fixed.

## Steps and results

1. **Install block, verbatim** (run with `bash -x` in an empty folder). PASS. Both downloads worked, `sha256sum -c` printed `goose-triggers-v0.0.4-x86_64-linux.tar.gz: OK`, and `goose-triggers --version` printed `goose-triggers 0.0.4` (exit 0). The tarball holds a static x86-64 binary (4.8 MB), `LICENSE`, `README.md` (identical to the README at the tag) and `examples/` (identical to the files the Use section downloads). A copy with one byte appended fails `sha256sum -c` (exit 1).
2. **Help and command-line errors.** PASS. `--help`, `-V` and each command's `--help` work. These exit 2 with clap's message: an unknown option, an unknown command, a missing `<RUN>`, an extra argument, `log abc`, and an option missing its value. With no command, the help is printed and the exit status is 2. Before any `run`, `status`, `log`, `approve` and `reject` say `no goose-triggers state in /tmp/.local/share/goose-triggers yet: start goose-triggers run first` (exit 1).
3. **`selftest`.** PASS. All 30 checks passed in about 17 s (exit 0). The terminal step (`approve` answered `n`) ran, because a pseudo-terminal was available, and nothing was left in `/tmp`. `--goose /usr/local/bin/goose` passes too. `--goose /nonexistent/goose` gives a clear error (exit 1).
4. **`selftest --keep`, then `status --all` and `log` on the kept folder.** PASS. The output ends with `its files: /tmp/goose-triggers-selftest-<n>`. `goose-triggers -c <folder>/config.yaml status --all` lists 8 runs: done, rejected, and failed with their reasons. `log 1` shows the event, the held call in full (approved), the timeline and goose's messages. `log` with no run shows the timeline of all runs. As the README says, nothing there is still held: `approve 1 --yes` and `reject 1` answer `run 1 is done, not held` (exit 1).
5. **Output closed early.** PASS. `status --all | head -2`, `log 1 | head -2` and a reader that exits before any output all end quietly with exit status 141, which `set -o pipefail` counts as a failure, as the README says.
6. **`hostile-suite` with the default holds.** PASS. It sent 16 events. With no holds, 13 of 13 canaries tripped. With `[shell, write, edit, read_image]`, 0 of 13 were carried out and all 13 were held at their first injected call. The structure check refused 2 of 2 recipe-changing values and 1 of 1 recipe-breaking value in both passes. goose offered `analyze, edit, load_skill, read_image, shell, tree, write`, and the open ones were all named as only reading. The suite reported `every result as expected` (exit 0, about 40 s) and left nothing in `/tmp`. `--keep` keeps the folder, with `results.md`.
7. **A weaker `--holds`.** PASS. With `--holds shell,write,edit`, W9 and F7 were carried out through `read_image`, which was marked `MORE THAN READS`; the suite reported `NOT every result as expected` (exit 1), as the README says it should. `--holds shell` let 6 of 13 through (exit 1). `--holds '*'` let 0 of 13 through, with `Not held ...: none` (exit 0). See finding 1 for a list separated by spaces.
8. **`hostile-suite --provider ... --model ...` with no provider reachable.** PASS.
   - CLI-agent providers (`claude-code`, `codex`, `gemini-cli`, `cursor-agent`, `claude-acp`, `codex-acp`, `gemini-acp`) were refused within 3 ms, before any pass, with no temporary folder made (exit 1). The same happened with `--approvals 2 --keep`.
   - `--provider` without `--model` exits 2, as does `--model` alone, and `--approvals` or `--run-minutes` without a provider.
   - With `--provider nosuchprovider`, every run failed with goose's `Unknown provider` and the suite failed (exit 1, 17 s). A CLI-agent name written in other capitals (`Claude-Code`) is not refused, but goose itself rejects it as an unknown provider.
   - The README's command under "With a real model", verbatim, with `OLLAMA_HOST` as shown and nothing listening: there is no check up front. With `--run-minutes 1` added to keep it short, all 31 goose runs were stopped at the limit; the suite ended after 32 minutes with `NOT every result as expected` (exit 1) and kept its folder.
   - SIGINT during this suite printed `interrupted; its temporary folder is removed` (exit 130) and left no folder.
   - See finding 2.
9. **Use, steps 1 to 5, verbatim, without a provider.** PASS.
   - Step 1, `goose run --text ...`: `error: No provider configured. Run 'goose configure' first.` (exit 1), as expected.
   - Step 2: the three downloads match the tarball's examples. `webhook-secret` is mode 600 and 32 characters long.
   - Step 3: `goose-triggers run` starts and prints the rules and `goose 1.57.0 (goose); state in /tmp/.local/share/goose-triggers`. A second `goose-triggers run` is refused: `another goose-triggers run is using ... already` (exit 1).
   - Step 4: the curl got `{"event":1,"run":1}`, and a PDF dropped into `~/Inbox` started run 2.
   - Step 5: `status` and `log` show each run failed with goose's own reason (`No provider configured`). `goose session list` shows the sessions named `goose-triggers <rule> run <n>`. `approve` and `reject` on a failed run answer `run 1 is failed, not held` (exit 1).
10. **Folder source.** PASS. These started no run: a PDF already in the folder before the first watch, `upper.PDF`, `notes.txt`, `.hidden.pdf` and `sub/deep.pdf`. `name with spaces.pdf` started a run. A PDF dropped while `run` was stopped was taken at the next start. SIGTERM and SIGINT both stop `run`, with exit 0.
11. **Webhook answers.** PASS.
    - 202 with `{"event":N,"run":N}`, also for HTTP/1.0 and with a query string.
    - 401 with no header, a wrong secret, the secret without `Bearer`, or with `bearer` in lower case.
    - 404 for an unknown rule, the folder rule's name, `/`, `/hooks/form-entry/x`, `GET /nope` and `POST /health`.
    - 405 for GET, PUT and DELETE on a webhook rule's path.
    - 413 at 65,537 bytes; exactly 65,536 bytes is accepted.
    - 411 for a chunked body (curl, and a raw request).
    - 400 for a body shorter than its `Content-Length`, a non-numeric `Content-Length`, and a garbage request line.
    - 408 after 10 to 11 s, for a stalled body, stalled headers, and a request sent one byte every 0.8 s.
    - GET and HEAD `/health` answer 200.
    - Event fields: numbers and objects arrive as JSON text, and a JSON `body` field is kept. A body without `name` gets 202, and the run then fails at once with `the recipe needs parameter(s) name that the event does not have`.
12. **The README's start refusals.** PASS. Each of the following was refused with a clear message (exit 1):
    - an unknown key (top level, and in a rule), a malformed value, two rules with the same name, a name or pattern that is not valid, no source, both sources;
    - a missing folder or recipe, a recipe that is not valid YAML;
    - a webhook rule with no `webhook:` section; a secret that is missing, 5 or 15 characters long, held in `secret_env` as 3 characters or empty, or named by an unset `secret_env`; both `secret` and `secret_env`;
    - `max_concurrent_runs: 0`, no rules;
    - `goose:` set to a missing path, `/bin/false` or a folder;
    - a listen address in use, not on this machine, or without a port;
    - a working folder or `state_dir` that cannot be made;
    - another goose plugin in the working folder;
    - a CLI-agent provider (step 14).

    A 16-character secret starts. A rule that holds some tools but not all four starts with the README's warning. File parameters are refused per event, as the README says (`the recipe's parameter doc is a file parameter ...`).
13. **`GOOSE_TRIGGERS_CONFIG` or `-c` naming a missing or empty file.** PASS. The cases tried were a missing file, a folder, an empty file, a file holding only a newline or only a comment, and `GOOSE_TRIGGERS_CONFIG` set but empty. In each case `status`, `status --all`, `log`, `log 1`, `approve 1 --yes`, `reject 1` and `run` refused (exit 1), while a default state with runs was present; nothing acted on the default state folder. See finding 3 for `selftest` and `hostile-suite`.
14. **CLI-agent providers, wherever goose may take one.** PASS.
    - Refused at start:
      - a rule's `provider:` of `claude-code`, `codex`, `gemini-cli`, `cursor-agent`, `claude-acp` or `codex-acp`;
      - a recipe's `settings.goose_provider: claude-code`;
      - goose's default set to `claude-code` in `~/.config/goose/config.yaml`, with no provider named, or with the recipe naming `openai` (as the README says);
      - the same default set through `GOOSE_PROVIDER`, `ACTIVE_PROVIDER`, `GOOSE_ADDITIONAL_CONFIG_FILES`, `XDG_CONFIG_HOME`, `GOOSE_PATH_ROOT`, or (as root) `/etc/goose/config.yaml`.
    - The precedence matches goose's own, checked with goose itself: `GOOSE_PROVIDER` over `ACTIVE_PROVIDER` over your config over the additional files (colon-separated, a later file winning) over `/etc`.
    - These start, as the README says: a rule naming `openai` while goose's default is `claude-code`, and a rule on `claude-code` with no holds.
    - goose's default changed to `claude-code` after start: the next run fails as it starts, with the reason.
    - Changed while a run is held, then approved: the run fails as it resumes, and the approved call never runs.
15. **Held runs** (with the stand-in). PASS.
    - The run is held at `shell`. `status` shows the call it waits on, and `log` shows it in full.
    - `approve` with no terminal and no `--yes` is refused (exit 1).
    - On a terminal (through `script`), answers of `n`, `no thanks`, Enter or a closed terminal all give `not approved; run 1 stays held` (exit 1). An answer of `Y` approves it. The running `goose-triggers run` then resumes the same session, the call runs once, and the run is done.
    - A run approved while `run` was stopped, whose model makes a different call on resume, is held again.
    - `approve --yes` works with stdout closed.
    - `reject` withdraws an approval, and a restart does not resume that run.
    - A rule without holds runs the call.
16. **A held `delegate` call** (with the stand-in). PASS. With goose's `summon` extension added to the recipe, goose offers `delegate`. A rule holding `[shell, write, edit, read_image]` holds it. Both `approve --yes` and `approve` answered `y` refuse with `goose-triggers does not approve delegate, which starts a subagent on a provider it cannot check ...` (exit 1); the run stays held, and `reject` ends it. `execute_javascript` (with `code_execution`) and `extensionmanager__manage_extensions` are held as well. With the example recipes alone, goose does not offer `delegate` (`Tool 'delegate' is not available`).
17. **Ctrl-C.** PASS. SIGINT was sent to the process group, as a terminal sends it: during `selftest` at 1, 3, 6, 10 and 14 s, and during `hostile-suite` at 2, 8, 20 and 35 s. Each time the command exited 130 within 2 s and printed `interrupted; its temporary folder is removed`, and no folder or goose process was left. The same holds for a real `^C` on a pseudo-terminal, for SIGINT to the process only, and for SIGTERM. With `--keep`, the folder is kept (finding 4).
18. **Not checked.**
    - `cargo install`: there is no Rust toolchain in the image.
    - Anything a real model does.
    - The start refusal for "goose's settings disable goose-triggers' plugin" (finding 9).

## Findings

1. **`--holds` does not say how to list several tools, and a list separated by spaces fails silently.**
   - What this test did: `goose-triggers hostile-suite --holds 'shell write edit read_image'`.
   - What happened: the whole string was taken as one tool name, so the second pass held nothing and 13 of 13 injected calls were carried out (exit 1). The table and the summary print `hold: [shell write edit read_image]`, which reads like four tools held.
   - What the README says: only "`--holds <tools>` tries others" and `[--holds <tools>]`. Only `--help` says "comma-separated". In a config file, a hold name with spaces at least gets the start warning.
2. **With a provider that cannot be reached, the real-model suite finds out only run by run.**
   - What this test did: the README's command under "With a real model", verbatim, with `OLLAMA_HOST` as shown and nothing listening.
   - What happened: there was no check before pass 1, so every run waits out goose's own retries; goose alone gives up after about 101 s. With `--run-minutes 1` added to keep it short, all 31 goose runs (13 a pass, plus the 5 plain entries) were stopped at the limit, and the suite reported after 32 minutes (exit 1). At the default limit, this would take about 50 minutes. The summary never says the provider could not be reached. Every case reads `did not act on it; canary untouched; run timed out (stopped by the suite)`, and the tools line is empty (`goose offered the runs these tools: .`).
   - What the README says: the provider's host or key comes from the environment, and a run that fails or times out makes the suite fail. It does not say that a provider which is not there is found only one run at a time. One call to the provider before pass 1 would catch it in seconds.
3. **`selftest` and `hostile-suite` take `-c` and `GOOSE_TRIGGERS_CONFIG` but ignore them, even when they name a missing or empty file.**
   - What this test did: `goose-triggers -c /tmp/typo.yml selftest`, `goose-triggers -c /tmp/typo.yml hostile-suite`, and `selftest` with `GOOSE_TRIGGERS_CONFIG` naming a missing file or an empty one.
   - What happened: all four ran and passed (exit 0). Both commands list `-c, --config <FILE>` in their `--help`.
   - What the README says: "a file named either way must exist and not be empty, for every command". This is harmless here, since neither command touches a state folder, but it is not true for every command, and an option that does nothing is confusing.
4. **Temporary folders are kept in cases the README does not mention.**
   - (a) What this test did: `hostile-suite` with `curl` not on `PATH`. What happened: it printed `its files: /tmp/goose-triggers-hostile-<n>`, then refused with `install curl` (exit 1), and left a 7 KB folder behind although nothing ran.
   - (b) What happened: a selftest or suite that fails keeps its folder and says so. That is sensible, but the README says only that `--keep` keeps it.
   - (c) What this test did: Ctrl-C with `--keep`, for both `selftest` and `hostile-suite`. What happened: the folder was kept (`interrupted; its files: ...`).
   - What the README says: "Stopped with Ctrl-C, it stops what it started and removes its folder", and for the suite, "`--keep` keeps it ...; Ctrl-C removes it".
5. **Some option values that are not valid exit 1, and their limits appear only in `--help`.**
   - What this test did: `--holds ''`, `--approvals 11` and `--run-minutes 0`.
   - What happened: `--holds ''` (`needs at least one tool name`) and `--approvals 11` (`at most 10`) exit 1, not 2. `--run-minutes 0` is accepted, and its runs were then stopped after about 60 s, not at once.
   - What the README says: "2 when the command line itself is not valid". It gives no maximum for `--approvals` and does not say what `--run-minutes 0` means.
6. **`goose-triggers run` starts, with no warning, when goose has no provider configured.**
   - What this test did: Use steps 2 to 4 without step 1.
   - What happened: `run` started and accepted events, and every run then failed with goose's `No provider configured. Run 'goose configure' first`.
   - What the README says: goose must be configured with a provider, and `run` "checks at start that it runs". That is true, but a user who skipped step 1 learns only at the first event. A start warning would help.
7. **The start check accepts a recipe that is empty, or that is valid YAML but not a recipe.**
   - What this test did: a rule whose recipe was an empty file, and one whose recipe held only `foo: bar`.
   - What happened: `run` started. Each event's run then failed with `goose cannot read the recipe (Error: Invalid recipe: missing field title); the run was not started`.
   - What the README says: `run` refuses "a rule's recipe is not valid YAML or JSON". That is accurate to the letter, but a user would expect a recipe goose cannot read at all to be caught at start too.
8. **The webhook tells a caller without the secret which rule names exist.**
   - What this test did: POST requests with no secret to rule names that exist and do not exist.
   - What happened: `POST /hooks/form-entry` with no secret answers 401, but `POST /hooks/nope` with no secret answers 404. The 405 and 413 answers also come before the secret is checked.
   - What the README says: it lists the answers but not the order in which they are given. This is minor on a listener kept on localhost.
9. **This test could not find which goose setting the start refusal "goose's settings disable goose-triggers' plugin there" refers to.**
   - What this test did: it tried `{"enabledPlugins":{"goose-triggers":false}}` and `{"disabledPlugins":["goose-triggers"]}` in the working folder's `.agents/settings.json` and `.agents/settings.local.json`, in `~/.agents/settings.json` and in `~/.config/goose/settings.json`.
   - What happened: each time, `run` started, goose still loaded the plugin, and the stand-in's `shell` call was held.
   - What the README says: it does not name the setting, so a user cannot tell what this refusal protects against, and this test could not check it.
10. **Cosmetic issues.**
    - With an unknown `--provider`, the summary reads `goose offered the runs these tools: .` (an empty list).
    - An empty folder under `.agents/plugins` in a working folder is refused as `other goose plugins (emptydir)`.
    - When `-c` names a missing file, `status`, `log`, `approve` and `reject` omit the `(see the README for an example)` that `run` adds.
    - The DETAIL column of `status` is empty for a rejected run.
    - The suite prints "longer than 1 minutes".
