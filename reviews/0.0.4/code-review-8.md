# goose-triggers 0.0.4: automated code review, round 8 of 10

Verdict: NOT FIT

This review covers `git diff b32bdbe..HEAD` (commit ac035e8), with the code around it. Nothing in the repository was built, tested or run. It checks findings 1 and 2 of the previous round (`reviews/0.0.4/code-review-7.md`) against ac035e8. Findings 3 and 4 there (nits) are left as they are, as the commit says. Read: `src/hostile.rs` (`Lexed`, `lex`, `expansion`, `simple_commands`, `is_date`, `date_arg`, `within_shell`, the start of `simple_within`, and the tests `lexes_a_command_as_bash_reads_it` and `what_bash_would_run_is_never_approved_as_a_step`); `src/runner.rs` (`effective_provider`, `goose_default_provider`, `active_provider_env`, `provider_in_goose_configs`, `check_provider`, `prepare_working_dir`, `goose_command`, the start of `run_goose`, and the provider tests); the start check in `src/daemon.rs`; `is_cli_agent_provider` in `src/util.rs`; README.md:30; and `reviews/0.0.4/code-review-7.md`. goose was read at tag v1.57.0 with `git show`: `crates/goose/src/config/providers.rs`, `base.rs` and `migrations.rs` (where the provider comes from); `crates/goose-cli/src/session/builder.rs` (`build_session`, `resolve_provider_and_model`, `validate_provider_override_context`, `is_provider_unavailable_error`); `crates/goose-cli/src/session/mod.rs` (`session_provider`); `crates/goose/src/agents/provider_manager.rs`; `crates/goose/src/providers/provider_registry.rs`, `anthropic_def.rs`, `openai_def.rs`, `codex.rs`, `cursor_agent.rs`, `claude_code.rs`, `gemini_cli.rs`; and `crates/goose/src/acp/provider.rs`. Each case below was traced by hand through the code of goose-triggers and of goose; goose was not run. Each claim about how bash reads a command was checked by running bash 5.3 in an empty temporary folder outside the repository, with `touch` of a file there.

## Findings

1. **major**: a resumed run can go on with goose's default provider, and goose-triggers does not check that provider when the recipe names one. The default can be a CLI-agent provider, whose own tools no hold sees. This is older than ac035e8; this review found it while checking goose's provider order. Files: src/runner.rs:94-105 (`effective_provider`), 167-177 (`check_provider`), 232-242 (a resume passes `--provider` only when the rule names one); README.md:30; goose `crates/goose-cli/src/session/builder.rs:673-683` (the session's provider), 743-799 (the fallback), 263-287 (`validate_provider_override_context`), 906-911 (`is_provider_unavailable_error`).
   - **What is wrong.**
     - goose-triggers resumes an approved run with `goose run --resume --session-id <id> --text ...`. It passes no recipe, and `--provider` only when the rule names one.
     - goose takes the session's saved provider. When that provider cannot be built, the run is a resume, and no `--provider` was given, goose looks at the error. If it says "is not set", "not configured" or "Configuration value not found", goose prints a warning and goes on with its default provider (`get_goose_provider`).
     - A missing API key gives that error. For example, `anthropic_def.rs:49` reads `ANTHROPIC_API_KEY` with `get_secret`, which fails with "Configuration value not found".
     - goose then refuses the resume only when the new provider manages its own context: `claude-code`, `gemini-cli` and the ACP providers do. `codex` and `cursor-agent` do not, so goose goes on with them. `cursor-agent` runs with `--force`. `codex` runs with `--yolo`, since goose-triggers sets `GOOSE_MODE=auto`. Both run their own tools.
     - goose-triggers' check of the resumed run reads the rule's provider, then the recipe's, then the default. With the recipe naming one, the default is never looked at.
     - README.md:30 says `run` refuses to start when "the rule, its recipe, `GOOSE_PROVIDER`, `ACTIVE_PROVIDER` or goose's own config names one". The code takes only the first of these that names a provider.
   - **The case.**
     - goose's own config names `cursor-agent` and a model, for the user's own goose sessions.
     - A rule with holds names no provider. Its recipe sets `goose_provider: anthropic`, since goose-triggers refuses `cursor-agent`.
     - `goose-triggers run` starts with `ANTHROPIC_API_KEY` in its environment (the key is not in goose's own secret store). A run is held.
     - goose-triggers is stopped, and started again from a shell that does not set the key. Its start check passes: the recipe names `anthropic`.
     - The user approves the held call. goose-triggers checks the run again: `anthropic`, so it resumes the run.
     - goose cannot build `anthropic`, warns, and goes on with `cursor-agent`. The model is told to make the approved call again, and `cursor-agent` makes it with its own tools. That call and every later one run with no hold.
     - New runs in that goose-triggers fail for want of the key, but the resumed run has already run.
     - Round 7's "Checked and found sound" says a resumed run's session started on a provider that was checked when it started. With this fallback, that is not enough.
   - **How likely.** It needs a CLI-agent default that goose does not refuse on resume (`cursor-agent` or `codex`), a rule that names no provider while its recipe names an API provider, and a key that is there when the run starts and missing when it resumes. No attacker is needed: an ordinary restart from another shell is enough. It is rated major because the CLI-agent provider check is one of the holds users rely on, and here it passes while goose runs a CLI agent.
   - **Fix.**
     - When a rule with holds names no provider, check goose's default provider as well as the recipe's, at `run`'s start and at each run's start, as README.md:30 already reads. A rule that names its provider is passed to goose as `--provider`, and goose does not fall back then.
     - Or, at least, check the default provider before a resume.
     - Add a test with a recipe that names an API provider and a default that names `cursor-agent`.

2. **minor**: after it passes over an `ACTIVE_PROVIDER` value that is not a name, `goose_default_provider` reads `active_provider` from goose's config files. goose 1.57.0 does not: it goes straight to `GOOSE_PROVIDER`. Files: src/runner.rs:112-135 (`goose_default_provider`), 137-150 (`active_provider_env`), 152-165 (`provider_in_goose_configs`); goose `crates/goose/src/config/providers.rs:65-73` (`get_active_provider`), `crates/goose/src/config/base.rs:773-778` (`get_param`) and 709-735 (`parse_env_value`).
   - **What is wrong.**
     - goose's `get_param` returns as soon as the variable is set. When the value does not read as a string, it fails there. It does not look at `active_provider` in the files.
     - `get_active_provider` then takes `GOOSE_PROVIDER` from the merged files.
     - goose-triggers, having passed the value over, calls `provider_in_goose_configs`, which takes the files' `active_provider` first.
   - **The case.** `/etc/goose/config.yaml` holds `active_provider: openai`. The user's `config.yaml` holds `GOOSE_PROVIDER: claude-code` (the layering of the test at src/runner.rs:935). goose-triggers runs with `ACTIVE_PROVIDER=1`.
     - goose-triggers takes the provider to be `openai`. `run` starts, and each run starts.
     - goose runs `claude-code`, whose own tools no hook sees.
     - One file holding both keys gives the same result.
     - This is not a regression: v0.0.3 and b32bdbe let it through too. ac035e8 does close round 7's case (files that set only `GOOSE_PROVIDER`).
   - **Smaller differences, where no hold is lost.**
     - Rust's `f64` parse accepts `inf`, `infinity` and `nan`, in any case and with a sign. goose keeps these as text, since `Number::from_f64` refuses a number that is not finite (base.rs:727-731). goose-triggers passes them over.
     - goose also takes an empty value as a name. goose-triggers passes it over (the test at src/runner.rs:903 asserts this).
     - In both cases goose stops on an unknown provider and runs nothing.
     - The files have the same pattern. This is older than this change. `provider_in_goose_configs` skips a file whose `active_provider` is not a string (`active_provider:` left empty is YAML null) and takes an earlier file's. goose merges the files first, so the later null wins. Its `get_param` then fails, and it goes to `GOOSE_PROVIDER`.
   - **How likely.** Very low. It needs `ACTIVE_PROVIDER` set to a value that names no provider, and config that sets both keys to different providers. Round 7 rated the wider case minor, and this one is narrower.
   - **Fix.**
     - Read the variable into three outcomes: unset, a name, or not a name. When it is not a name, take only `GOOSE_PROVIDER` from the files.
     - In `provider_in_goose_configs`, take each key from the last file that sets it, whatever its value. Go on to the next key when that value is not a string.
     - Pass over a number only when it is finite.
     - Test the order with the variable's value given as an argument.
     - The fix of finding 1, checking every provider goose could use, would also cover this.

3. **minor**: the two tests added for round 7's finding 1 do not test the fix. Files: src/hostile.rs:2985-2987.
   - **What is wrong.**
     - Both commands are refused before `is_date` matters. After the substitution, the lexer reads a `)` operator outside quotes and a quote left open, so `simple_commands` returns none. bash refuses both as a syntax error and runs nothing (checked).
     - The comment says bash and the lexer end the substitution in different places. For these two they end it at the same `)`. In each, the lone quote is inside a closed string of the other kind.
     - Each substitution text also has an odd count of one quote. b32bdbe's even-count check refused them too.
     - So the suite at ac035e8 passes with b32bdbe's `is_date` put back. No test holds round 7's finding 1 closed. The older tests at src/hostile.rs:2969 and 2979 cover only an open quote of one kind, which b32bdbe's check also refused.
   - **The case.** Put b32bdbe's two count checks back in place of the two new checks in `is_date`. Every test still passes. Yet the first command below is approved again, and bash runs its `touch` (checked).
   - **Fix.** Add round 7's commands as refused. bash runs the `touch` in each (checked):
     ```
     echo "$(date "+'" '+)' ; touch ran ; )" >> entries.csv
     echo $(date "+'" '+)' ; touch ran ; ) Ada
     cat >> entries.csv <<EOF
     $(date "+'" '+)' ; touch ran ; )
     EOF
     ```
     Add the last two with the quotes swapped (`$(date '+"' "+)" ; touch ran ; )`). Assert that `is_date` is false for `date "+'" '+`. Correct or drop the comment at 2985.

## Checked and found sound

- **Round 7's finding 1 is fixed** (`is_date`, src/hostile.rs:1012-1023; `lex`, 893).
  - `lex` now records whether it ended inside a quote. `is_date` refuses that, and refuses text with both kinds of quote.
  - For `$(`, `expansion` returns the text up to the first `)`. `is_date` refuses `(`, so the depth never rises.
  - `is_date` allows only letters, digits, space, tab and `+%-:_./,'"`. So there is no `\`, `$`, backtick, `#`, parenthesis, brace, glob, `~`, `=`, operator or line break.
  - With one kind of quote and none of those, `lex` and bash open and close quotes at the same characters. So when `lex` ends outside a quote, bash is outside one at that `)` and ends the substitution there. bash then runs `date` with those words only.
  - This holds in a plain word, in double quotes and in an unquoted here-document. bash parses the substitution's text afresh in each.
  - The open-quote check alone closes the gap. Refusing both kinds of quote only narrows what is approved.
  - Round 7's forms (`date "+'" '+` and `date '+"' "+`) are refused by both checks.
  - A `)` inside a closed quote of one kind (`$(date "+%F" "+)")`) is refused. `lex` stops at the first `)` with `"+` open. bash ends the substitution at the second `)` (checked), so the two do disagree there.
  - What is still approved works as a careful user expects. `$(date "+%F")`, `$(date '+%Y-%m-%d %H:%M')` and `$(date "+%F")` in an unquoted here-document print the date (checked).
  - `date_arg` allows only a `+` format or a fixed option. There is no `-s`, `-d`, `-f`, and no operand that sets the clock.
  - Backticks are read as before: bash ends them at the next backtick, as `lex` does.
- **`active_provider_env`** (finding 2 aside).
  - It reads a value as `parse_env_value` does. It tries JSON on the value as it is, then `true` or `false` in any case on the trimmed value, then a number, else the text untrimmed. The earlier trim is gone, as goose keeps the text untrimmed.
  - A JSON string is the name. Any other JSON value, a boolean or a finite number is passed over, where goose's `get_param::<String>` fails.
  - Every text that parses as an `i64` also parses as an `f64`, so one check covers goose's two.
  - `GOOSE_PROVIDER` still comes first, as in goose.
  - Both callers use `goose_default_provider`: `effective_provider` (each run's check) and `run`'s start check (src/daemon.rs:186-198).
  - The added test covers a name, a quoted name, `1`, `true`, `null`, `2.5` and the empty forms.
- **A fresh run** (not a resume) does not fall back: goose returns the error, and the run fails. A rule that names its provider is passed as `--provider`, so its resumed runs do not fall back either. `prepare_working_dir` checks the provider for a resume as for a start (src/runner.rs:439).
- **Holds.** ac035e8 changes only `src/hostile.rs` and the provider read in `src/runner.rs`. The hook, the store, `approve`, `reject` and `check_provider` itself are not touched.
- **reviews/0.0.4/code-review-7.md** names no paths of a build machine.
