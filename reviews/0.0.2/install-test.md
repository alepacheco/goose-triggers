# goose-triggers 0.0.2: clean-install test by an automated tester

Verdict: PASS

goose-triggers v0.0.2, tested on 2026-11-06 as a new user would. Clean containers (Debian bookworm-slim, goose 1.57.0, HOME=/tmp, no provider, no API key), following only the README at tag v0.0.2 (identical to the README in the tarball). Scripts and outputs: install.sh, s1.sh..s4.sh, s1.out..s4.out (s1 install + selftest, s2 Use section + webhook/folder, s3 config reference, s4 follow-ups and held runs).

## Steps

1. Install (README "Install", verbatim): checksum OK, `goose-triggers --version` = `goose-triggers 0.0.2`, static ELF; tarball holds goose-triggers, LICENSE, README.md, examples/ (config identical to the one Use step 2 downloads).
2. `goose-triggers selftest`: passes with no API key in about 9 s, `selftest passed: 24 checks`, exit 0; its ok lines cover the README's 9 listed checks in order (folder event, hold, 401/202, restart, approve and done, differing call held again and rejected, YAML injection refused, goose failure reported in goose's own words without event values, broken value / missing parameter / NUL byte refused before goose runs). No goose folders under HOME afterwards; temp folder removed on pass. `--keep` + `-c <folder>/config.yaml status --all` lists 7 runs with reasons; `log 1` shows event, held call, timeline, goose's messages. `--goose /nonexistent/goose` gives a clear error.
3. Use section: `goose run --text ...` fails with `error: No provider configured` (expected, no provider). Steps 2-4 work verbatim (secret file mode 600). Both runs fail at once; `status` shows `goose exited with exit status: 1: error: No provider configured. Run 'goose configure' first.; see …`; `log` shows `goose printed: error: No provider configured. ...`. The reason holds neither the posted name/email nor goose's banner (grep-checked).
4. Other claims: webhook 202/401/404/405/413/411/health as listed (65536 bytes accepted, 65537 refused); 408 for unfinished headers or a trickling body, but 400 for a stalled body (finding 1). Event bodies: non-JSON, arrays, empty, missing name all give 202 then a failed run naming only the recipe's parameter; numbers/booleans become text; JSON `body` field overrides; YAML injection refused; NUL byte gets goose-triggers' own reason. Folder rules: pre-existing, .txt, hidden, subfolder and UPPER.PDF files ignored; files dropped while stopped run at start; patterns case-sensitive as documented. SIGTERM/SIGINT stop cleanly. Commands: status 20 of 25 / --all 25; unknown run ids answered; approve/reject on non-held runs refused. Held runs (stopped selftest --keep): status shows waits-on and hint; approve off a terminal approves and says the run resumes at next start; on a terminal it asks [y/N]. Config reference: -c, GOOSE_TRIGGERS_CONFIG, relative paths, webhook.secret, listen port work; goose receives --provider/--model/--max-turns, --quiet --output-format stream-json, GOOSE_MODE=auto, GOOSE_SLASH_COMMANDS_ENABLED=false, recipe rendered twice; with provider openai and no key the reason is goose's own 401 error; JSON recipe works; value filling settings.goose_model refused; other plugin in the working folder refused. Bad configs refused at start with clear messages; CLI-agent providers with holds refused. Hold warning as described.

## Findings

1. A stalled body gets 400 `{"error":"could not read the body"}` after 10 s, not the 408 the README's webhook paragraph promises ("408 for a request that does not arrive whole within 10 seconds"); 408 only for unfinished headers or a body still trickling at 10 s.
2. `goose-triggers status | head -3` and `log N | head -20` panic: `failed printing to stdout: Broken pipe (os error 32)`. Expected a quiet exit.
3. `goose: /nonexistent/goose` is not checked at start; each event then fails with `goose cannot read the recipe, even with placeholder values (running /nonexistent/goose: No such file or directory)`, which reads as a recipe problem.
4. `reject` on an approved run that has not resumed answers `run 2 is approved, not held`: an approval cannot be withdrawn while `goose-triggers run` is stopped.
5. For runs goose failed (no provider), status/log show session `-` though `goose session list` lists the session goose created.
6. Use step 5's examples use run 3 (`log 3`, `approve 3`, `reject 3`); after steps 1-4 a new user has runs 1 and 2.
7. The `--keep` paragraph's `log 1` needs `-c <its folder>/config.yaml` too; it does not say the folder is printed as `its files: <path>`.
8. (cosmetic) status's DETAIL column cuts `; see goose-triggers log N` to `see …`.
9. (cosmetic) The selftest config holds only `[shell]`, against the README's advice; its daemon.log carries the hold warning on each start.
10. (cosmetic) `HEAD /health` answers 404 (README lists only GET).
11. (note) Off a terminal `approve <run>` approves without asking, even without `--yes` (as the README says); requiring `--yes` off a terminal would be safer.
