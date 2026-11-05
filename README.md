# goose-triggers

Event-driven background runs for [goose](https://github.com/aaif-goose/goose), with the user in the loop.

goose can already run on a timer. goose-triggers runs it when something happens: a file lands in a folder, or a webhook fires (from a form, a home-automation hub, a CI system). Each matching event starts a goose recipe in the background, as an ordinary goose session that you can open, inspect and continue.

You mark the actions that need your approval (running a command, writing a file, sending a mail through an extension). When a run reaches one, goose-triggers holds it: the call is denied with a reason, the headless goose run is stopped, and the pending call is stored. The run waits, across restarts of goose-triggers, until you approve or reject it. Approving resumes the same goose session with a one-time permission for exactly that call (same tool, same input); a call that differs is held again. Rejecting ends the run.

**Status: release 0.0, a working prototype of that core path.** Linux x86-64. Tested with goose 1.57.0. See [Limits of 0.0](#limits-of-00).

## How it works

- A **rule** names an event source (a folder and a file-name pattern, or a webhook), a goose **recipe**, and the tools to **hold**.
- Each event becomes a **run**: `goose run --recipe <recipe> --params <field>=<value> ... --output-format stream-json`, headless, in the rule's working folder. The event's fields are passed as the recipe's parameters of the same names.
- goose-triggers writes its own goose plugin into each rule's working folder (`.agents/plugins/goose-triggers/`). goose loads it when the run starts there. Its `PreToolUse` hook calls `goose-triggers hook`, which allows the call, or holds it when the tool matches the rule's `hold` list. Its `SessionStart` hook records the run's goose session. The hook does nothing in goose sessions that goose-triggers did not start.
- On a hold, the hook denies the call with a reason and stores it in goose-triggers' SQLite store; the run becomes `held`, every further call in it is denied, and goose-triggers stops the goose process as soon as goose has recorded the denial.
- `goose-triggers approve <run>` stores a one-time permission for that tool name and input (compared as JSON, key order aside). goose-triggers then runs `goose run --resume --session-id <session> --text "<the user approved this call ...>"`. When goose makes that exact call again, the hook lets it through once. A different held call stops the run again for a new decision.
- `goose-triggers reject <run>` ends the run; the held call never runs.
- goose runs with `GOOSE_MODE=auto`: goose's own confirmations do not apply to headless runs, so the hold is the gate.

Run states: `queued`, `running`, `held`, `approved` (waiting to resume), `done`, `failed`, `rejected`.

## Requirements

- Linux x86-64.
- The goose CLI on your PATH (or set `goose:` in the config), configured with a provider for which goose runs the tools itself: an API provider (OpenAI, Anthropic, Google, OpenRouter, ...) or a local model (Ollama, ...). With CLI-agent providers (`claude-code`, `codex`, `gemini-cli`, `cursor-agent`, the `*-acp` providers), the other agent runs its own tools and goose's hooks never see them, so goose-triggers refuses to run rules with holds on those providers.

## Install

From the release (a static binary):

```sh
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.0/goose-triggers-v0.0.0-x86_64-linux.tar.gz
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.0/goose-triggers-v0.0.0-x86_64-linux.tar.gz.sha256
sha256sum -c goose-triggers-v0.0.0-x86_64-linux.tar.gz.sha256
tar -xzf goose-triggers-v0.0.0-x86_64-linux.tar.gz
mkdir -p ~/.local/bin
install -m 755 goose-triggers-v0.0.0-x86_64-linux/goose-triggers ~/.local/bin/
goose-triggers --version
```

(`~/.local/bin` must be on your PATH.)

Or from source, with Rust 1.85 or newer:

```sh
cargo install --locked --git https://github.com/alepacheco/goose-triggers --tag v0.0.0
```

## Check it works: selftest

```sh
goose-triggers selftest
```

The selftest runs the whole path with your goose binary and a scripted model server built into goose-triggers, which answers goose's model requests from a fixed script. It needs no API key and does not touch your goose settings or sessions: everything happens in a temporary folder, with goose's own folders there too (`GOOSE_PATH_ROOT`). It checks, in order:

1. a new file in a watched folder starts a headless goose recipe run, with the file's name as a recipe parameter;
2. the run is held at the marked call (`shell`), and goose-triggers stops goose there;
3. a webhook with a wrong secret is refused; one with the right secret starts a second run, also held;
4. goose-triggers is stopped and started again: both runs are still held, their calls stored;
5. `approve` resumes the first run in the same goose session, the approved call runs once, and the run is done;
6. `approve` on the second run resumes it, its model makes a different call, which is held again; `reject` ends the run, and no held call ran.

Pass `--goose <path>` to test another goose binary, `--keep` to keep the temporary folder.

## Use

1. Write a config at `~/.config/goose-triggers/config.yaml` (or pass `-c <file>`, or set `GOOSE_TRIGGERS_CONFIG`). This one is in [`examples/config.yaml`](examples/config.yaml), with its recipes in [`examples/recipes/`](examples/recipes/):

   ```yaml
   webhook:
     listen: 127.0.0.1:8787
     secret_env: GOOSE_TRIGGERS_WEBHOOK_SECRET

   rules:
     - name: inbox-notes
       folder: ~/Inbox
       pattern: "*.pdf"
       recipe: recipes/file-note.yaml
       hold: [shell, write, edit]

     - name: form-entry
       webhook: true
       recipe: recipes/form-entry.yaml
       hold: [shell, write, edit]
   ```

2. Start it (it runs until you stop it with Ctrl-C or SIGTERM):

   ```sh
   export GOOSE_TRIGGERS_WEBHOOK_SECRET="$(head -c 24 /dev/urandom | base64)"
   goose-triggers run
   ```

3. Make an event: drop a PDF into `~/Inbox`, or post to the webhook:

   ```sh
   curl -X POST http://127.0.0.1:8787/hooks/form-entry \
     -H "Authorization: Bearer $GOOSE_TRIGGERS_WEBHOOK_SECRET" \
     -d '{"name": "Test Person", "email": "test@example.com"}'
   ```

4. See what happened, and decide on held runs:

   ```sh
   goose-triggers status        # held runs show the call they wait on
   goose-triggers log 3         # one run: its event, held calls, timeline and goose's messages
   goose-triggers approve 3     # resume run 3; that exact call may run once
   goose-triggers reject 3      # end run 3
   ```

   A run's goose session is an ordinary goose session: `goose session list` shows it (named `goose-triggers <rule> run <n>`), and you can open it with `goose session --resume --session-id <id>`.

## Config reference

| key | meaning |
|---|---|
| `state_dir` | where the store (`state.db`), run logs (`runs/`) and default working folders (`work/<rule>`) live. Default `~/.local/share/goose-triggers` |
| `goose` | the goose binary. Default `goose` on PATH |
| `max_concurrent_runs` | goose processes at once. Default 2 |
| `webhook.listen` | address of the webhook listener. Default `127.0.0.1:8787` |
| `webhook.secret_env` | environment variable holding the shared secret (at least 16 characters); or `webhook.secret` for the secret itself |
| `rules[].name` | letters, digits, `-` and `_` |
| `rules[].folder`, `rules[].pattern` | a folder source: new files in the folder (not its subfolders) whose name matches the pattern (`*`, `?`, `[...]`; default `*`). Hidden files are ignored |
| `rules[].webhook: true` | a webhook source: `POST /hooks/<rule name>` |
| `rules[].recipe` | the goose recipe (YAML or JSON) |
| `rules[].hold` | tool names to hold; `*` and `?` match any characters. goose's developer tools are `shell`, `write`, `edit`, `tree`, `analyze`; an extension's tools are `<extension>__<tool>`, so `gmail__*` holds every tool of an extension named `gmail` |
| `rules[].working_dir` | the folder goose works in. Default `<state_dir>/work/<rule name>` |
| `rules[].provider`, `rules[].model`, `rules[].max_turns` | passed to `goose run` as `--provider`, `--model`, `--max-turns` |

Relative paths are relative to the config file; `~/` is your home folder.

**Hold `shell` whenever you hold anything.** A run that may run commands can do through them whatever a held tool would do. goose-triggers warns at start when a rule holds tools but not `shell`.

### Event fields

The fields an event gives; a recipe gets those it declares as parameters, by the same names. A run whose event lacks a parameter the recipe requires (no default) fails without starting goose, and says which.

- Folder events: `file_path` (the full path), `file_name`, `folder`.
- Webhook events: each top-level field of a JSON object body (text as it is; numbers, booleans, arrays and objects as JSON text), and `body`, the whole body as text (unless the JSON has its own `body` field). The body may be up to 1 MiB.

The webhook answers `202 {"event": <id>, "run": <id>}`, `401` for a missing or wrong secret, `404` for no webhook rule of that name. `GET /health` answers `200`.

## Commands

| command | does |
|---|---|
| `goose-triggers run` | watches the rules' sources and runs their recipes until stopped; one per state folder |
| `goose-triggers status [--all]` | held runs and the 20 latest others (`--all`: every run) |
| `goose-triggers log [<run>]` | a run's event, held calls, timeline and goose's messages; with no run, the latest timeline of all runs |
| `goose-triggers approve <run>` | approves the call a held run waits on; `run` resumes the same session |
| `goose-triggers reject <run>` | rejects it; the run ends |
| `goose-triggers selftest [--goose <path>] [--keep]` | the end-to-end check above |

`approve` and `reject` work whether `goose-triggers run` is running or not: an approved run resumes once it runs.

## Tests

```sh
cargo test
```

runs the unit tests (rules, the store's hold and approval logic, the hook's decisions, folder and webhook sources, recipe parameters, the scripted model server) and then the selftest above, when a goose binary is on PATH (or at the path in `GOOSE_BIN`); without one, the selftest is skipped and says so.

## Limits of 0.0

- Linux only (x86-64 release binary). goose-triggers uses Linux's parent-death signal so goose stops if goose-triggers dies.
- Sources: folders (read every second, not recursive) and webhooks. Webhooks are plain HTTP with a bearer secret: keep the listener on localhost, or put it behind a TLS proxy.
- You decide on holds from the command line; there are no notifications yet. The config is read when `goose-triggers run` starts.
- The hold depends on goose loading goose-triggers' plugin from the working folder. goose-triggers checks goose's settings for a disabled plugin before each run, and stops a run, marking it failed, if goose did not run the plugin's session-start hook.
- One approval lets one call through: once the run stops at another held call, an earlier unused approval lapses.

## License

Apache-2.0, like goose. See [LICENSE](LICENSE).
