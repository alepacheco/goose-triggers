# goose-triggers 0.0.5: clean-install test by an automated tester

Verdict: FAIL

This test installed goose-triggers 0.0.5 and used it from scratch, following only the README at the `v0.0.5` tag (the same README as in the release tarball) and the tool's `--help`. It ran in a clean Debian bookworm-slim container with goose 1.57.0 and no provider and no API key. Each group of steps ran in a fresh container as a normal user (uid 1000, not in `/etc/passwd`) with `HOME=/tmp`. No step ran as root, and nothing was installed. No model was used. Where a step needed a run that is really held (approve, reject, resume, a held `delegate` call), the test pointed goose's `openai` provider at a small scripted stand-in for an OpenAI-compatible endpoint on 127.0.0.1, written for this test in Perl (which the image has). The stand-in answers with one fixed tool call while the last message is the user's, and with the text `done` after a tool result. It can also wait before it answers, or refuse every request with 401 or 404. It holds no model, so nothing here shows what a real model does. The verdict is FAIL because of one bug: goose-triggers' hook blocks tool calls in goose sessions it did not start. These include a run's own session when you open it again as the README says (finding 1). Everything else worked as the README says. Of the 0.0.4 install test's findings, nine are fixed, and the last is fixed except for one part that could not be re-checked.

## Steps and results

1. **Install block, verbatim** (run with `bash -x` in an empty folder). PASS. Both downloads worked. `sha256sum -c` printed `goose-triggers-v0.0.5-x86_64-linux.tar.gz: OK`, and `goose-triggers --version` printed `goose-triggers 0.0.5` (exit 0). The tarball (2,355,218 bytes) holds these files:
   - a static x86-64 binary (4,868,984 bytes);
   - `LICENSE`;
   - `README.md`, identical to the README at the tag;
   - `examples/`, identical to the three files the Use section downloads.

   Its SHA-256 matches the digest GitHub lists for the asset. Three corrupted copies fail `sha256sum -c` (exit 1): one byte appended, one byte changed, or the file cut short. See finding 2 for what the block does next.
2. **Help and command-line errors.** PASS.
   - `--help`, `-h`, `-V`, `--version`, `help`, `help <command>` and each command's `--help` work (exit 0). With no command, the help is printed with exit 2.
   - Exit 2 with clap's message: an unknown option or command, a missing `<RUN>`, an extra argument, `log abc`, `log -1`, a run number too large, and an option missing its value.
   - Exit 2 for `-c` or `--config` with `selftest` or `hostile-suite` (before or after the command): `selftest and hostile-suite make a config of their own ...: -c/--config is not for them`.
   - Exit 2 for these bad values: `--holds ''`, `' '` or `','`; `--approvals 11`, `-1` or `abc`; `--run-minutes 0`, `1441` or `1.5`; `--provider` without `--model`, or the reverse; `--approvals` or `--run-minutes` with no provider.
   - `--approvals 0` and `10`, and `--run-minutes 1` and `1440`, are accepted.
   - Before any `run`: `status`, `log`, `approve` and `reject` say `no goose-triggers state in /tmp/.local/share/goose-triggers yet ...` (exit 1).
3. **`selftest`.** PASS. All 31 checks passed in 15 s (exit 0). The terminal check ran, because a pseudo-terminal was available. Nothing was left in `/tmp`, and nothing was made under `HOME` (no `/tmp/.config`, no `/tmp/.local/share`). These pass too:
   - `--goose /usr/local/bin/goose`;
   - `GOOSE_TRIGGERS_CONFIG` naming a missing file, or set but empty (the README says the selftest does not read it).

   `--goose` naming a missing file, `/bin/false`, a folder or a file that cannot run gives a clear error (exit 1). Without `--keep`, no folder is left. With `--keep`, the folder is kept, as the README says. goose missing from `PATH` says `install goose (...) or pass --goose <path>`.
4. **`selftest --keep`, then the kept folder.** PASS. The output ends with `its files: /tmp/goose-triggers-selftest-<n>`.
   - `-c <folder>/config.yaml status --all` lists 8 runs: done, rejected (with the rejected call) and failed (with their reasons).
   - `log 1`, `log 2` and `log 3` show the event, the held calls in full, the timeline and goose's messages. `log` with no run shows the timeline of all runs.
   - `log 0` and `log 999` say `there is no run ...` (exit 1).
   - As the README says, nothing there is still held: `approve 1 --yes`, `approve 1` and `reject 1` answer `run 1 is done, not held` (exit 1).
   - `status --all | head -1`, `log 1 | head -2` and `log | true` end quietly. Under `set -o pipefail` their exit status is 141.
5. **`hostile-suite` with the default holds, and `--keep`.** PASS. The suite took 16 s and ended `every result as expected` (exit 0), leaving nothing in `/tmp`.
   - It sent 16 events. With no holds, 13 of 13 canaries tripped. With `[shell, write, edit, read_image]`, 0 of 13 were carried out, and all 13 were held at their first injected call.
   - The structure check refused 2 of 2 values that would change the recipe and 1 of 1 that would break it, in both passes.
   - goose offered `analyze, edit, load_skill, read_image, shell, tree, write`. The tools left open were named as only reading.
   - Paths print as `<suite>`.
   - `--keep` keeps the folder with `results.md`, `holds-off/`, `holds-on/` and `goose-root/`.
6. **A weaker `--holds`.** PASS.
   - `--holds shell,write,edit` and `--holds 'shell write edit'` give the same result. W9 and F7 were carried out through `read_image`, which was marked `MORE THAN READS`. The suite reported `NOT every result as expected` (exit 1) and kept its folder, as the README says.
   - `'shell, write ,edit'` gives the same result.
   - `--holds shell` let 6 of 13 through (exit 1).
   - `--holds 'shell write edit read_image'` and `--holds '*'` pass (exit 0). With `'*'`, the tools line reads `Not held ...: none`.
7. **`hostile-suite --provider ... --model ...` with no provider reachable.** PASS.
   - **The README's command, verbatim**, with `OLLAMA_HOST` as shown and nothing listening. It printed `Checking that goose gets an answer from qwen3:8b through its ollama provider (at most 20 minutes) ...`. After 100 s it stopped with `goose got no answer ... so the suite sent nothing; goose said: ... Could not connect to 127.0.0.1:11434` (exit 1). No folder was left.
   - **A host that accepts connections and never answers**, with `--run-minutes 1`: it stopped after 60 s with `... within 1 minute (--run-minutes), so the suite sent nothing`.
   - **`--provider openai` with no key:** it stopped after 1 s with goose's 401. That one request, with no key, went to api.openai.com.
   - **A stand-in that refuses the key (401):** it stopped after 1 s. **A stand-in that does not have the model (404):** it stopped after 7 s.
   - **CLI-agent providers:** `claude-code`, `codex`, `gemini-cli`, `cursor-agent`, `claude-acp`, `codex-acp` and `gemini-acp` were refused within 1 ms (exit 1). `Claude-Code` (other capitals) is not refused by name, but goose's check fails it as an unknown provider.
   - **A stand-in that answers** (`--approvals 1 --run-minutes 2 --keep`): the check printed `it answers (0 s)`. Both passes and the approval pass ran in 38 s. The table, summary and `results.md` were written, and the suite ended `every result as expected` (exit 0).
8. **Use, steps 1 to 5, verbatim, without a provider.** PASS.
   - Step 1 fails as expected: `No provider configured. Run 'goose configure' first.` (exit 1).
   - Step 2: the three downloads match the tarball's examples. `webhook-secret` is mode 600 and 32 characters long.
   - Step 3: `goose-triggers run` starts. It prints, for each rule, the new warning that neither the rule nor its recipe names a provider and goose has none set.
   - Step 4: the curl got `{"event":1,"run":1}`. A PDF dropped into `~/Inbox` started run 2.
   - Step 5: `status` and `log 1` show both runs failed with goose's own reason. `approve 1` and `reject 1` answer `run 1 is failed, not held` (exit 1). `goose session list` shows the sessions named `goose-triggers <rule> run <n>`.
9. **Held runs, with the stand-in as goose's provider** (goose's `config.yaml` names `openai`, and `OPENAI_HOST` points at the stand-in). PASS.
   - Step 1 now answers.
   - The webhook run is held at `shell`. `status` shows the call it waits on, and `log` shows it in full.
   - `approve` with no terminal shows the call and refuses without `--yes` (exit 1).
   - On a terminal (through `script`), an answer of `n` or Enter leaves the run held (exit 1). An answer of `y` approves it. The running `goose-triggers run` then resumes the same session, the call runs once, and the run is done.
   - A PDF run held, then rejected, ends without running the call. Rejecting it again exits 1.
   - A resumed run whose model makes a different call is held again.
   - A run approved while `run` is stopped resumes at the next start and is done.
   - A run approved and then rejected while `run` is stopped is not resumed.
   - With goose's `summon` extension in the recipe, a `delegate` call is held. `approve --yes`, and `approve` answered `y`, refuse with `goose-triggers does not approve delegate ...` (exit 1). `reject` ends the run.
10. **Opening a run's goose session again, and other goose sessions in a rule's working folder.** FAIL (finding 1). Run on a terminal, in the run's working folder, with `goose-triggers run` stopped, `goose session --resume --session-id <id>` opens the session. The stand-in's next call there (a `write`) was blocked by goose-triggers' hook. `goose run --text ...` in that folder had calls blocked too, at random.
11. **Start refusals in the Config reference.** PASS. Each item the README lists was refused with a clear message (exit 1):
    - an unknown key (top level, in a rule, under `webhook`), and a malformed value (`max_concurrent_runs: two`, `hold: shell`);
    - two rules with the same name, and a name with a space, a dot, or no characters at all;
    - the pattern `[abc`;
    - a rule with no source, or with both;
    - a missing folder or recipe;
    - a webhook rule with no `webhook:` section; no secret; a secret of 15 characters; `secret_env` unset or holding 3 characters; both `secret` and `secret_env`;
    - `max_concurrent_runs: 0`; `rules: []` and no `rules` key;
    - `goose:` set to a missing path or `/bin/false`;
    - another goose plugin in the working folder;
    - recipes that goose cannot use: an empty recipe, `foo: bar`, broken YAML, broken JSON, a recipe with a file parameter;
    - a listen address in use, not on this machine, without a port, or with a port that is not a number;
    - a working folder or `state_dir` that cannot be made, a working folder that is a file, and a read-only working folder;
    - a second `run` on the same state folder, with another port or with the same config.

    These start, as the README says: a 16-character secret; an empty folder in `.agents/plugins` (0.0.4 refused it); a JSON recipe; `[::1]:<port>`. The README's start warnings appear in these cases:
    - a rule that holds only `shell`, only `gmail__*`, or a hold name with a space;
    - no provider anywhere.

    No warning appears with `hold: ["*"]`, or when each rule names a provider. See finding 4 for a pattern with a slash.
12. **CLI-agent providers and goose's plugin settings, as goose reads them.** PASS.
    - **CLI-agent providers refused:** these sources are refused when they set a CLI-agent provider:
      - a rule's `provider:` (`claude-code`, `gemini-acp`);
      - a recipe's `settings.goose_provider`;
      - goose's default, through `GOOSE_PROVIDER`, through `ACTIVE_PROVIDER`, through `active_provider:` in `config.yaml` (the key goose itself writes), or through `GOOSE_ADDITIONAL_CONFIG_FILES`.
    - **Precedence:** a later additional file, and your own `config.yaml`, win, which matches what `goose info -v` reports. An upper-case `ACTIVE_PROVIDER:` key in `config.yaml` is not taken as the default. goose does not take it either (`No provider configured`).
    - **Plugin settings refused** (checked against goose 1.57.0's own reading, `enabledPlugins` and `disabledPlugins` being lists):
      - `disabledPlugins: ["goose-triggers"]` in the working folder's `.config/goose/settings.local.json`, in its `.config/goose/settings.json`, in `~/.config/goose/settings.json`, and in `$GOOSE_PATH_ROOT/.config/goose/settings.json`;
      - a file that lists the plugin both as enabled and as disabled;
      - `plugins: {<working folder>/.agents/plugins/goose-triggers: {enabled: false}}` in `config.yaml`, also when the working folder is written with a symlink, a trailing slash or `/./`;
      - the same as a JSON map in `PLUGINS`.
    - **Plugin settings that start:** an earlier file that lists the plugin as enabled (local over project, project over user). Settings goose itself passes over also start, and goose then loaded the plugin, so the run was held. These were a list-shaped `plugins:` value, YAML in `PLUGINS`, a `settings.json` with a trailing comma, and an entry in an additional config file.
    - **Added while running:** a `disabledPlugins` added while `run` runs makes the next run fail before goose starts, with the reason.
13. **Config file named by `-c` or `GOOSE_TRIGGERS_CONFIG`, and `HOME` unset.** PASS.
    - A missing file, an empty file, a file holding only a comment, or a folder, named by `-c`, is refused by `status`, `status --all`, `log`, `log 1`, `approve 1 --yes`, `reject 1` and `run` (exit 1). Each message now ends with `(see the README for an example)`.
    - `GOOSE_TRIGGERS_CONFIG` naming a missing file, an empty file or a folder, or set but empty, is refused (exit 1).
    - With `HOME` unset or empty (and no entry in `/etc/passwd`), `run` refuses: `no home folder ... Set HOME`. It starts when `GOOSE_PATH_ROOT` is set. The selftest passes. See finding 5 for the other commands.
14. **Webhook answers and their order.** PASS.
    - **Without the secret**, every path under `/hooks/` gets 401. The cases tried were:
      - no header, a wrong secret, the secret without `Bearer`, `bearer` in lower case, the secret with one character added;
      - an unknown rule, the folder rule's name, `/hooks/`, `/hooks/form/x`;
      - GET, PUT, DELETE and HEAD;
      - a chunked body, 65,537 bytes, a bad `Content-Length`, a short body, a stalled body.

      `/hooks`, `/nope` and `POST /health` get 404, and GET and HEAD `/health` get 200.
    - **With the secret:**
      - 202 `{"event":N,"run":N}`, also with a query string and for HTTP/1.0;
      - 404 for an unknown rule, the folder rule's name, `/hooks/` and `/hooks/form/x`, and before 405 (GET on an unknown rule), 411 (chunked) and 413;
      - 405 for GET, PUT and DELETE, and before 411;
      - 411 for a chunked body, and before 413;
      - 413 at 65,537 bytes, or for `Content-Length: 99999`; exactly 65,536 bytes is accepted;
      - 400 for a non-numeric `Content-Length`, a garbage request line, and a body that ends (the sender closes) before its `Content-Length`;
      - 408 after 10.1 to 10.3 s for stalled headers, a stalled body, and one byte every 0.8 s.
15. **Event fields.** PASS.
    - Numbers, booleans, arrays and objects arrive as JSON text: `n = 42`, `b = true`, `arr = [1,"x"]`, `obj = {"k":"v"}`. Unicode passes through.
    - A JSON `body` field is kept. A body that is not JSON, a JSON array, or an empty body gives only `body`.
    - Fields the recipe does not declare are not passed to goose.
    - A body without `name` gets 202, and the run then fails at once with `the recipe needs parameter(s) name that the event does not have`.
    - See finding 3 for `null`.
16. **Folder source.** PASS. `new.pdf`, `name with spaces.pdf` and a file moved in with `mv` started runs. These did not:
    - a PDF already there before the first watch;
    - `upper.PDF`, `notes.txt` and `.hidden.pdf`;
    - `sub/deep.pdf`.

    A PDF dropped while `run` was stopped was taken at the next start. SIGINT and SIGTERM stop `run` with exit 0.
17. **Ctrl-C.** PASS. Every interrupt below exited with status 130 and left no goose process. The README promises Ctrl-C; SIGTERM was also tried and behaves the same.
    - **`selftest`:** SIGINT was sent to the process group (as a terminal sends it) at 1, 4, 8 and 12 s, and to the process only at the same times. SIGTERM was sent at 6 s. A real `^C` was typed on a pseudo-terminal at 6 s. Each time it exited 130 within 0.2 to 3.2 s, printed `interrupted; its temporary folder is removed`, and left no folder.
    - **`hostile-suite`:** the same at 2, 8 and 15 s, and with SIGTERM and a real `^C` at 8 s. Each time it exited 130 within 0.7 s and left no folder.
    - **During `hostile-suite --provider` checks:** SIGINT during the provider check (a host that never answers) exited 130 within 40 ms. SIGINT during a pass with the stand-in exited 130 too. Neither left a folder.
    - **With `--keep`:** both commands print `interrupted; its files: ...` and keep the folder, as the README now says.
18. **Other promises.** PASS.
    - **goose's command line and environment:** a run's goose is started in the working folder as `goose run --recipe <recipe> --params name=... --params email=... --name "goose-triggers form run 1" --provider openai --model gpt-4o-mini --max-turns 7 --quiet --output-format stream-json`. Its environment has `GOOSE_MODE=auto` and `GOOSE_SLASH_COMMANDS_ENABLED=false`.
    - **`max_concurrent_runs`:** at the default of 2, a third event waits as `queued`.
    - **The plugin:** it is written to `.agents/plugins/goose-triggers/` in each working folder. A changed `hooks.json` is written again before the next run.
    - **`status`:** it shows the held runs and the 20 latest others (23 lines with 3 held). `--all` shows all 30 runs.
    - **goose-triggers killed:** with `goose-triggers run` killed by SIGKILL while goose runs, goose was gone within 4 s. At the next start the run was `failed: goose-triggers stopped while this run was going`.

## The 0.0.4 install test's findings

1. `--holds` separated by spaces: fixed. `'shell write edit'` now equals `shell,write,edit`, and the README and `--help` say so.
2. An unreachable provider found only run by run: fixed. The provider is checked before pass 1 and nothing is sent. That took 100 s (connection refused), 1 s (bad key), 7 s (unknown model), or at most `--run-minutes` (a silent host).
3. `-c` and `GOOSE_TRIGGERS_CONFIG` taken and ignored by `selftest` and `hostile-suite`: fixed. `-c` exits 2, and the README now says neither reads `GOOSE_TRIGGERS_CONFIG`.
4. Temporary folders kept in cases the README did not mention: fixed. The README now says when they are kept, and a suite that refuses because `curl` is missing leaves none. One small inconsistency is left (finding 6).
5. Bad option values exiting 1, with limits only in `--help`: fixed. They exit 2, and the README gives the ranges.
6. No warning when goose has no provider: fixed. `run` warns for each rule at start.
7. An empty recipe or YAML that is no recipe accepted at start: fixed. Both are refused at start with goose's reason.
8. The webhook revealing rule names without the secret: fixed. Every path under `/hooks/` gets 401 first, whatever the method, body size or encoding.
9. The plugin-disabling setting not named: fixed. The README names the settings, and each is refused as goose 1.57.0 reads them (step 12).
10. Cosmetics: fixed, with one part not re-checked.
    - Fixed: `status` shows the rejected call; an empty folder in `.agents/plugins` is accepted; every command adds `(see the README for an example)`; the suite says `1 minute`.
    - Not re-checked: the empty tools list (`.`, now `none seen`). The up-front provider check now stops such runs before any event, so the case did not arise.

## Findings

1. **Bug: goose-triggers' hook blocks tool calls in goose sessions it did not start, in a rule's working folder.**
   - What this test did: first, `goose-triggers run` was started once and stopped, so a rule's working folder (`/tmp/forms`) held goose-triggers' plugin. Then the test ran `goose run --text ...` in that folder many times, with the stand-in's next call a `shell` command or a `write`. It also opened a held-then-rejected run's session again, as Use step 5 says, with `goose session --resume --session-id <id>` on a terminal in the working folder.
   - What happened with small calls: 1 of 20 `shell` calls was blocked on an idle machine, and 15 of 20 with the container's two cores busy. goose said: `Tool call blocked because policy hook 'goose-triggers' could not complete: the hook did not receive the request payload. That hook is configured to block on failure.`
   - What happened with a large input: 5 of 5 `write` calls with 70,000 characters of content were blocked. The reopened session's `write` of that size was blocked too. The same session opened from `/tmp` ran it.
   - Why: the plugin's `PreToolUse` hook runs `[ -n "$GOOSE_TRIGGERS_RUN" ] || exit 0; exec ... hook`, with `"on_failure": "block"`. Outside goose-triggers' runs it exits without reading its input. goose 1.57.0 writes the call to the hook's stdin, and when that write fails (the hook has exited; always so when the input is larger than a pipe holds), goose counts it as a hook failure. `block` then denies the call.
   - What the README says: "The hook does nothing in goose sessions that goose-triggers did not start." It also says a run is "an ordinary goose session that you can open, inspect and continue" with `goose session --resume --session-id <id>`. Reading the input before exiting would make the hook a no-op there.
2. **README gap: the Install block goes on after a failed checksum.**
   - What this test did: it ran the block's lines from `sha256sum -c` on, line by line as a paste does, on a downloaded copy with one byte appended.
   - What happened: `sha256sum -c` printed `FAILED`. `tar` still extracted the files before it complained. `install` put the binary in `~/.local/bin`, and `goose-triggers --version` printed `goose-triggers 0.0.5` (exit 0).
   - What the README says: nothing about stopping when the check fails. Chaining the lines with `&&`, or a line saying "stop unless it prints OK", would make the check protect the install.
3. **README gap: a JSON `null` field arrives as empty text.**
   - What this test did: it posted `{"z": null, ...}` to a webhook rule.
   - What happened: the event has `z = ` (an empty value), and a recipe parameter `z` would be given an empty string.
   - What the README says: it covers text, numbers, booleans, arrays and objects, but not `null`.
4. **Minor: a pattern that can never match is accepted.**
   - What this test did: it started `run` with `pattern: "sub/*.pdf"`.
   - What happened: `run` started with no warning. A folder source matches names of files directly in the folder, so this rule never fires.
   - What the README says: `run` refuses "a name or pattern [that] is not valid". It is silent on patterns with `/`.
5. **Minor: with no home folder, only `run` refuses.**
   - What this test did: it ran `status` with `HOME` unset, as a user with no entry in `/etc/passwd`.
   - What happened: `run` refuses (`no home folder ... Set HOME`). `status` falls back to `/` and says `no goose-triggers state in /.local/share/goose-triggers yet`.
   - What the README says: only that `run` refuses. The release notes say the home folder now comes from the user database instead of `/`. Other commands were not tried.
6. **Cosmetic: the suite's kept folder on a refusal.**
   - What this test did: `hostile-suite --provider ollama ... --keep` with nothing listening, and `hostile-suite --keep` with `curl` missing.
   - What happened: with the unreachable provider, the suite printed `its files: /tmp/goose-triggers-hostile-<n>` before the refusal, and kept a folder in which nothing ran. With `curl` missing and `--keep`, it made no folder and printed nothing.
7. **Cosmetic: an empty `--provider ''` is not a command-line error.**
   - What happened: the suite runs its provider check, and reports `goose got no answer from x through its  provider` (two spaces), with exit 1 rather than 2.
8. **Cosmetic: a hard-to-read log line.** `run`'s log says `webhook: a request to a path under /hooks/ that is no webhook rule's refused, wrong or missing secret`.

## Not checked

- The install from source (`cargo install ...`) and `cargo test`. The image has no Rust toolchain, and none was installed.
- Anything a real model does: the stand-in makes one fixed call.
- goose's default provider set in `/etc/goose/config.yaml`, which needs root (the 0.0.4 test checked it).
- The holds on `execute_javascript` and `extensionmanager__manage_extensions` (the 0.0.4 test checked them).
- goose being sent SIGTERM and then SIGKILL when it does not stop within 5 s of SIGINT.
- `approve`, `reject` and `log` with `HOME` unset (only `status` was tried).
- The GitHub workflows (CI and Release install).
