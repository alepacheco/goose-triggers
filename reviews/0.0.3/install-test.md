# goose-triggers 0.0.3: clean-install test by an automated tester

Verdict: PASS

goose-triggers v0.0.3, tested as a new user from the README at the tag, in clean containers (Debian bookworm-slim, goose 1.57.0, no provider, no API key, HOME=/tmp). Its scripts and raw outputs are not published.

## Steps and results

1. Install (README Install, verbatim): sha256 OK, `goose-triggers --version` = 0.0.3, tarball holds README, LICENSE, examples. PASS.
2. `selftest`: 29 checks, ~12 s, exit 0; `--goose /usr/local/bin/goose` passes; `--goose /nonexistent/goose` clear error. PASS.
3. `selftest --keep`: "its files: <folder>"; `-c <folder>/config.yaml status --all` and `log 1` work as written (8 runs). PASS (finding 3).
4. `hostile-suite`: 16 events; no holds 13/13 canaries; example holds 0/13 carried out, 13 held at their first injected call; structure check 3/3 both passes; offered tools printed; exit 0; --keep keeps results.md. PASS.
5. `hostile-suite --holds shell,write,edit`: W9 and F7 carried out through read_image, flagged "MORE THAN READS", exit 1, as the README's pass rule predicts. `--holds '*'` and a longer list pass.
6. Use section verbatim: steps 1-5 behave as documented without a provider (runs fail with goose's reason, sessions named, approve/reject on failed runs refused). PASS.
7. Folder source rules (baseline, hidden, case, subfolders, files dropped while stopped), SIGTERM/SIGINT stop cleanly. PASS.
8. Webhook answers: 202, 401, 404, 405, 413, 411, 400 (body cut short, bad Content-Length), 408 after ~10 s (stalled body/headers, trickle), GET and HEAD /health 200. PASS.
9. status window (20 + held; --all), `status | head -3`, `log <run> | head` quiet (exit 141, finding 6). PASS.
10. Held runs (from an interrupted selftest): approve off a terminal needs --yes; on a terminal n/EOF declines, y approves; --yes with stdout closed approves; reject withdraws an approval not resumed and `run` does not resume it. PASS.
11. Start checks: missing goose path, /bin/false, a directory; read_image start warning; CLI-agent providers, other plugins, bad names, short secret, unset secret_env, missing recipe/folder, unknown keys, duplicates, max_concurrent_runs 0 refused with clear messages. PASS.

## Findings

1. GOOSE_TRIGGERS_CONFIG pointing at a missing file (a `.yml` typo) or empty: status, log, approve and reject act on the default state folder (~/.local/share/goose-triggers) with exit 0; approve/reject there worked. `-c` with the same path errors, and so does `run`. (Confirmed in code: Config::load_for_state falls back to defaults when the default path, which is the env var's, does not exist.)
2. Ctrl-C during selftest or hostile-suite leaves /tmp/goose-triggers-selftest-* / -hostile-* behind (~1.6 MB) without --keep.
3. A kept selftest folder has no run left held: approve/reject cannot be tried there; the README gives no way to get a held run without a provider.
4. A CLI-agent provider set in goose's own config is caught only when an event's run starts (each fails with a clear reason); set in a rule or GOOSE_PROVIDER, `run` refuses to start. README does not say when.
5. `hostile-suite --holds shell,write,edit` exits 1 as designed; the README could say a weaker --holds set is expected to fail.
6. `status | head -3` exits 141 (SIGPIPE): quiet, but fails under `set -o pipefail`; not in the README.
7. Declining at the approve prompt exits 0, like approving.
8. Cosmetic: start line "goose goose (1.57.0)"; reasons end ".; see goose-triggers log N"; goose's provider error shows literal "\n\n"; `--holds '*'` prints an empty "Not held" list; suite table lists F7 before F6.
9. Undocumented start refusals (missing rule folder, unset secret_env, max_concurrent_runs 0); messages clear.
10. Forced case only: resuming an approved run in an interrupted selftest's state with GOOSE_PATH_ROOT set by hand made goose call the real provider endpoint (401); the run was marked failed.
11. Not checkable without a provider: `goose session --resume --session-id <id>` (goose: "Error: not connected").
