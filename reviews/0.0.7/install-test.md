# goose-triggers 0.0.7: independent release-install test

**Verdict: PASS for the Linux release install and the local scripted paths tested.** No functional failure was observed. One low-severity diagnostic usability issue is listed below. Real-model behavior was not tested.

Tested on 2026-11-09 in a clean Debian GNU/Linux 12 x86-64 container, with one CPU, 3 GiB of memory, bash, curl, and goose 1.57.0 at `/usr/local/bin/goose`. The container had no configured model or model credentials. Only a new test workspace was mounted. The inputs were the [v0.0.7 README](https://github.com/alepacheco/goose-triggers/blob/v0.0.7/README.md), its linked example config and recipes, and the [published release assets](https://github.com/alepacheco/goose-triggers/releases/tag/v0.0.7). No implementation, earlier review report, or author build artifact was read. Nothing in the repository or release was changed or published.

| Check | Observed result |
|---|---|
| README binary install | PASS; both downloads succeeded, checksum `OK`, version `goose-triggers 0.0.7` |
| GitHub asset digest | PASS; matches the downloaded tarball and its `.sha256` file |
| `selftest --keep` | PASS, 33 checks; terminal confirmation check ran without a skip |
| Scripted `hostile-suite --keep` | PASS, 16 cases in each of two passes |
| Explicit working goose path | PASS, selftest 33 checks |
| Explicit unusable goose paths | Correct exit 1 and path-specific diagnostics; one minor stderr omission |
| Blank provider/model values | Correct exit 2 for spaces, tabs/newline, and empty strings |
| Downloaded example folder/webhook sources | Folder event queued; unauthenticated webhook 401; authenticated webhook 202 |
| Example runs without a model | Correctly failed with goose's missing-model reason; no model request made |
| Status, log, approval, rejection | PASS through selftest, retained-state inspection, and terminal-state refusal checks |
| Temporary-folder cleanup | PASS for the early failures checked and a passing selftest without `--keep` |

## Release download and install

The README's release install block was executed as written, from a new working directory:

```sh
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.7/goose-triggers-v0.0.7-x86_64-linux.tar.gz &&
curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.7/goose-triggers-v0.0.7-x86_64-linux.tar.gz.sha256 &&
sha256sum -c goose-triggers-v0.0.7-x86_64-linux.tar.gz.sha256 &&
tar -xzf goose-triggers-v0.0.7-x86_64-linux.tar.gz &&
mkdir -p ~/.local/bin &&
install -m 755 goose-triggers-v0.0.7-x86_64-linux/goose-triggers ~/.local/bin/ &&
export PATH="$HOME/.local/bin:$PATH" &&
goose-triggers --version
```

Observed output:

```text
goose-triggers-v0.0.7-x86_64-linux.tar.gz: OK
goose-triggers 0.0.7
```

The tarball's SHA-256 was:

```text
001740ddfa3af10d5a94d7a7d9e1f39dd070da49ad8e3240afb5b3270939330f
```

`sha256sum goose-triggers-v0.0.7-x86_64-linux.tar.gz` and the tarball's `digest` field from `https://api.github.com/repos/alepacheco/goose-triggers/releases/tags/v0.0.7` agreed. The API listed the tarball as 2,359,568 bytes. The installed binary was copied unchanged into the mounted workspace for subsequent fresh containers.

## Scripted end-to-end checks

```sh
goose-triggers selftest --keep
goose-triggers hostile-suite --keep
```

Both exited 0. The first ended with `selftest passed: 33 checks`. It exercised a folder event, bad-secret and authenticated webhook events, held calls, stopping and restarting the daemon, persistence of holds, approval and resume of the same session, the approved call running once, a different resumed call being held again, rejection, and withdrawing an approval before resume. It also exercised ordinary goose sessions in the plugin's working folder, large writes, malformed/missing/unrepresentable event parameters, invalid recipe startup, missing-provider error reporting, interactive approval answered `n`, approval without a terminal, closed approval output, and quiet status/log pipe closure. The pseudo-terminal check actually ran.

The hostile suite reported:

```text
With `hold: [shell, write, edit, read_image]`, it carried out 0 of 13; 13 were held at their first injected call and rejected.
The recipe-structure check refused 2 of 2 values that would change the recipe and 1 of 1 that would break it, in both passes.
hostile suite: every result as expected
```

All 13 executable injections tripped their canaries with no holds. The three remaining cases were refused by the recipe-structure check in both passes. The offered tools were `analyze`, `edit`, `load_skill`, `read_image`, `shell`, `tree`, and `write`; the suite reported no unexpected unheld tool. These results cover its scripted calls, not arbitrary model behavior or other possible routes.

The retained selftest folder was then inspected using the README's commands. Here `self_dir` is the folder the selftest printed:

```sh
goose-triggers -c "$self_dir/config.yaml" status --all
goose-triggers -c "$self_dir/config.yaml" log 1
goose-triggers -c "$self_dir/config.yaml" log 2
```

All exited 0. Status showed run 1 `done`, runs 2 and 8 `rejected`, and the deliberately invalid runs 3–7 `failed`. Log 1 contained the original event, the full approved shell call, hold/approval/resume timeline, the same goose session before and after resume, and the final `SELFTEST DONE`. Log 2 showed the first approved call and the changed call that was held and rejected. The stopped-daemon notice agreed with the selftest having ended. Intentional goose interruption messages remained visible in the history of the successful run, as expected for stopping at a hold.

## Explicit goose diagnostics and CLI validation

The following commands were run for **both** `selftest` and `hostile-suite`:

| Argument / environment | Output or behavior | Exit |
|---|---|---:|
| `--goose /work/not-a-goose` | Names that exact path; `No such file or directory (os error 2)` | 1 |
| Same nonexistent path with `--keep` | Same error; no temporary folder made | 1 |
| `--goose /work/nonexecutable-goose` | Names the attempted `--version` command; `Permission denied (os error 13)`; says an explicit working binary is needed | 1 |
| `--goose /work/failing-goose` | Names the attempted `--version` command and `exit status: 42`; says an explicit working binary is needed | 1 |
| Same failing executable with `--keep` | Same error, followed by the kept temporary-folder path | 1 |
| Default goose, with `PATH=/usr/bin:/bin` | Names `goose --version`, says it cannot be found, and gives installation / `--goose` guidance | 1 |
| `-c /work/not-a-config` before the subcommand | Explains that these commands make their own config | 2 |

There was no fallback from an explicitly unusable path to the working goose on PATH. After the failure checks without `--keep`, only the two directories explicitly kept for the failing executable remained. An independent empty temporary root stayed empty after the default-goose and nonexistent-explicit-path checks.

The two test executables were made with:

```sh
printf '#!/bin/sh\nexit 0\n' > /work/nonexecutable-goose
chmod 644 /work/nonexecutable-goose
printf '#!/bin/sh\nprintf "deliberate goose diagnostic on stderr\\n" >&2\nexit 42\n' > /work/failing-goose
chmod 755 /work/failing-goose
```

The valid explicit path was also exercised:

```sh
goose-triggers selftest --goose /usr/local/bin/goose
GOOSE_TRIGGERS_CONFIG=/work/absent.yaml goose-triggers selftest --goose /usr/local/bin/goose
```

Each exited 0 with 33 checks. The second confirmed that an invalid `GOOSE_TRIGGERS_CONFIG` is ignored by selftest. Neither used `--keep`; their temporary folders were removed. There were three complete selftest invocations in total, all passing.

For whitespace checks, these six commands were executed under bash:

```sh
goose-triggers hostile-suite --provider '   ' --model test
goose-triggers hostile-suite --provider ollama --model '   '
goose-triggers hostile-suite --provider $'\t\n' --model test
goose-triggers hostile-suite --provider ollama --model $'\t\n'
goose-triggers hostile-suite --provider '' --model test
goose-triggers hostile-suite --provider ollama --model ''
```

Every command exited 2 and named the offending `--provider <NAME>` or `--model <NAME>`, with `must contain a non-whitespace character`. A padded nonblank pair was also tested:

```sh
goose-triggers hostile-suite --goose /work/failing-goose --provider ' ollama ' --model ' local-test '
```

It passed local argument validation and stopped at the deliberately failing binary, exit 1. This establishes the non-whitespace validation behavior; it does not establish that goose accepts padded provider/model names, or that they are trimmed.

Additional checks:

| Command / condition | Observed result |
|---|---|
| `hostile-suite --provider ollama` | Exit 2, missing `--model` |
| `hostile-suite --model test` | Exit 2, missing `--provider` |
| `hostile-suite --provider codex --model test` | Exit 1, explains why CLI-agent providers cannot be held |
| `hostile-suite --provider ollama --model test --approvals 11` | Exit 2, allowed range `0..=10` |
| `-c /work/absent.yaml status` | Exit 1, named config cannot be read |
| `-c /work/empty.yaml log 1` | Exit 1, named config is empty |
| Root, selftest, and hostile-suite `--help` | Exit 0, options displayed |

## Downloaded example config and events

The README's example download commands were executed unchanged:

```sh
mkdir -p ~/.config/goose-triggers/recipes ~/Inbox
cd ~/.config/goose-triggers
base=https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.7/examples
curl -fsSL -o config.yaml "$base/config.yaml"
curl -fsSL -o recipes/file-note.yaml "$base/recipes/file-note.yaml"
curl -fsSL -o recipes/form-entry.yaml "$base/recipes/form-entry.yaml"
(umask 077; head -c 24 /dev/urandom | base64 > webhook-secret)
export GOOSE_TRIGGERS_WEBHOOK_SECRET="$(cat ~/.config/goose-triggers/webhook-secret)"
```

For this additional source/diagnostic check, `GOOSE_PATH_ROOT=/work/example-goose` isolated goose's settings and `GOOSE_PROVIDER=goose-triggers-install-no-provider` deliberately named an unavailable provider. No model was configured. `goose-triggers run` started with the downloaded config, and `GET /health` succeeded. While it ran:

```sh
curl -sS -w '\n%{http_code}\n' -X POST http://127.0.0.1:8787/hooks/form-entry \
  -d '{"name":"Test Person","email":"test@example.com"}'
printf '%%PDF-1.4\nlocal install test only\n' > ~/Inbox/install-test.pdf
sleep 2
curl -sS -w '\n%{http_code}\n' -X POST http://127.0.0.1:8787/hooks/form-entry \
  -H "Authorization: Bearer $GOOSE_TRIGGERS_WEBHOOK_SECRET" \
  -d '{"name":"Test Person","email":"test@example.com"}'
sleep 2
goose-triggers status --all
goose-triggers log 1
goose-triggers log 2
goose-triggers approve 1 --yes
goose-triggers reject 2
```

The curl commands above show the same requests with response bodies printed directly; the test saved bodies to files using `-o` and printed them afterward. The first POST returned 401 and `{"error":"missing or wrong shared secret"}`. The authenticated POST returned 202 and `{"event":2,"run":2}`. The folder event was run 1 (`inbox-notes`); the webhook was run 2 (`form-entry`). The test PDF was only a filename/event fixture, not a document-processing test.

Both runs then failed with goose's actual reason:

```text
goose exited with exit status: 1: error: No model configured. Run 'goose configure' first
```

Status and log exited 0 and showed the source fields, failed state, goose session, and reason. `approve 1 --yes` and `reject 2` each exited 1 with `run N is failed, not held`. These are expected refusals of terminal runs. Successful hold/approve/reject behavior was exercised by the scripted selftests, not by these unconfigured example runs. The daemon stopped cleanly on SIGTERM.

## Finding

**Low severity, diagnostic usability: failed explicit goose version checks omit stderr.**

Both commands below correctly refuse the executable and exit 1:

```sh
goose-triggers selftest --goose /work/failing-goose
goose-triggers hostile-suite --goose /work/failing-goose
```

Each prints:

```text
goose-triggers: `/work/failing-goose --version` failed (exit status: 42): the path given with --goose must be a working goose binary
```

The executable's `deliberate goose diagnostic on stderr` is omitted. The path and exit code are useful and correct, but a genuine startup failure's own reason could help explain a missing library or similar environment problem. Including a bounded stderr excerpt would improve troubleshooting. This is a cosmetic/usability observation, not an install or execution blocker. No other cosmetic issue was observed.

## Limits and other failures

- No real-model provider check or real-model hostile suite ran. No Ollama server/model or model credentials were supplied. The README's configured-provider greeting and successful processing of the downloaded example recipes therefore remain unverified. No paid model call was made.
- Source installation, `cargo test`, and rebuilding the binary were not attempted: this was a published-asset test without implementation access. Only Debian 12 x86-64 with goose 1.57.0 was tested; compatibility with goose's latest release or other platforms was not established.
- An initial test-harness prerequisite check discovered that the clean image lacked Python 3 and stopped before any product command ran. The unnecessary prerequisite was removed; the install and product tests used the image's shell utilities. This was a harness setup issue, not a goose-triggers failure.
- The nonzero exits listed above were intentional invalid-input or unconfigured-environment checks. The selftest's failed/rejected runs were also intentional test cases. There was no unexpected functional failure in the checks performed.

## Evidence

The companion scripts are `install-test.sh`, `diagnostics.sh`, `examples-test.sh`, and `cleanup-check.sh`. They use `/work` as the mounted test directory. They do not require source code. The installed binary is in `installed/`; downloads and `release.json` retain the asset evidence.

Full output is in `logs/install.txt`, `logs/selftest.txt`, `logs/hostile-suite.txt`, `logs/diagnostics-transcript.txt`, the individual files in `logs/diagnostics/`, `logs/examples/`, and `logs/cleanup/transcript.txt`. The original selftest and hostile-suite temporary folders were retained under `tmp/`; the suite's own `results.md` contains its complete case table. These files preserve the exact commands, error output, and generated test state used for this verdict.
