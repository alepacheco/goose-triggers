# goose-triggers 0.0.7: independent code review

**Verdict: FIT**

Reviewed on 2026-11-09. No correctness findings requiring changes were found in this release diff. Required fixes: none.

## Exact scope

- Candidate: `c219c3b58761bcb842e97891e25eda425134ef08`.
- Base: `v0.0.6`, commit `19146fca684655cc808575fdbade776fb96d26c3` (annotated tag object `2634b7ee8968b77905ca046fff02c0e3d0fab0f3`).
- Range: `git diff v0.0.6..c219c3b`.
- Read the repository README and the surrounding CLI, goose-version probe, selftest/suite preflight, daemon and hook/plugin code, together with the changed tests and release-install workflow.
- Sponsor references: goose's `documentation/docs/guides/context-engineering/plugins.md` and `hooks.md` at `609d3432d397e0a35b4c6b95afbd3d99fe694811`; hook implementation checked at the released `v1.57.0` commit `30bba95105e59bc3e7df624cf6b4d433690cfe12`.

This was a fresh review by a reviewer that did not implement the candidate. No author task notes or previous code-review verdicts were used. The archived 0.0.6 installation report is a historical artifact included in the diff; its claims were not used as validation of 0.0.7. No implementation files were changed, and no commit, push or release was made.

## Findings

None. No high, medium or low severity correctness finding was identified within the reviewed change.

## Deliverables checked

| Claim | Evidence and result |
|---|---|
| The README explains where the kept-folder path appears. | The selftest paragraph now says it “prints its path after the reason.” Both commands actually emit the failure on the first stderr line and the kept path on the last line. PASS. |
| An explicit `--goose` failure diagnoses the selected binary. | `selftest::goose_hint` is chosen before resolving the option, and both preflight paths pass it to `util::goose_version`. Permission failures name the binary and OS error; unsuccessful probes name the binary and exit status. They say the selected path must be a working goose binary, with no instruction to pass `--goose` again. Missing absolute paths name that path and the filesystem error before a temporary folder is made. PASS. |
| Default PATH failures retain useful guidance. | Both commands name the failed `goose --version` probe and include the installation URL and `pass --goose <path>` alternative. The daemon's caller also retains installation/config guidance after the common helper stops adding it itself. PASS. |
| CLI provider/model names cannot be trim-empty. | Both options use `non_blank_name`, which rejects `value.trim().is_empty()` and leaves valid input intact. Empty strings, spaces, tab/newline and Unicode em-space were rejected for each option with exit 2. Existing provider/model pairing and range checks still pass. PASS. |
| Current version/install references are 0.0.7. | Cargo manifest/lockfile, binary output, README release assets and extraction/install paths, source-install tag, downloaded examples and workflow example agree. Historical results retain their original versions. PASS. |

## Regression assessment

The hook, plugin command, approval store and runner logic are unchanged in this range. The plugin manifest obtains its version from Cargo, so it advances to 0.0.7 without changing its decisions. The existing plugin reads stdin before allowing sessions outside its own runs; its `PreToolUse` block response and `on_failure: block` configuration agree with goose 1.57.0's documented and implemented protocol.

The changes to `hostile.rs` only select and pass the diagnostic hint during preflight. They do not change cases, canaries, call classification, allowed task steps, approval limits or pass/fail judging. The README's statement that the earlier model results use the same judging remains supported by this diff.

## Executed verification

All checks ran in an isolated Linux container with Rust 1.99.1, locked dependencies, read-only candidate source, a separate build directory and no network. No API credentials or model calls were needed. The container exited 0.

Five repository tests passed:

```text
tests::values_out_of_range_are_command_line_errors
util::tests::a_missing_goose_is_named_with_the_hint
a_selftest_or_suite_that_cannot_start_keeps_its_folder_only_with_keep
goose_version_names_an_explicit_binary_that_exits_unsuccessfully
the_readme_installs_this_version
```

An additional 25 independent CLI checks passed:

- For each of `selftest` and `hostile-suite`, with and without `--keep`: a non-executable selected file, a selected program exiting 7, a missing absolute selected path, and no goose on PATH (16 checks). Assertions covered exit status, path/reason/hint text, stderr line order, and actual folder retention/removal.
- For each of `--provider` and `--model`: empty, space-only, tab/newline-only and Unicode em-space-only names were rejected with exit 2 (8 checks).
- `--version` printed exactly `goose-triggers 0.0.7` (1 check).

The added repository tests prove the explicit-path hint and rejection of blank names. Their scope has limits: `tests/keep.rs` checks message presence rather than ordering, and `tests/readme.rs` checks installation text rather than downloading release assets. The independent CLI checks above cover ordering and PATH guidance. Published-asset installation, the complete selftest/hostile-event runs, the full validation script and minimum-supported-Rust validation were not rerun as part of this focused review; those remain separate release checks. FIT is the verdict for the reviewed code change, not a claim that unpublished release assets were installed.
