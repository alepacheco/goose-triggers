# goose-triggers

Event-driven background runs for [goose](https://github.com/aaif-goose/goose), with the user in the loop.

goose can already run on a timer. goose-triggers runs it when something happens: a file lands in a folder, or a webhook fires (from a form, a home-automation hub, a CI system). Each matching event starts a goose recipe in the background, as an ordinary goose session that you can open, inspect and continue.

You mark the actions that need your approval (running a command, writing a file, sending a mail through an extension). When a run reaches one, goose-triggers holds it: the call is denied with a reason, the headless goose run is stopped, and the pending call is stored. The run waits, across restarts of goose-triggers, until you approve or reject it. Approving resumes the same goose session with a one-time permission for exactly that call (same tool, same input); a call that differs is held again. Rejecting ends the run.

**Status: release 0.0, a working prototype of that core path.** Linux x86-64. Tested with goose 1.57.0. See [Limits of 0.0](#limits-of-00).

[![CI](https://github.com/alepacheco/goose-triggers/actions/workflows/ci.yml/badge.svg)](https://github.com/alepacheco/goose-triggers/actions/workflows/ci.yml) The unit tests and the [selftest](#check-it-works-selftest) run on GitHub Actions on every push and pull request, and every day against goose's latest CLI release (and goose 1.57.0), so a new goose release that breaks goose-triggers shows there: [CI runs](https://github.com/alepacheco/goose-triggers/actions/workflows/ci.yml).

## How it works

- A **rule** names an event source (a folder and a file-name pattern, or a webhook), a goose **recipe**, and the tools to **hold**.
- Each event becomes a **run**: `goose run --recipe <recipe> --params <field>=<value> ... --quiet --output-format stream-json`, headless, in the rule's working folder. The event's fields are passed as the recipe's parameters of the same names.
- goose fills parameters into the recipe's text before it reads the recipe, so a value carrying YAML or JSON syntax could add to the recipe itself (an extension, a retry command) rather than fill in its text. Before each new run, goose-triggers has goose render the recipe twice, with the event's values and with placeholders, and starts the run only if the two have the same structure and the values fill in nothing but the recipe's text fields (`prompt`, `instructions`, `title`, `description`, `activities`, `author`): no value can choose an extension, a command or the provider. goose runs with its slash commands off (`GOOSE_SLASH_COMMANDS_ENABLED=false`), so an event's text cannot start one of your slash-command recipes either. File parameters (`input_type: file`, whose file goose pastes into the recipe) are not filled from events.
- goose-triggers writes its own goose plugin into each rule's working folder (`.agents/plugins/goose-triggers/`). goose loads it when the run starts there. Its `PreToolUse` hook calls `goose-triggers hook`, which allows the call, or holds it when the tool matches the rule's `hold` list. Its `SessionStart` hook records the run's goose session. The hook does nothing in goose sessions that goose-triggers did not start.
- On a hold, the hook denies the call with a reason and stores it in goose-triggers' SQLite store; the run becomes `held`, every further call in it is denied, and goose-triggers stops goose as soon as goose has recorded the denial. It stops goose with SIGINT, as Ctrl-C would: goose cancels its turn, stops the commands it started and saves the session.
- The hook records every call it sees, and the start of every goose turn. If goose runs a tool the hook never saw (goose did not load the plugin), goose-triggers stops the run and marks it failed.
- `goose-triggers approve <run>` stores a one-time permission for that tool name and input (compared as JSON, key order aside). goose-triggers then runs `goose run --resume --session-id <session> --text "<the user approved this call ...>"`. When goose makes that exact call again, the hook lets it through once. A different held call stops the run again for a new decision.
- `goose-triggers reject <run>` ends the run; the held call never runs.
- goose runs with `GOOSE_MODE=auto`. In a headless run goose answers its own confirmation requests itself, allowing the call (tools you set to "ask before" in goose included), so the hold is the only gate.

Run states: `queued`, `running`, `held`, `approved` (waiting to resume), `done`, `failed`, `rejected`.

## Requirements

- Linux x86-64.
- The goose CLI on your PATH (or set `goose:` in the config; `goose-triggers run` checks at start that it runs), configured with a provider for which goose runs the tools itself: an API provider (OpenAI, Anthropic, Google, OpenRouter, ...) or a local model (Ollama, ...). With CLI-agent providers (`claude-code`, `codex`, `gemini-cli`, `cursor-agent`, the `*-acp` providers), the other agent runs its own tools and goose's hooks never see them, so goose-triggers refuses to run rules with holds on those providers.

## Install

From the release (a static binary). It goes in `~/.local/bin`, which must be on your PATH: the `export` line adds it for this shell (add it to your shell's profile to keep it).

```sh
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.2/goose-triggers-v0.0.2-x86_64-linux.tar.gz
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.2/goose-triggers-v0.0.2-x86_64-linux.tar.gz.sha256
sha256sum -c goose-triggers-v0.0.2-x86_64-linux.tar.gz.sha256
tar -xzf goose-triggers-v0.0.2-x86_64-linux.tar.gz
mkdir -p ~/.local/bin
install -m 755 goose-triggers-v0.0.2-x86_64-linux/goose-triggers ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
goose-triggers --version
```

Or from source, with Rust 1.85 or newer (it installs to `~/.cargo/bin`):

```sh
cargo install --locked --git https://github.com/alepacheco/goose-triggers --tag v0.0.2
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
6. `approve` on the second run resumes it, its model makes a different call, which is held again; `reject` ends the run, and no held call ran;
7. a webhook event whose value carries YAML that would add a command to the recipe is refused before goose runs anything;
8. a run that goose fails (its rule names a provider goose does not have) is marked failed with goose's own reason, and none of the event's values in it; the goose session goose made for it is named, though goose failed before goose-triggers' hooks ran;
9. an event whose value breaks the recipe's YAML, and one that lacks the recipe's parameter, fail before goose runs, with reasons that quote none of the sender's text; one whose value goose cannot be given (a NUL byte) fails with goose-triggers' own reason;
10. a run approved while goose-triggers is stopped, then rejected: the approval is withdrawn and the call never runs;
11. `approve` with no terminal to ask on needs `--yes`; `status` and `log` end quietly when their reader goes away (`| head`).

Pass `--goose <path>` to test another goose binary, `--keep` to keep the temporary folder.

## Use

1. Set up goose with a model provider (`goose configure`), one for which goose runs the tools itself ([Requirements](#requirements)), and check that it answers headless:

   ```sh
   goose run --text "Say hello in one line"
   ```

2. Get the example config and its two recipes ([`examples/`](examples/)), make the folder it watches, and make the webhook's shared secret once, in a file only you can read:

   ```sh
   mkdir -p ~/.config/goose-triggers/recipes ~/Inbox
   cd ~/.config/goose-triggers
   base=https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.2/examples
   curl -fsSL -o config.yaml "$base/config.yaml"
   curl -fsSL -o recipes/file-note.yaml "$base/recipes/file-note.yaml"
   curl -fsSL -o recipes/form-entry.yaml "$base/recipes/form-entry.yaml"
   (umask 077; head -c 24 /dev/urandom | base64 > webhook-secret)
   ```

   The config, `~/.config/goose-triggers/config.yaml` (or pass `-c <file>`, or set `GOOSE_TRIGGERS_CONFIG`), reads:

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

3. Start it in one terminal (it runs until you stop it with Ctrl-C or SIGTERM):

   ```sh
   export GOOSE_TRIGGERS_WEBHOOK_SECRET="$(cat ~/.config/goose-triggers/webhook-secret)"
   goose-triggers run
   ```

4. In another terminal, make an event: drop a PDF into `~/Inbox`, or post to the webhook:

   ```sh
   export GOOSE_TRIGGERS_WEBHOOK_SECRET="$(cat ~/.config/goose-triggers/webhook-secret)"
   curl -X POST http://127.0.0.1:8787/hooks/form-entry \
     -H "Authorization: Bearer $GOOSE_TRIGGERS_WEBHOOK_SECRET" \
     -d '{"name": "Test Person", "email": "test@example.com"}'
   ```

5. See what happened, and decide on held runs. Runs are numbered from 1 in the order their events came; `status` shows each run's number. Here, run 1:

   ```sh
   goose-triggers status        # held runs show the call they wait on
   goose-triggers log 1         # one run: its event, held calls in full, timeline and goose's messages
   goose-triggers approve 1     # shows the call, asks, and records the approval; `goose-triggers run` then resumes run 1, and that exact call may run once
   goose-triggers reject 1      # end run 1 (held, or approved but not resumed yet)
   ```

   A run's goose session is an ordinary goose session: `goose session list` shows it (named `goose-triggers <rule> run <n>`), and you can open it with `goose session --resume --session-id <id>`.

To see runs that were held, and their calls, without a model provider, keep the selftest's folder and look at it: `goose-triggers selftest --keep` ends with `its files: <folder>`; then `goose-triggers -c <folder>/config.yaml status --all` and `goose-triggers -c <folder>/config.yaml log 1`.

## Config reference

| key | meaning |
|---|---|
| `state_dir` | where the store (`state.db`), run logs (`runs/`) and default working folders (`work/<rule>`) live. Default `~/.local/share/goose-triggers` |
| `goose` | the goose binary. Default `goose` on PATH |
| `max_concurrent_runs` | goose processes at once. Default 2 |
| `webhook.listen` | address of the webhook listener. Default `127.0.0.1:8787` |
| `webhook.secret_env` | environment variable holding the shared secret (at least 16 characters); or `webhook.secret` for the secret itself |
| `rules[].name` | letters, digits, `-` and `_` |
| `rules[].folder`, `rules[].pattern` | a folder source: new files in the folder (not its subfolders) whose name matches the pattern (`*`, `?`, `[...]`, case-sensitive; default `*`). Files already there when the rule first watches the folder are not new; files that arrive while goose-triggers is stopped are taken when it starts. Hidden files are ignored |
| `rules[].webhook: true` | a webhook source: `POST /hooks/<rule name>` |
| `rules[].recipe` | the goose recipe (YAML or JSON) |
| `rules[].hold` | tool names to hold; `*` and `?` match any characters. goose's developer extension has `shell`, `write`, `edit`, `tree` and `read_image` (goose adds `analyze` with it); an extension's tools are `<extension>__<tool>`, so `gmail__*` holds every tool of an extension named `gmail`. `*` holds every call |
| `rules[].working_dir` | the folder goose works in. Default `<state_dir>/work/<rule name>` |
| `rules[].provider`, `rules[].model`, `rules[].max_turns` | passed to `goose run` as `--provider`, `--model`, `--max-turns` |

Relative paths are relative to the config file; `~/` is your home folder.

**A hold covers only the tools it names.** A run that may run commands or write files can do through them what a held tool would do: hold `shell`, `write` and `edit` whenever you hold anything (goose-triggers warns at start when a rule does not), plus every extension tool that sends or deletes things, or hold `*`. A rule that holds anything also holds goose's code mode (`execute_javascript`, which calls tools out of the hook's sight), its extension manager (`manage_extensions`, which turns on extensions mid-run) and `delegate` (which starts a subagent with a recipe's extensions). goose-triggers refuses to run a rule with holds in a working folder that holds other goose plugins, whose hooks could run commands. Plugins in your own `~/.agents/plugins` load in every goose session, goose-triggers' runs included: their hooks are yours to trust. A rule's holds are copied into each run when its event arrives.

### Event fields

The fields an event gives; a recipe gets those it declares as parameters, by the same names. A run whose event lacks a parameter the recipe requires (no default) fails without starting goose, and says which. Declare parameters as strings (`input_type: string`): goose-triggers does not fill file parameters from events, as goose would paste the named file into the recipe; pass the path as a string and let the recipe read the file with its tools.

- Folder events: `file_path` (the full path), `file_name`, `folder`.
- Webhook events: each top-level field of a JSON object body (text as it is; numbers, booleans, arrays and objects as JSON text), and `body`, the whole body as text (unless the JSON has its own `body` field). The body may be up to 64 KiB.

The webhook answers `202 {"event": <id>, "run": <id>}`, `401` for a missing or wrong secret, `404` for a path that is not `/hooks/<name of a webhook rule>`, `405` for a method other than POST on a webhook rule's path, `413` for a body over 64 KiB, `411` for a chunked body (send a `Content-Length`), `400` for a body that ends before its `Content-Length`, `408` for a request that does not arrive whole within 10 seconds. `GET /health` (and `HEAD`) answers `200`. A `202` means the event is stored and its run queued; a run whose event lacks a parameter the recipe needs then fails at once, and `status` says why.

## Commands

| command | does |
|---|---|
| `goose-triggers run` | watches the rules' sources and runs their recipes until stopped; one per state folder |
| `goose-triggers status [--all]` | held runs, with the call each waits on in short, and the 20 latest others (`--all`: every run) |
| `goose-triggers log [<run>]` | a run's event, its held calls in full, its timeline and goose's messages; with no run, the latest timeline of all runs |
| `goose-triggers approve <run> [--yes]` | shows the call a held run waits on and approves it: on a terminal it asks first, unless `--yes`; with no terminal (a script) it needs `--yes`. `run` resumes the same session |
| `goose-triggers reject <run>` | rejects a held run's call, or withdraws the approval of a run not resumed yet; the run ends |
| `goose-triggers selftest [--goose <path>] [--keep]` | the end-to-end check above |

Read a held call in full with `log <run>` before approving it. `approve` and `reject` work whether `goose-triggers run` is running or not: an approved run resumes once it runs, and until then `reject` can withdraw the approval.

## Tests

```sh
cargo test
```

runs the unit tests (rules, the store's hold and approval logic, the hook's decisions, folder and webhook sources, recipe parameters and the structure check, the scripted model server) and then the selftest above, when a goose binary is on PATH (or at the path in `GOOSE_BIN`); without one, the selftest is skipped and says so.

[CI](https://github.com/alepacheco/goose-triggers/actions/workflows/ci.yml) ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs the same with goose's latest release and goose 1.57.0, builds the static release binary and runs its selftest, on every push and pull request and once a day.

The release binary is built static, with musl:

```sh
rustup target add x86_64-unknown-linux-musl   # and musl-tools (Debian, Ubuntu) for musl-gcc
cargo build --locked --release --target x86_64-unknown-linux-musl
```

## Limits of 0.0

- Linux only (x86-64 release binary). If goose-triggers dies, Linux sends goose SIGINT (parent-death signal), so goose cancels its turn.
- Sources: folders (read every second, not recursive) and webhooks. Webhooks are plain HTTP with a bearer secret: keep the listener on localhost, or put it behind a TLS proxy.
- You decide on holds from the command line; there are no notifications yet. The config is read when `goose-triggers run` starts.
- The hold depends on goose loading goose-triggers' plugin from the working folder. Before each run, goose-triggers rewrites the plugin and checks goose's settings for a disabled one. If goose runs a tool the hook did not see, the run is stopped and marked failed, but that tool has already run.
- A run stopped while it is going (goose-triggers stopped, or goose failed) is marked failed and not retried, except a resumed run that had not yet made its approved call: it resumes again.
- A run has no time limit of its own. `max_turns` on a rule bounds the model's turns, not time. goose ends a tool call after its timeout (300 seconds by default; a shell call may ask for a longer one, or for none), so a long run keeps one of the `max_concurrent_runs` places until it ends or goose-triggers is stopped.
- goose is stopped with SIGINT. If it has not stopped 5 seconds later it gets SIGTERM and then SIGKILL, and a command it had started may then keep running.
- Event values and approved calls go to goose on its command line, which other users of the machine can see in the process list.
- A value that only fills in text is checked by its place in the recipe, not by what it says: a rule's recipe still reads event content, which may carry instructions for the model. That is what holds are for.
- One approval lets one call through: once the run stops at another held call, an earlier unused approval lapses.

## License

Apache-2.0, like goose. See [LICENSE](LICENSE).
