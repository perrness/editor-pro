# Delivery validation — 2026-10-14

Entries below retain the evidence from each delivery. The latest
[M5 real-server acceptance follow-up](#m5-real-server-acceptance-follow-up)
supersedes earlier missing-server blockers and test counts.

## Foundation revalidation before Milestone 2

Starting commit: `864f6ab`. Linux x86_64, Rust/Cargo 1.99.0, Python 3,
OpenJDK 25.0.4.1, Maven 3.8.7. Stable Rust, rustfmt and Clippy were installed
from the official Rust distribution. The persistent toolchain is under
`/opt/editor-pro-rust/stable`, exposed in `/usr/local/bin`; the build cache
used for these checks is `/tmp/editor-pro-cargo`.

| Observed check | Result |
| --- | --- |
| Cargo resolution and fetch | Passed; generated Cargo.lock, reviewed direct dependencies, CI now uses --locked. |
| cargo fmt / cargo fmt --check | Passed after applying rustfmt to the uncompiled foundation. |
| cargo clippy --locked --all-targets -- -D warnings | Passed. |
| cargo test --locked --all-targets | All 34 Rust tests passed. |
| cargo build --locked | Passed. |
| Python probe unittest discovery | All 7 tests passed. These exercise the harness and a test double. |
| python3 tools/terminal_smoke.py | Passed after fixing the PTY harness to answer the terminal cursor-position query. Dirty quit now explicitly checks that the process stays alive. |
| Isolated Kotlin probe | Blocked, exit 2: Kotlin LSP executable is not installed or on PATH. Report: /tmp/editor-pro-kotlin-m2-result.json. |

The initial PTY run failed because a raw PTY does not emulate terminal DSR
responses; no compiler or editing-test failures were found. The harness now
supplies the same cursor-position response a terminal emulator supplies.

Rust downloads succeeded with authorized network access. The previous claim
that downloads are impossible no longer applies. JDK 25 and Maven exist, but
no standalone Kotlin server archive/launcher is installed; no real Kotlin
capabilities or functional behavior were observed. See
[LANGUAGE_SERVERS.md](docs/LANGUAGE_SERVERS.md).

macOS, hosted CI, and interactive visual inspection in a physical terminal have
not been run here. These remain platform/visual validation limits.

## M2 slice 1a — Rust highlighting

Formatting and Clippy passed; all 38 Rust tests and the PTY smoke test passed.
Regression checks compare incremental trees/captures against fresh parses after
Unicode/CRLF edits and undo/redo, verify multiline viewport captures, and verify
unknown/oversized-file fallback. Cursor movement reuses the existing tree.

## M2 slice 1b — Remaining languages

Formatting, Clippy, all 41 Rust tests and the PTY smoke test passed. Fixtures
exercise Java, Kotlin, C#, JSON, YAML, TOML and Markdown grammar/query loading,
semantic captures and incremental/fresh equivalence. Markdown inline ranges
exclude fenced code, including after edits. Kotlin uses the published
`tree-sitter-kotlin-sg` package of the fwcd grammar and its compatible upstream
queries. Unsupported query predicates trigger fallback rather than silently
matching everything. Language injections and semantic LSP tokens are not included.

## M2 slice 2 — Modal editing

Formatting, Clippy, all 60 Rust tests and the extended PTY smoke test passed.
The new checks cover multiplied counts, inclusive/exclusive and linewise
operators, Unicode Visual selections, CRLF and EOF behavior, nested/quoted text
objects, character/line registers, paste, find/repeat, and grouped change undo.
A 2,000-action mixed modal regression checks grapheme boundaries after every key.
The PTY test now exercises Visual yank/paste and change/undo/redo with CRLF text.

## M2 slice 3 — Search

Formatting, Clippy, all 68 Rust tests and the extended PTY smoke test passed.
Search tests cover forward/backward counts and wraparound, literal matching,
Unicode boundaries, cancellation, cache invalidation after edits/history,
Visual selections and narrow prompt rendering. Search is case-sensitive and
single-line, with at most 100,000 results; truncation is reported. `:noh` hides
matches and `n`/`N` restores them.

## M2 slice 4 — Multiple buffers

Formatting, Clippy, all 78 Rust tests and the extended PTY test passed. Checks
cover independent histories, dirty text/cursor/viewport preservation, path alias
deduplication, failed-open rollback, save-as identity collisions, inactive disk
conflicts, all-buffer quit protection, and a separate buffer-list view. The PTY
workflow edits two files, verifies refusal to quit with an inactive dirty buffer,
and saves both. `:e path` now retains the buffer being left behind.

## M2 slice 5 — Fuzzy file navigation

Formatting, Clippy, all 85 Rust tests and the extended PTY test passed. Nucleo
matches in its worker pool; a cancellable discovery thread uses ignore rules,
excludes VCS metadata and symlinks, and reports unreadable entries and a 100,000
file cap. Tests cover fuzzy ranking, Unicode/spaces, ignore files, stale-query
suppression, failed opens, cancellation, dirty-buffer preservation and tiny
terminals. The PTY test finds, opens and edits a second file through the picker.

## M2 slice 6 — Themes and statusline

Formatting, Clippy, all 88 Rust tests and the extended PTY test passed. Tokyo
Night, Catppuccin and Gruvbox-inspired palettes color syntax, selections, search,
modes and contextual views. Status rendering reports buffer identity/count,
dirty buffers, language/plain fallback, EOL/encoding, grapheme column, position
and pending operators, with compact layouts for narrow terminals. The PTY test
switches all three themes and verifies actual RGB output; its child environment
now explicitly models a truecolor terminal rather than inheriting NO_COLOR.

## Final hardening and regression coverage

All **92 Rust tests** pass after the six feature slices. Final hardening fixes
repeated `t`/`T` searches stopping at the adjacent prior target, putting an empty
line into an empty buffer, and repeated full-line segmentation for counted
horizontal motions/deletes. Each has a regression test. Existing mixed-action
tests continue checking grapheme boundaries after thousands of operations.

The first release performance run exposed a Markdown inline-parser timeout:
combining separate inline ranges in one parse could treat them as one paragraph.
The final implementation uses the upstream Markdown block/inline parser, with
Tree-sitter 0.26 as required by that API. A new regression checks separate
paragraph delimiters, table-cell emphasis, blockquotes, Unicode/CRLF edits and
incremental/fresh equivalence through undo/redo. The completed release workload
has no syntax fallback in any of the eight languages.

The PTY harness now completes **10 sessions**: UTF-8 editing/history/save, dirty
quit refusal, Visual/change with CRLF, search, multiple buffers, fuzzy file open,
three themes with actual RGB output, wide text with real resize events (including
a one-column terminal), bracketed paste containing command-looking text, and an
external-write conflict recovered with save-as. Every session checks saved bytes,
process lifetime, restored termios, alternate-screen exit and disabled bracketed
paste. Physical-terminal appearance and injected panic/error cleanup have not
been manually exercised; the latter retains the existing scope guard/panic hook.

## Final command results

All commands below were run on this Linux VM with Rust/Cargo 1.99.0. Cargo used
the fetched lockfile/cache in offline mode (`CARGO_HOME=/tmp/editor-pro-cargo`);
ordinary reproduction commands are shown without that environment-specific path.
The globally installed `cargo`/`rustc` resolve through `/usr/local/bin` to the
persistent toolchain, and both report 1.99.0. The manifest's minimum Rust 1.88 was
not separately tested.

| Command | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed, no warnings. |
| `cargo test --locked --all-targets` | 92 tests passed, none failed or ignored. |
| `cargo build --locked` | Passed. |
| `cargo build --locked --release --bin editor-pro` | Passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | All 7 tests passed; test-double results are not real server evidence. |
| `python3 tools/terminal_smoke.py` | All 10 Linux PTY sessions passed against the debug binary. |
| `cargo run --locked --release --example performance` | Completed all eight languages and the 10,000-file picker with assertions passing. |
| `target/release/editor-pro --help` and `--version` | Passed without a TTY; updated M2 usage and version 0.1.0 displayed. |
| `git diff --check` | Passed. |

No macOS run, hosted CI result, physical-terminal visual inspection, minimum-Rust
build, live Kotlin language-server result, or full real-world project benchmark
is claimed. No push was performed.

## Release performance smoke measurements

Reproduce with `cargo run --locked --release --example performance`. These are
one final run's Linux x86_64 measurements, not timing gates or general guarantees.
The example uses a 120×32 Ratatui TestBackend, repeats each fixture 1,000 times,
and samples 50 single-character edits at varying line ends, with undo between
samples. Edit timing includes insertion, commit, incremental parsing and drawing;
scroll timing measures drawing after moving the cursor. Initial timing includes
parsing/drawing after grammar/query setup. Loading, terminal output transport and
human-perceived input latency are not included.

| Language | Bytes / lines | Initial ms | Edit+draw p95 ms | Edit+draw max ms | Scroll+draw p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 51,000 / 3,001 | 6.31 | 0.90 | 1.05 | 0.35 |
| Java | 39,000 / 1,001 | 4.43 | 1.80 | 1.99 | 0.42 |
| Kotlin | 33,000 / 1,001 | 5.96 | 0.92 | 0.95 | 0.43 |
| C# | 39,000 / 1,001 | 6.32 | 0.99 | 1.24 | 0.40 |
| JSON | 28,017 / 1,004 | 3.66 | 0.77 | 0.99 | 0.33 |
| YAML | 22,000 / 2,001 | 5.08 | 1.47 | 1.53 | 0.24 |
| TOML | 24,000 / 2,001 | 3.75 | 1.93 | 2.17 | 0.22 |
| Markdown | 65,000 / 8,001 | 35.62 | 47.04 | 49.84 | 0.33 |

No language entered fallback during the final edits or undos. Markdown visits
many inline trees and remains the slowest case; typing on larger or more complex
Markdown can be noticeably slower. Above 2 MiB syntax is disabled. The parser's
cooperative cancellation budget does not promise a hard latency ceiling.

The picker discovered/ranked **10,000 files in 18.74 ms** and settled the fuzzy
query `file04200` in **1.08 ms**, selecting `file_04200.rs`. Fixture creation was
outside the timed region. Discovery and matching were polled at 1 ms intervals;
the actual editor uses a 16 ms event-poll interval while the picker is open.
Remote filesystems and heavily loaded machines will behave differently.

## Milestone 2 acceptance assessment

| Criterion | Evidence / status |
| --- | --- |
| Everyday editing and code reading | All six requested slices implemented. Tests and PTY sessions cover the documented workflow; subjective comfort still needs physical-terminal use. |
| Unicode/multiline selections and operators | Passing grapheme, emoji, combining-mark, nested-object, CRLF, history and mixed-action regressions; real PTY Visual/change/save workflow passes. |
| Responsive highlighting while typing/scrolling | Incremental/fresh equivalence and viewport tests pass for the supported grammar fixtures; the release workload completes with the timings above and no fallback. Large Markdown remains a documented limit. |
| Buffer switching preserves edits and location | Independent histories, cursor/viewport, path aliases, inactive conflicts, failed opens and all-buffer quit guards pass; multi-file PTY workflow passes. |
| Navigation and presentation | Nucleo ranking/ignore/cancellation/stale-query tests, tiny-terminal rendering and all three RGB palette checks pass. |

**M2 is implemented and meets the automated Linux acceptance checks for the
measured workloads. Full platform/interactive acceptance is not claimed:** macOS
and physical-terminal comfort remain unobserved, and the separate real Kotlin
feasibility gate is blocked by the missing server executable. Known product
limits—including a Vim subset, lexical text objects, literal search, synchronous
file/search work and no persistent configuration—are listed in the README. Git,
integrated LSP, AI, plugins, GUI and splits remain outside this delivery.

## Milestone 3 baseline and Git backend

Revalidated the unchanged `07a326f` checkout on Linux with Rust/Cargo 1.99.0
and Git 2.43.0: formatting, Clippy, all 92 Rust tests, seven Python probe tests,
all ten PTY sessions, debug/release builds and the release performance example
passed. Default Cargo attempted a crates.io refresh and failed DNS resolution.
The existing registry was verified and reused offline through
`CARGO_HOME=/tmp/editor-pro-m3-cargo` (a registry link to the previous cache,
without its obsolete rustup command shims). No global toolchain repair was needed.

Baseline edit/draw p95 milliseconds: Rust 0.93, Java 1.99, Kotlin 0.93, C# 1.03,
JSON 0.76, YAML 1.48, TOML 1.94, Markdown 49.88 (max 53.93). Picker scan/rank:
18.67 ms for 10,000 files; query 1.10 ms. These are observations, not guarantees.

The first Git slice adds the bounded CLI worker and comparison/coordinate model.
Formatting, Clippy and 13 new Git regression tests pass. Tests cover separate
HEAD/index/revision/staged comparisons, unsaved Rope snapshots, unusual filenames,
renames, additions/deletions/untracked/empty files, detached/unborn HEAD, linked
worktrees, outside-repository discovery, binary/conflict/error states, CRLF,
Unicode, missing final newlines, paired coordinates, stale requests, timeouts and
shutdown process cleanup. Inspection tests assert unchanged worktree/index bytes.
No language server was provisioned; `intellij-server` and `kotlin-lsp` remain
missing. macOS and physical-terminal validation remain unobserved.

## M3 contextual navigation and rendering slice

All 115 Rust tests (92 existing plus 23 Git/editor regressions), formatting and
Clippy pass. The expanded Linux harness passes all 13 PTY sessions. The new
sessions cover changed-file selection, historical deletions, staged/index/HEAD
comparisons, missing revisions, unsaved text, paired layout, narrow resizing,
source return, and unchanged index bytes. All sessions check terminal restoration.
The harness now waits for fresh output after asynchronous view changes; a first
run exposed its earlier assumption that Git would always finish in 150 ms.

The Git responsiveness example completes on a temporary repository containing
2,501 tracked files, 100 staged, 100 unstaged and 100 untracked paths. An initial
run observed 139.67 ms status/comparison completion and 135.55 ms after a burst of
150 edits (including the 75 ms debounce). Navigation key/tick/draw p95 was 0.79 ms,
max 2.69 ms; typing p95 4.11 ms, max 5.86 ms. It uses a 120×32 TestBackend and an
active 3,001-line Rust buffer. Setup, terminal transport and existing synchronous
disk checks are excluded; these numbers are observations, not timing guarantees.

## M3 final hardening and acceptance

Final code validation on Linux x86_64 used Rust/Cargo 1.99.0 and Git 2.43.0,
with the verified offline registry described above. There are **121 passing Rust
tests**: the original 92 and 29 Git/backend/editor/rendering regressions. The last
slice adds cancellation during a running Git process, helper suppression,
literal-pathspec and non-UTF-8 filename handling, content/patch limits, visible
errors, CRLF/final-newline annotations, and source navigation to dirty buffers
even after their parent directory has been removed. No dependencies were added;
`Cargo.lock` is unchanged.

| Final check | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 121 passed, none failed or ignored. |
| `cargo build --locked` | Passed. |
| `cargo build --locked --release --bin editor-pro` | Passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | All seven harness/test-double tests passed. |
| `python3 tools/terminal_smoke.py` | All 13 sessions passed, including restoration checks. |
| `cargo run --locked --release --example performance` | Passed all grammar and picker assertions; measurements below. |
| `cargo run --locked --release --example git_performance` | Passed repository/index invariance and responsiveness workload; measurements below. |
| Release `--help` / `--version` without TTY | Passed; help includes Git commands. |
| `git diff --check` | Passed. |

Source jumps and changed-file opens reuse known buffer identities before resolving
filesystem paths. This preserves dirty documents whose entire containing directory
was deleted externally; inspection never recreates that directory. Clean deleted
files instead retain an inspectable historical diff with no current destination.

Git tests use temporary repositories and local deterministic author/identity/EOL
configuration. They assert unchanged index and worktree bytes after inspection.
Cancellation tests wait for a launcher-start signal before superseding a request,
then assert only the latest result and that the earlier process was reaped. No
assertion treats an observed interactive latency as a hard correctness threshold.
Configured diff/textconv, clean/process and fsmonitor helpers are prevented from
executing; a regression test checks that their side-effect marker is never created.

| Milestone 3 criterion | Evidence / assessment |
| --- | --- |
| Inspect working changes inside the editor | Changed-file view, source gutters, inline and paired diffs; Rust and PTY workflows pass. |
| Compare every supported base | HEAD, index and selected commit-ish, plus explicitly separate HEAD-to-index staged comparisons; staged+unstaged+unsaved fixtures pass. |
| Deletion, rename and untracked files | Historical deletions, rename source paths, additions/empty files and buffer-only entries; unusual paths, detached/unborn HEAD and linked worktrees pass. |
| Close restores source location | Original buffer ID/cursor/viewport and independent dirty histories survive scrolling, refresh, base/layout changes and resizing. Explicit source-jump/deletion anchors have separate tests. |
| Refresh without blocking Git work in editing | One bounded coalescing worker; cancellation, timeouts, failure states and stale-result suppression; measured key/tick/draw workload below. No Git subprocess work in input/render. |
| Independent of LSP | No LSP dependencies, provisioning or runtime integration; Git tests and PTY workflows run without a language server. |

**Milestone 3 meets its automated Linux acceptance criteria for these workloads.**
This does not claim macOS, hosted CI, physical-terminal visual/comfort validation,
a minimum-Rust build or real Kotlin LSP feasibility. The real server executable is
still absent. All remain separate open gates. Milestone 4 review marks, notes,
persistence/export and later snapshots/LSP/AI/plugin/GUI work were not implemented.

Known limits: comparisons are UTF-8 text up to 2 MiB per side, with 100,000-row/file
and 16 MiB process-output caps. Conflicts, symlinks and submodules are unsupported
for textual inspection; submodule interiors are ignored. Clean/process filters
are disabled and reported, so filter-dependent status can differ from ordinary
Git. Rename detection follows Git heuristics and untracked moves can appear as a
deletion/addition pair. Diff reanchoring after substantial changes is approximate.
Repository sampling is not atomic against external writers, and clean loaded
buffers can intentionally differ from disk (labels and disk-conflict flags explain
this). Polling refreshes derived data, never document text/history. Filesystem
stalls, existing synchronous file/search work, large Markdown parsing and terminal
transport remain responsiveness limits; no hard latency ceiling is claimed.

### Final release performance observations

The existing eight-language workload is unchanged: 50 edit/undo/draw samples per
fixture and a 10,000-file picker. The 120×32 TestBackend excludes terminal output.

| Language | Initial ms | Edit+draw p95 ms | Maximum ms | Scroll+draw p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Rust | 6.03 | 0.93 | 1.17 | 0.30 |
| Java | 4.44 | 2.02 | 2.23 | 0.40 |
| Kotlin | 5.97 | 0.99 | 1.36 | 0.46 |
| C# | 6.56 | 1.01 | 1.37 | 0.44 |
| JSON | 3.61 | 0.73 | 0.78 | 0.37 |
| YAML | 4.73 | 1.45 | 1.84 | 0.24 |
| TOML | 3.95 | 2.00 | 2.17 | 0.33 |
| Markdown | 35.90 | 52.53 | 54.59 | 0.35 |

Picker scan/ranking took 20.00 ms; its query settled in 0.54 ms. No grammar entered
fallback. Markdown remains the slowest fixture; changes to Git did not remove the
existing cooperative-parser limitation.

Reproduce the new workload with
`cargo run --locked --release --example git_performance`. It creates 2,501 tracked
files (40 lines each except a 3,001-line UTF-8 Rust source), 100 staged paths,
100 unstaged paths and 100 untracked paths. It measures 150 navigation samples
during refresh and 150 typing samples that coalesce pending work. Both index and
disk bytes are asserted unchanged after the unsaved edit burst.

| Final Git measurement | Observed ms |
| --- | ---: |
| Initial discovery, status and comparison | 117.87 |
| Refresh completion after the edit burst | 133.87 |
| Navigation key + tick + draw p95 / maximum | 1.61 / 2.49 |
| Typing key + tick + draw p95 / maximum | 5.09 / 6.76 |

Refresh completion includes the 75 ms quiet-period debounce and 1 ms result
polling. The interaction samples include editor handling, request submission or
polling, syntax and drawing, but exclude deliberate inter-sample sleeps. They
exclude fixture setup, terminal transport and the main loop's existing synchronous
disk checks. Continuous typing postpones comparisons until the quiet period;
cursor navigation permits the worker to finish while drawing continues. These are
single-run local-VM observations, not representative remote-filesystem or large
monorepo guarantees.

For this VM's offline reproduction, use `CARGO_HOME=/tmp/editor-pro-m3-cargo` and
`CARGO_NET_OFFLINE=true` with the commands above. This is a verified temporary
cache path for this session, not a distribution prerequisite. Normal installations
can fetch the unchanged lockfile dependencies through Cargo's usual registry.

Implementation was saved in logical local commits; `prompt.md` was left untracked
and excluded. No push was performed.

## Milestone 4 baseline and first review slice

Revalidated `cef231e` without resetting the checkout. Rust/Cargo 1.99.0,
rustfmt 1.10.0, Clippy 0.1.99 and Git 2.43.0 are available globally. The complete
M3 baseline passed on Linux: formatting, Clippy, 121 Rust tests, seven Python
probe tests, 13 PTY sessions, debug/release builds, both release performance
examples, release help/version without a TTY, and `git diff --check`.
Baseline Git measurement: 224.58 ms initial comparison, 190.48 ms after the
150-edit burst, navigation p95/max 1.03/2.66 ms and typing 4.78/6.32 ms.
Baseline Markdown edit/draw p95/max was 53.64/74.33 ms. These are local observations.

The verified `/tmp/editor-pro-m3-cargo` registry link remains usable. Sandbox
registry DNS failed; authorized Cargo network access fetched the new SHA-256
implementation. `serde`/`serde_json` were already in the cache/lockfile;
`sha2` 0.10.9 and its dependencies were added to Cargo.lock. Subsequent checks use
`CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true` and `--locked`.
These temporary cache paths are not product prerequisites.

The first M4 slice passes formatting, all-target Clippy, and all **139 Rust tests**
(121 existing plus 18 review regressions). It implements structured full identities,
SHA-256 content fingerprints, unique hunk/line signatures, marks, durable notes,
record-level conflict checks, explicit Markdown export, contextual navigation,
and background validation of the current file plus up to 31 annotated paths.
Tests cover exact-byte undo, restored timestamps, both sides/index/moving refs,
missing versus empty, endings, repetitions, worktrees, persistence failures,
conflicts, restart, source-history preservation and bounded rendering.
The extended terminal workflow and final performance results are recorded below
when observed; this slice does not claim those new checks have run yet.

## M4 final hardening, workflow and acceptance

Final Linux x86_64 validation uses Rust/Cargo 1.99.0 and Git 2.43.0 with the
verified offline cache and Cargo.lock. All **151 Rust tests** pass: 121 existing
and 30 M4 regressions. The final code also passes the seven Python harness tests
and **16 PTY sessions** (13 existing plus three review workflows).

Hardening covers mode changes, strict store-schema validation, full worktree/base
isolation, obsolete record revisions, monotonic review request generations,
concurrent writers, failed annotations surviving a repository switch, and the
32-path validation limit. Identical hunks in different files cannot borrow each
other's displayed marks. Review-list commands require opening a comparison before
annotating current content. File/hunk filtering builds progress once instead of
repeatedly scanning every record for every changed path.

| Final check | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 151 passed; none failed or ignored. |
| `cargo build --locked` | Passed. |
| `cargo build --locked --release --bin editor-pro` | Passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | Seven passed; harness/test doubles only. |
| `python3 tools/terminal_smoke.py` | All 16 sessions passed against the final debug binary. |
| `cargo run --locked --release --example performance` | All eight grammars and the 10,000-file picker completed with assertions passing. |
| `cargo run --locked --release --example git_performance` | Completed; worktree/index invariance assertions passed. |
| `cargo run --locked --release --example review_performance` | Completed; progress, persistence/export and source/index invariance assertions passed. |
| Release `--help` / `--version` with non-TTY stdin/stdout | Passed; help includes review/note/export commands, version 0.1.0. |
| `git diff --check` | Passed, including final documentation. |

The new PTY workflow inspects changes, marks a file and hunk, changes layouts and
resizes to narrow widths, edits a Unicode/multiline note, quits, restarts and
observes resumed fresh annotations. It changes source externally, explicitly
reloads, observes STALE records, and exports escaped Markdown outside the
worktree. A third session provokes a metadata-lock failure, keeps and saves the
note draft after removing the test lock, and verifies export overwrite refusal.
The harness waits for observable rendered or durable-store signals and retries
idempotent commands until those signals, rather than assuming Git/I/O finishes
within a sleep. Every session checks termios, alternate-screen exit and disabled
bracketed paste. Source/index bytes are asserted unchanged by review operations;
the deliberate external source edit is accounted for separately.

Regression coverage includes file/hunk state transitions and independent counts;
file/hunk/old/new notes; reload/restart and unsaved-content validation; explicit
clear/revalidation/attachment correction; same-length edits with restored mtime;
changes to either side, index and moving revision names; CRLF/final newline;
empty versus absent; exact-byte undo/redo; repeated signatures; deleted files,
renames and untracked files; cancelled/obsolete workers and record revisions;
non-UTF-8/control-containing paths; bounded Unicode notes/context; corrupt,
unknown-field, future-version, oversized, symlinked, read-only and
locked metadata; independent/concurrent writers; deterministic escaped export,
no-clobber destinations and queued-write shutdown durability. Note editing keeps
source histories, registers, cursor and viewport intact. Layout/movement alone
leave identical comparisons fresh.

### Final M4 release measurements

All numbers below are one final local-VM run, not thresholds or hard guarantees.
The 120×32 TestBackend excludes real terminal transport. Existing synchronous
source loading/saving/disk checks remain separate costs.

| Language fixture | Initial ms | Edit+draw p95 / max ms | Scroll+draw p95 ms |
| --- | ---: | ---: | ---: |
| Rust, 51,000 bytes / 3,001 lines | 6.16 | 0.94 / 1.21 | 0.26 |
| Java, 39,000 / 1,001 | 4.52 | 2.34 / 2.70 | 0.42 |
| Kotlin, 33,000 / 1,001 | 6.74 | 1.18 / 1.41 | 0.47 |
| C#, 39,000 / 1,001 | 6.59 | 1.06 / 1.49 | 0.42 |
| JSON, 28,017 / 1,004 | 3.65 | 1.07 / 1.76 | 0.39 |
| YAML, 22,000 / 2,001 | 4.92 | 1.53 / 1.73 | 0.25 |
| TOML, 24,000 / 2,001 | 4.08 | 2.22 / 2.27 | 0.23 |
| Markdown, 65,000 / 8,001 | 36.99 | 49.34 / 58.22 | 0.29 |

No grammar entered fallback. The 10,000-file picker scan/rank took 18.64 ms;
the query settled in 1.09 ms. Large Markdown remains the separate known latency
limitation; review does not remove it.

The unchanged Git fixture contains 2,501 tracked files, 100 staged, 100 unstaged,
100 untracked and an active 3,001-line Rust buffer. Initial status/comparison:
**191.61 ms**. Refresh after 150 coalesced edits: **191.01 ms**, including the
75 ms debounce. Navigation key+tick+draw p95/max: **1.34/3.22 ms**; typing:
**4.66/6.30 ms**. Setup, terminal transport, deliberate between-sample sleeps and
the main loop's synchronous disk checks are excluded.

Reproduce review measurements with
`cargo run --locked --release --example review_performance`. It builds 24 tracked,
changed files, each **45,844 bytes / 1,001 lines**, with two hunks per file. Initial
metadata has **288 records**: 24 reviewed file marks, 24 attention hunks and 240
Unicode/multiline notes. After saving one more note, the store is **801,971 bytes**.

| Review measurement | Observed ms |
| --- | ---: |
| Initial metadata/Git load and 24-path validation | 678.64 |
| Note submission, serialization, atomic persistence and completion polling | 10.85 |
| Refresh of 24 annotated paths, spanning the navigation sample loop | 444.66 |
| Markdown export and completion polling | 7.49 |
| Note typing key+tick+draw p95 / maximum, 150 samples | 0.13 / 0.20 |
| Diff navigation during refresh p95 / maximum, 150 samples | 0.87 / 0.97 |
| SHA-256 of both 2 MiB sides, p95 / maximum, 40 samples | 2.36 / 2.58 |

Load/refresh include debounce and 1 ms polling. Fixture creation and the initial
source Document open are outside the timed region. Hash timing isolates content
fingerprinting and excludes reads/Git/diff construction. I/O timings include queue
submission, serialization/fsync or export and observable completion, not just
in-memory mutation. Navigation/typing include handling, tick and draw, excluding
deliberate waits. There are no remote-filesystem or monorepo latency claims.

### Acceptance and remaining limits

**Milestone 4 meets its automated Linux acceptance criteria:** changes can be
reviewed, annotated, resumed later and exported entirely in the terminal, with no
AI provider or language server. Tests verify that changed content cannot
accidentally retain a fresh mark. The exact-byte undo policy is intentional and
requires a newly accepted full comparison, never an old generation or row anchor.
README contains the complete command reference and a review/resume/export walk-through.

Validation is deliberately bounded to the current path plus 31 other annotated
paths for the selected comparison; excess paths, other bases, errors and unsupported
comparisons remain unavailable. File/hunk counts are explicit fresh marks, not a
percentage asserting an entire repository is reviewed. A containing content/base
change stales all its annotations; repeated/ambiguous locations require a file
note or explicit correction. No M7 snapshots or automatic reanchoring are included.
Git sampling is non-atomic, and clean loaded buffers intentionally remain distinct
from externally modified disk. Metadata caps are 4 MiB/4,096 records/16 KiB per
note/512-byte context. Locks are cooperative and abandoned locks require manual
cleanup; arbitrary external writers and filesystem stalls remain limitations.
Exports require a new path outside the worktree/Git metadata and open source paths.

macOS, hosted CI, a minimum-Rust 1.88 build and physical-terminal appearance/comfort
remain **unobserved**. Neither `intellij-server` nor `kotlin-lsp` is on PATH; no real
Kotlin server was provisioned or run for M4. Its separate feasibility gate remains
blocked by the missing executable; seven Python tests validate only the harness.
No integrated LSP, AI, plugins, GUI or later-milestone snapshot engine was added.
Implementation is recorded in logical local commits. `prompt.md` remains untracked
and excluded from commits. No push was performed.

## M5 initial language client implementation and validation

Inspected the actual M1–4 modules and tests before selecting M5. At the start of
implementation, no `AGENTS.md` existed in this checkout or its ancestors. The
starting checkout was clean, and all 151 existing Rust tests passed before
implementation.

The client uses a concrete background manager for Kotlin, Java and C# sessions.
It adds negotiated commands, document synchronization, diagnostics and contextual
result/edit-preview views without changing the editing engine. Tests use
`tools/lsp/fake_server.py`, explicitly labelled as a protocol test double.
Separate language IDs and roots do not prove support from three real servers.

Final Rust validation on Linux x86_64 uses Rust/Cargo 1.99.0 and the updated
`Cargo.lock`. The new direct dependencies are `lsp-types` 0.95.1 and Tokio
(resolved to 1.53.3). Cargo fetched their dependencies using authorized network
access; later checks used `CARGO_HOME=/tmp/editor-pro-m3-cargo` and
`CARGO_NET_OFFLINE=true`. The temporary cache is not a product prerequisite.

| Check | Observed result |
| --- | --- |
| `cargo fmt` and `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | **178 passed**, none failed or ignored: 151 existing and 27 new LSP regressions. |
| `cargo build --locked` | Passed. |
| `cargo build --locked --release --bin editor-pro` | Passed on the final M5 code. |
| Release `--help` / `--version` without a TTY | Passed; help includes LSP commands, version 0.1.0. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | All seven tests passed; harness/test doubles only. |
| `python3 tools/terminal_smoke.py` | All **17 sessions passed** against the final debug binary, including terminal restoration. |
| Isolated real Kotlin probe | Blocked, exit 2: executable absent. |
| `git diff --check` | Passed. |

Coverage includes fragmented framed stdio and malformed/oversized messages;
UTF-16 surrogate rejection, combining graphemes and CRLF; full and incremental
synchronization; save, reload, save-as and close; separate Kotlin/Java/C# sessions;
Gradle/Maven/solution/project boundaries and explicit root overrides; missing
servers, unsupported capabilities/encodings, slow initialization, deadlines,
crash recovery, explicit restart, shutdown and child reaping. Captured requests
are rejected after edits, reloads, cursor/buffer changes or newer requests.
Versioned stale diagnostics are suppressed; pull and unversioned reports retain
their distinct provenance.

Feature tests exercise completion resolution/documentation, hover, signature
help, definitions/references, symbols, diagnostic navigation, formatting, rename,
direct/resolved/command-driven code actions, preview cancellation and application.
Multi-file edits are validated before any mutation, preserve dirty text and
independent histories, and leave disk bytes untouched. Tests reject conflicting
versions, overlapping edits, resource operations, external writes and
unsynchronized open targets. Review note text never reaches the source server.
A code-action preview can remain open beyond the network request deadline.
Rendering checks include 1×1, 2×2 and narrow contextual views.

The PTY addition runs the real editor binary against the test double: wait for
actual didOpen, inspect hover, request formatting, resize an edit preview, apply,
undo, apply again, save and quit. It checks exact saved bytes, shutdown/exit,
alternate-screen exit, bracketed-paste disablement and termios restoration.
Existing editing/Git/review sessions are retained.

Review found and corrected completion cursor placement after returning to Insert,
combining-grapheme completion prefixes, save-as session retention, didClose on
last-buffer shutdown, pull-diagnostic scheduling after immediate requests, and
server edits targeting unsynchronized open buffers. No general backend trait,
plugin framework, second editing engine or M6–8 feature was added.

**Acceptance remains open.** The installed environment has no
`intellij-server`/`kotlin-lsp`, `jdtls`, `csharp-ls` or `dotnet`. The real Kotlin
probe still reports missing prerequisites. Provision pinned real distributions
and record capabilities, runtime/project SDKs, cold/warm import behavior and
essential commands on representative Gradle, Maven, `.sln` and `.csproj` projects.
No KMP, Android, Razor or framework support is inferred from protocol fixtures.
macOS, hosted CI, minimum Rust 1.88 and physical-terminal comfort remain unobserved.

Known client limits are documented in README: manual completion, plain
documentation, static capabilities, no snippets/completion commands, virtual
sources, watched-file registration or resource/annotated edits; eight sessions,
2 MiB documents and 32-file/4,096-replacement proposals. Undo is per buffer.
Versionless push diagnostics are unverified. Open edit targets must have been
synchronized by the requesting server. Unopened unversioned targets are guarded
from proposal preparation onward, not before the response. JDT workspace data
is temporary. Existing performance examples compile as all-target checks, but
their timing workloads were not rerun and no new real-server latency claim is made.

## M5 real-server acceptance follow-up

The next incomplete milestone was M5 acceptance, not M6. Inspection confirmed
that the client and 27 LSP regressions already existed, while the real-server
gate was still open. All 178 starting Rust tests passed. This follow-up adds
real-project acceptance and fixes the interoperability failures it exposed;
no later-milestone features or dependencies were introduced.

Incremental fixes and regression coverage:

- Advertise pull-diagnostic refresh support, cancel superseded in-flight pulls,
  and repull after indexing notifications. Inter-file dependency reports retain
  all captured document stamps and invalidate when another document changes.
- Advance versions independently for each document while seeding newly opened
  paths above the session maximum. The tested Kotlin server returned consecutive
  document versions; the previous global counter skipped versions when another
  file opened. Version checks and buffer-instance guards remain strict.
- Support advertised completion commands, including commands supplied during
  resolution. The first preview requires approval before execution; subsequent
  `workspace/applyEdit` edits require a separate guarded preview. Cancellation,
  unadvertised commands, undo and unchanged disk bytes have regressions.
- Enable JDT signature help through initialization and workspace configuration.
  Its advertised capability alone was insufficient with the default preference.
- Close stdin after shutdown/exit so stdio servers can finish their transport
  loops. Both clients have EOF regressions; the editor retains bounded kill/reap
  fallback. The independent real Kotlin probe now exits cleanly.

Under concurrent import load, the existing PTY buffer workflow sometimes sent a
command before Escape had been processed, leaving command text in Insert mode.
That sequence now waits for observable Normal/buffer-list/source transitions
instead of relying on a fixed delay to separate Escape from the next command.

### Final automated checks

Linux x86_64, Rust/Cargo 1.99.0, Python 3. Cargo checks used the existing temporary
cache with `CARGO_NET_OFFLINE=true`. Network access was used separately for pinned
server/SDK downloads and language-server dependency imports.

| Check | Observed result |
| --- | --- |
| `cargo fmt` and `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | **186 passed**, none failed; **six opt-in real-server tests ignored**. Includes 35 LSP regressions. |
| `cargo build --locked` and release binary build | Passed. |
| Python probe unittest discovery | **Eight passed**; protocol-double/harness evidence only. |
| `python3 tools/terminal_smoke.py` | **18 sessions passed**, including completion-command cancellation, both approvals, undo, save and terminal restoration. |
| `cargo test --locked --test lsp_real LANGUAGE_ -- --ignored --nocapture --test-threads=1` | **Six passed** across the Kotlin, Java and C# filters, using actual servers. |
| Standalone official Kotlin probe | **Passed**, exit 0, 63.052 seconds; diagnostics version 2 after an unsaved edit and clean process exit. |
| `git diff --check` | Passed. |

The ordinary suite downloads nothing and requires no installed language server.
The opt-in tests exercise the ordinary Rust `Editor`, background worker, contextual
views and edit approval path. They copy source/build fixtures to temporary roots,
wait for actual diagnostics, and fail on missing prerequisites, timeouts or failed
features. A failed real feature is never recorded as a successful empty result.

### Pinned real environments and results

[servers.json](tools/lsp/servers.json) records archive URLs and verified hashes.
Kotlin, JDT LS and Gradle SHA-256 values were checked against published files;
the .NET SDK SHA-512 comes from Microsoft release metadata. The csharp-ls SHA-256
records the installed NuGet package. Reproduction and cache isolation instructions
are in [tools/lsp/README.md](tools/lsp/README.md).

| Server | Observed runtime | Fixture targets |
| --- | --- | --- |
| Official Kotlin 263.6379.0 | Bundled JBR 25.0.4.1+1-b601.61 | Kotlin 2.3.0, JVM 21; Maven and two-module Gradle 9.1.0 with JDK 21 toolchain |
| JDT LS 1.60.0-202606262232 | OpenJDK 25.0.4.1; server reports `1.60.0-SNAPSHOT` | Java 21; Maven compiler plugin 3.14.1 and two-module Gradle 9.1.0 with JDK 21 toolchain |
| csharp-ls 0.28.0 | .NET SDK 10.0.402 | `net10.0`; project-only and app/library solution |

All six fixtures passed diagnostics, hover, signature help, definition jumps,
references, document symbols, quick-fix previews, completion documentation and
application, unsaved diagnostic changes, formatting/apply/undo, multi-file rename,
cancel/apply/undo, dirty quit protection and restart/resynchronization. Original
source/build bytes remained unchanged on disk. The type-error fixes were Kotlin
and Java's change-declared-type action and C#'s explicit-cast action. Kotlin's
completion exercised the command-driven two-preview path. With snippets disabled,
the tested Java completion can insert a method name without adding parentheses.

The fresh-project and restart samples below use cached Maven/Gradle/NuGet
dependencies. Every test starts from a new copied project, and JDT workspace
indexes are temporary on both launches. Times are observed readiness samples,
not cold-download benchmarks or guarantees; CPU contention affects them.

| Fixture | Result | First diagnostics (s) | Unsaved restart/resync (s) | Diagnostics |
| --- | --- | ---: | ---: | --- |
| kotlin-maven | Passed | 36.37 | 5.89 | Version-verified |
| kotlin-gradle | Passed | 59.09 | 6.04 | Version-verified |
| java-maven | Passed | 11.96 | 8.58 | Versionless push |
| java-gradle | Passed | 14.30 | 16.58 | Versionless push |
| csharp-project | Passed | 4.56 | 4.15 | Version-verified |
| csharp-solution | Passed | 4.62 | 5.52 | Version-verified |

The standalone probe report is `/tmp/editor-pro-m5-real-kotlin.json`; editor
reports are `/tmp/editor-pro-m5-real-final/*.json`. They contain fixture hashes,
commands, capabilities, bounded logs, per-check results/timings and failure phases.
Re-running with `EDITOR_PRO_LSP_REPORT_DIR` writes fresh reports. Source fixtures
and pinned downloads are retained in the repository; downloaded binaries/caches
and raw reports are temporary environment artifacts.

### Limits and manual checks

A cold JDT Maven import timed out an early references request at the normal
15-second deadline; cached retries passed. The client remains usable and reports
the error. Further cold-download, large-project and peak-memory measurements are
still needed. csharp-ls logs a null-configuration warning and uses defaults;
all tested features worked. JDT's push diagnostics were versionless, so their
freshness remains explicitly unverified; Kotlin/C# reports were version-verified.

Real-server restart with unsaved text is covered. Missing-server, malformed
protocol, timeout, crash/retry and out-of-order races are covered by deterministic
doubles; real crash injection remains a manual check. The small Gradle fixtures
prove app/library import and cross-module navigation, not large or mixed-language
build compatibility. Android, KMP, Razor, older .NET targets, macOS, minimum Rust
1.88, hosted CI and physical-terminal comfort remain unobserved.

Manually inspect `:Doctor`, import progress and `:LspLog` on your projects; try
completion/diagnostics/navigation, quick fixes, formatting and multi-file rename.
Cancel and apply proposals, undo in each buffer, check dirty quit protection,
and restart or terminate the server with unsaved text. Inspect narrow/normal
layouts and terminal restoration on Linux and macOS. Snippets, virtual documents,
watched-file registration and resource/annotated edits remain unsupported.
No editing/Git/review performance workload was rerun in this follow-up.

## M6 code comprehension

Inspection of the implementation, tests and pinned-server fixtures confirmed M6
as the next incomplete milestone. All **186** starting deterministic Rust tests
passed. Nested document symbols already existed; the new outline presentation
and alias reuse them. Work proceeded through source history/recent files,
cancellable project search, then workspace symbols and call hierarchy. No new
Cargo dependency, general service abstraction, review snapshot or AI feature was
introduced.

### Final automated checks

Linux x86_64, Rust/Cargo 1.99.0, Python 3 and ripgrep 15.2.0. Cargo commands used
the existing `/tmp/editor-pro-m3-cargo` cache with `CARGO_NET_OFFLINE=true`.

| Check | Observed result |
| --- | --- |
| `cargo fmt`, `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | **205 passed**, none failed; six opt-in real-server tests ignored. Includes 42 LSP, six navigation and six project-search regressions. |
| `cargo build --locked` | Passed. |
| `cargo build --locked --release --bin editor-pro` | Passed. |
| Python probe unittest discovery | **Eight passed**; harness/double evidence. |
| `python3 tools/terminal_smoke.py` | **20 sessions passed**, including restoration and the new search/navigation/comprehension workflows. |
| Extended `lsp_real` tests, with `--ignored --nocapture --test-threads=1` | **Three passed**: `kotlin_maven`, `java_maven`, `csharp_project`. |
| `git diff --check` | Passed. |

Navigation coverage includes independent dirty buffers and undo, exact cursor and
viewport restoration, branching forward history, save-as/close/reopen, inaccessible
history entries, deleted parent directories, bounded stacks/recent paths and
source-preserving narrow rendering. The existing buffer/file/Git/review suites
remain green.

Search tests run actual ripgrep on Unicode/CRLF text, repeated matches, literal
metacharacters/option-like queries, ignore files, hidden files, binary files,
symlinks, colon/newline/non-UTF-8 Unix filenames and automatic/explicit roots.
They check byte-to-scalar jumps and exact return locations. Dirty-buffer and
external-write mismatches refuse navigation without creating or replacing a
buffer. A separate process double exercises delayed/obsolete results, cancellation,
leaving the view, missing rg, malformed output, stderr failures, timeout/reaping
and visible 4,096-result truncation. Input/escape/paste remain responsive while
that double sleeps; this is not a large-project throughput benchmark.

LSP regressions cover nested outline labels, workspace query parameters, lazy
symbol resolution with opaque data, dirty-buffer reuse, recursive incoming/
outgoing calls, caller-relative call-site details, parent selection/scroll,
16-level limits, empty/unsupported/virtual results, malformed ranges, UTF-16
surrogate rejection, cancellation before/after a result panel, and late replies
after edits, newer queries or returning to a parent. All existing M5 tests remain.
Review preserved failed diagnostic-list selections and eliminated a second
unopened-target read between validating its position and activating it.

The PTY harness adds a real-ripgrep search across files, back/forward with an
unsaved edit and undo, recent-file selection, extreme resize, and a separate
protocol-double outline/workspace-symbol/call-hierarchy workflow. Saved bytes
and terminal restoration are checked. An older picker session's fixed Escape
delay failed under concurrent load; it now waits for observable Normal mode.
New checks force a full redraw where needed because Ratatui's incremental escape
stream can omit unchanged spaces/characters even when the screen is correct.
`EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-m6-terminal` retained
`m6-search-history.ansi` and `m6-symbols-calls.ansi` from the passing run.

### Real-server observations

The installed pinned distributions from `tools/lsp/servers.json` were reused:
official Kotlin 263.6379.0, JDT LS 1.60.0 and csharp-ls 0.28.0 with .NET 10.0.402.
The extended fixture tests require a useful workspace symbol, a successful source
jump, incoming and outgoing calls, and restoration of the original source
cursor/viewport. They also retain the M5 completion, diagnostics, edits, undo,
restart/resynchronization and unchanged-disk checks. All three advertised the
new capabilities and passed; no unsupported branch was counted as real feature
success. Lazy workspace-symbol resolution is additionally exercised by the double.

| Fixture | Workspace symbols (ms) | Prepare calls (ms) | Incoming (ms) | Outgoing (ms) |
| --- | ---: | ---: | ---: | ---: |
| Kotlin Maven | 185 | 198 | 239 | 221 |
| Java Maven | 10 | 20 | 63 | 53 |
| C# project | 64 | 35 | 98 | 43 |

These are single observed samples on small fixtures with cached dependencies,
not performance guarantees. New-project first diagnostics took 37.16, 7.76 and
9.21 seconds respectively; unsaved restart/resync took 7.22, 6.84 and 7.21 seconds.
Reports are `/tmp/editor-pro-m6-real-final/{kotlin-maven,java-maven,csharp-project}.json`.

Two sandboxed Kotlin attempts failed before useful project import: the native
server/importer still tried to install its Maven plugin under read-only
`/root/.m2`, despite copied writable dependencies and JVM cache overrides. Those
180-second failures are preserved in `/tmp/editor-pro-m6-real` and
`/tmp/editor-pro-m6-real-cached`. The successful runs used approved execution
with the required local networking/cache access and disposable project roots.
No source fixture was weakened to turn an import failure into a pass.

### Limits and manual verification

M6 did not rerun the Gradle or solution fixtures, the standalone Kotlin probe,
or earlier editing/Git/review performance examples. Their M5/M4 observations above
remain historical evidence. macOS, minimum Rust 1.88, hosted CI, large production
projects and physical-terminal appearance remain unobserved.

Install rg, open your project, try `:grep TEXT` and `:grepdir DIRECTORY`, cancel
or replace a slow query, and open results with dirty buffers. Follow definitions,
`:WorkspaceSymbols`, and `:Calls` (`i`/`o`, Backspace, Enter); use Ctrl-o/Ctrl-i
to return through each source location. Check `:recent`, same-file jumps, undo,
external-write conflicts, wide text and narrow layouts on Linux/macOS.

History/recent files are session-local and scalar locations do not track moving
symbols. Search uses literal case-sensitive disk text, a 2 MiB file limit,
4,096 results, 16 MiB capture and a cooperative ten-second deadline. Selecting a
result validates its line, not an atomic whole-project snapshot. Existing source
file loads remain synchronous under the 64 MiB limit. Workspace queries use the
current project/language session; calls retain at most 16 parents. Virtual sources,
dynamic registrations and later review/AI milestones remain outside this slice.


## M7 immutable review snapshots

2026-10-15: inspected the implementation before selecting the next incomplete
milestone. The existing Git `Snapshot` and M4 content hashes did not retain source
baselines. All 205 starting deterministic Rust tests passed before implementation.
M7 adds immutable trees to the existing review store/worker and uses the existing
Git diff projection and request guards. No dependency or AI integration was added.

### Automated evidence

The following passed on Linux with stable Rust. Cargo used the existing verified
`/tmp/editor-pro-m3-cargo` cache and `CARGO_NET_OFFLINE=true`; initial default
registry access failed DNS. No lockfile/dependency changes were needed.

```sh
cargo fmt
cargo fmt --check
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo clippy --locked --all-targets -- -D warnings
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo test --locked --all-targets
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo build --locked
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-m7-terminal python3 tools/terminal_smoke.py
git diff --check
```

There are **219 passing deterministic Rust tests**, including **14 new snapshot
tests** in `tests/review_snapshots.rs`. The six opt-in real-server tests remain
ignored by this command. Eight Python probe tests and **22 PTY sessions** pass.
The terminal harness adds two real-editor workflows: capture/review an external
revision while retaining clean and dirty source, then restart and append a second
checkpoint while retaining the first full baseline. Saved file and index bytes,
checkpoint contents and terminal restoration are asserted. ANSI captures are
`/tmp/editor-pro-m7-terminal/m7-live-review.ansi` and
`/tmp/editor-pro-m7-terminal/m7-checkpoint-resume.ansi`.

| Invariant | Evidence |
| --- | --- |
| Capture does not modify source, index or refs | Exact byte/status assertions, unsaved Rope capture, and PTY index/source checks. |
| Checkpoints and the full baseline stay immutable | Multiple captures, external commits, restart, pinned checkpoint IDs, and removal of the original loose Git commit. |
| Disk revisions cannot silently retain file progress | Same-size writes with restored mtime, periodic refresh, and changed whole-file mark staleness. |
| Unsaved source and location survive | Dirty and clean loaded documents, external-save refusal, cursor/viewport/undo, and source mapping to retained bytes. |
| Content reattachment is conservative | Unique hunk/old/new-line notes, Unicode/CRLF and shifted coordinates; changed/repeated signatures are stale and never attached by ordinal. |
| Scope and persistence remain bounded | Concurrent checkpoint CAS plus merged annotations, version-1 retention, corrupt hashes, invalid paths, 512-path and 32-checkpoint limits, unsupported binary/symlink refusal. |
| Renames/deletions remain inspectable | Renamed content after commit, deleted and empty files, additions, unborn/index baselines, and lossless raw Unix filenames. |
| Async content stays guarded | Edits during capture preserve request-time text; later buffer mutations immediately reject obsolete review actions/source jumps. |
| Rendering remains source-independent | Narrow/paired TestBackend and PTY layouts plus source preservation. |

### Limits and manual verification

This run did not rerun real language servers, earlier performance examples,
release timing workloads, macOS, minimum Rust 1.88 or hosted CI. Existing real-server
observations remain separate evidence. No claim of atomic filesystem capture or
physical-terminal appearance is made.

Capture is all-or-nothing for supported text: 512 paths across current/retained
trees, 2 MiB per file, 8 MiB per tree, 32 checkpoints, 24 MiB retained source and
32 MiB serialized metadata. Binary/non-UTF-8 files, symlinks, submodules and
conflicts refuse capture. Capture has a cooperative 30-second Git deadline;
serialization/fsync and filesystem stalls are not hard-time bounded. There is one
full baseline per worktree store, with no pruning/reset UI. Ordinary annotation
validation still samples the current path and up to 31 others. Renames without
Git hints or unique exact content can remain additions/deletions, and changed or
ambiguous attachments require explicit correction.

On a disposable text repository, mark/note a hunk, run `:review snapshot`, then
`:review full`. Modify and commit another hunk externally. Verify that the first
unique hunk remains reviewed, the whole-file mark is stale, and `:review since`
shows only the newer revision. Capture again and restart; compare both checkpoint
IDs. Repeat with a dirty source, a rename/deletion, paired/narrow layouts, and a
save conflict. Check source cursor/scroll/undo and labels on a physical Linux and
macOS terminal, then assess responsiveness on your representative repository.

## M8 AI-assisted comprehension

2026-10-15: read the repository guidance, architecture and roadmap, then inspected
the Git/review/LSP/navigation implementations and matching tests. No provider or
conversation implementation existed. All **219 starting deterministic Rust tests**
passed before M8 changes. Implemented the next milestone with one Codex CLI path,
using existing source transactions, review Markdown, LSP positions and navigation.
No dependency or lockfile change was needed.

### Final automated evidence

Linux, Rust **1.99.0** and Cargo **1.99.0**. Default crates.io access failed DNS;
the existing `/tmp/editor-pro-m3-cargo` cache supported locked offline builds.

```sh
cargo fmt
cargo fmt --check
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo clippy --locked --all-targets -- -D warnings
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo test --locked --all-targets
CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true cargo build --locked
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-m8-terminal python3 tools/terminal_smoke.py
git diff --check
```

All commands pass. The Rust suite has **238 passing deterministic tests**, no
failures, and **seven ignored opt-in tests** (six real language-server fixtures
and one actual Codex fixture). New coverage comprises **18 AI tests** and **one
LSP/context integration test**; the existing editing, Git, review/snapshot,
navigation, rendering and LSP suites remain passing. Eight Python probe tests
and **24 PTY sessions** pass. The two new PTY sessions cover context preview,
approved revisions, per-buffer undo, dirty-conversation quit refusal, save/load,
historical edit rejection, follow-up context, narrow layouts and terminal
restoration. They use a clearly labelled CLI protocol double. Captures:

- `/tmp/editor-pro-m8-terminal/m8-ai-revisions.ansi`
- `/tmp/editor-pro-m8-terminal/m8-ai-resume.ansi`

An initial PTY resume assertion searched for a contiguous title even though
Ratatui emitted changed cells only. Forcing a redraw before that assertion fixed
the harness; all 24 sessions then passed. No application workaround was needed.

| Invariant | Evidence |
| --- | --- |
| Nothing is sent before explicit preview/send | Context tests use a missing executable and still capture/render; worker remains idle. The fake CLI checks read-only/tool/config arguments and receives context through stdin. |
| Source text/locations remain independent | Unsaved Unicode/CRLF selection/file context, narrow TestBackend renders, citations/history restoration, source/index byte assertions and PTY undo checks. |
| Context uses existing evidence | Positive Outline full-range, reference-line and version-labelled diagnostic checks; hunk/diff and persisted note Markdown attachment; note-draft isolation. |
| No stale or implicit revisions | Approval requires a separate edit preview. Editing/reload/close during a request, edits after preview, external disk changes and changed unopened targets reject before any splice. Restored suggestions have no live authority. |
| Multi-file updates preserve work | Complete validation before mutation, dirty unopened buffers, independent undo, whole-source fingerprints, exact unique text, overlapping occurrences/replacements and grapheme/CRLF guards. |
| Async failures stay contained | Partial structured output is visible but not actionable. Missing executable, malformed/oversized events, failed turns, tool attempts, invalid citations, nonzero exit, missing completion and update-only output fail safely. Timeout/cancellation kill/reap the process; obsolete results cannot replace a newer turn. |
| Persistence does not overwrite source | Outside-project destinations, private atomic writes, expected-content conflicts, no-clobber creation, corruption/symlink/hard-link refusal, raw Unix paths, and transcript follow-up without old edit authority. |

### Actual Codex acceptance

The installed **codex-cli 0.162.1** and the official exec/configuration documentation
were inspected before integration. [tools/ai/README.md](tools/ai/README.md) records
the binary SHA-256, exact subprocess arguments, official sources and reproduction.
The version is observed, not an automatically installed or updated dependency.

```sh
EDITOR_PRO_REAL_CODEX=/root/.local/bin/codex \
  CARGO_HOME=/tmp/editor-pro-m3-cargo CARGO_NET_OFFLINE=true \
  cargo test --locked --test ai_real -- --ignored --nocapture
```

The first sandboxed attempt stopped before inference: Codex could not initialize
its own state directory on the read-only filesystem. The same disposable fixture
passed with normal state-directory/network access and the child provider's
read-only sandbox unchanged. Existing ChatGPT authentication and the CLI default
model were used; no credentials were printed or stored in the repository.

The final run passed in **13.95 s**, with inference requests of approximately
**8.32 s** and **5.50 s**. It observed cited comprehension, a structured revision,
editor preview/approval, undo preserving prior unsaved text, unchanged disk bytes,
private transcript save, a fresh editor loading history, refusal to reapply an
old suggestion and a successful contextual follow-up. Valid message content
arrived before process exit and was displayed pending completion. A prior run
also passed in 13.00 s. These real-provider observations are separate from the
deterministic doubles; default `cargo test` never contacts a provider.

### Review findings, limitations and manual checks

Review retained four concrete responsibilities (context/schema, worker, file
store, view) and reused the existing final edit transaction. No generic provider
layer, runtime, command executor or second editor was introduced. Review fixed
overlapping-match ambiguity (`aa` in `aaa`), bounded review-feedback construction,
shared captured text across view rows, scanned grapheme boundaries once per edit
target, and required a completed answer item as well as turn/process success.

No release performance suite, real language servers, hosted CI, Rust 1.88,
macOS or physical-terminal checks were repeated for M8. The actual provider test
covers one small fixture/account, not every model, managed policy or CLI version.
Conversation save is explicit; unsent drafts are not saved. History is replayed
as bounded context rather than resuming a provider-side session. There is no
automatic project exploration, test execution, shell approval or file resource
operation. Diff/diagnostic/feedback citations have no direct source destination;
attach source when navigation/revisions are needed. Model semantics remain
unverified. The documented JSONL path supports progress/item updates, not a
guaranteed token stream.

Limits: 16 attachments/256 KiB/8,192 lines per turn, 64 KiB per attachment, 2 MiB
sources, 8 KiB questions, 32 turns, 1 MiB/32,768 prompt lines, 128 KiB answers and
128 replacements, 4 MiB transcripts, a cooperative 180-second deadline, 1 MiB
events, 4 MiB stdout and 256 KiB stderr. Source preparation/validation is bounded
but synchronous. File stalls and final persistence races with non-cooperating
writers are not hard-time or filesystem-transaction guarantees.

On a disposable project in a physical terminal, preview a file or Visual
selection, ask a contextual question and inspect source citations. Attach a
selected Outline symbol, references, diagnostics, a test file, a hunk and review
feedback. Request a small revision, inspect `:ai edits`, apply and undo it; edit
another target or change disk before applying and confirm refusal without partial
changes. Save/load the conversation and ask a follow-up. Test cancellation,
missing/login-failed Codex and narrow resizing, and confirm all core editing and
review features work without the provider. Repeat on macOS and larger real
conversations before extending the compatibility claim.

## Installation and distribution

Validated on Linux x86_64, 2026-10-15, with Rust/Cargo 1.99.0 and Python 3.12.
Rust commands used the existing `/tmp/editor-pro-m3-cargo` cache and
`CARGO_NET_OFFLINE=true`; dependencies and `Cargo.lock` were unchanged.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 245 passed; seven opt-in provider/server tests ignored. |
| `cargo build --locked` | Passed. |
| `python3 -W error -m unittest discover -s tools -p 'test_*.py' -v` | Three release-tool tests passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | Eight probe tests passed. |
| `sh tools/install-dev.sh /tmp/editor-pro-install-check` | Release build installed one executable and a relative `ep` symlink; `ep --version` printed `editor-pro 0.1.0`. |
| `cargo install --locked --path .. --root /tmp/editor-pro-install-check` from `docs/` | Passed; replaced the same binary and preserved the short command. |
| Development installer rerun | Passed with the existing `ep` symlink and byte-for-byte unchanged user configuration. |
| Existing-command collision checks | Installer refused an unrelated regular `ep` file and a dangling symlink without replacing either. |
| `python3 tools/terminal_smoke.py /tmp/editor-pro-install-check/bin/ep` | All 27 PTY sessions passed through the installed command. |
| `actionlint` 1.7.12 | Both CI and release workflows passed; YAML and each shell block also parsed separately. |
| `git diff --check` | Passed. |

New Rust coverage checks native macOS/Linux config paths, empty/relative XDG
fallbacks, absent home directories, optional defaults without creating files,
strict settings/size errors, unchanged user bytes, non-TTY help/version/path
reporting, malformed-config handling, the `ep` symlink and mixed-directory
argument errors. The PTY harness opens `ep .`, selects and edits Unicode/CRLF
text, opens an explicit directory containing spaces, resolves a relative `:e`
from that directory, and renders a theme loaded from isolated user settings.
The observed config session is captured at
`/tmp/editor-pro-install-captures/installation-config.ansi`.

The release-tool tests reject mismatched and prerelease tags, verify archive
layout/executable permissions and the relative symlink, require release
archives and check formula URLs/SHA-256 values. They use fixture executables,
not macOS binaries. One initial full PTY run exposed an existing raw-output
assertion that missed a headline split across ANSI diff writes; waiting for the
completed-turn status fixed the assertion, and the complete installed-binary
run then passed. No AI product behavior changed.

Created the public [Homebrew tap](https://github.com/perrness/homebrew-tap) and
published its installation README. A dedicated write deploy key is scoped to
that tap, with the private key stored as `HOMEBREW_TAP_DEPLOY_KEY` in the source
repository's Actions secrets. API reads confirmed the key's write setting and
the secret's presence; the temporary local key was removed. The first successful
tagged release will generate the installable formula from published assets.

The macOS builds, Homebrew installation/tests, release asset upload
and automatic tap update are configured but have not run on hosted Actions in
this change. No release tag or binary release was published. macOS 14 is the
deployment target; the workflow tests on macOS 15, so macOS 14 runtime behavior
still needs separate acceptance. Signing/notarization, Linux release archives,
Rust 1.88 and physical-terminal checks remain unverified or deferred. No
self-updater or background update check was added.

## Automatic language-server installation

2026-10-15, Linux x86_64. Implemented the approved per-user installer for the
existing Kotlin, Java and C# integrations. The executable enables automatic
installation by default; library editors and ordinary tests keep downloads off
unless a local fixture explicitly enables them. New dependencies provide HTTPS,
archive extraction and OS file locks; Cargo resolved them for Rust 1.88 compatibility.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 256 passed; seven opt-in real-provider/server tests ignored. |
| Final focused library/LSP/configuration tests | 58 passed after final discovery/Doctor refinements. |
| `cargo build --locked` | Passed. |
| Python tool and Kotlin probe unit tests | Six tool/harness tests and eight Kotlin probe tests passed. |
| `python3 tools/terminal_smoke.py` | All 28 PTY sessions passed, including installation controls and narrow resizing. |
| `git diff --check` | Passed. |

Cargo checks used `CARGO_HOME=/tmp/editor-pro-m3-cargo` and
`CARGO_NET_OFFLINE=true` after dependency provisioning. The sandbox initially
refused the local HTTP fixture's loopback listener; those tests passed with
normal localhost socket access. Ordinary tests do not contact external services.
The first PTY run exposed test settings being created inside a Git fixture,
changing the file-list selection. Moving all default smoke settings outside
project directories restored fixture isolation; the complete 28-session run passed.
The new terminal capture is
`/tmp/editor-pro-lsp-install-terminal/lsp-installation-controls.ansi`.

Eleven new deterministic tests cover native data paths, strict settings/defaults,
installation shared by two open projects, current unsaved Unicode/CRLF snapshots
at first startup, offline reuse, cancellation/explicit retry, checksum/network
failure cleanup, independent installers sharing an OS lock, disabled policy and
explicit overrides, old/failed/hanging runtime probes, archive traversal/escaping
links, supported internal symlinks, duplicate/truncated archives and platform recipes.
Installation status retains the latest update when the UI event queue is full.

### Actual downloads and server acceptance

Each first run started with its server absent from the managed directory and
used the editor's installer, without an argv override. Pinned package checksums
were verified before extraction/installation. These runs used existing SDKs and
cached project dependencies; they are not measurements of wholly cold project
dependency downloads.

| Fixture | Managed server | Result / elapsed test time |
| --- | --- | --- |
| `kotlin_maven` | Kotlin 263.6379.0, bundled JBR 25 | Passed, 117.67 s including download/extraction and restart. Project JAVA_HOME selected JDK 21, confirming separate server/runtime selection. |
| `java_maven` | JDT LS 1.60.0-202606262232, OpenJDK 25 | Passed, 38.18 s including installation and direct-Java launch with private configuration/data. |
| `csharp_project` | csharp-ls 0.28.0, .NET SDK 10.0.402 | Passed, 18.42 s. A fresh install after the final local NuGet-feed change also passed in 18.72 s. |

All three exercised actual diagnostics, completion, hover, navigation, symbols,
formatting, rename, edit previews/undo, source preservation and restart/resync with
unsaved content. Reports are under `/tmp/editor-pro-lsp-install-real` and
`/tmp/editor-pro-lsp-install-real-final`. Installations are isolated under
`/tmp/editor-pro-lsp-managed-bWrsPI`; no user's PATH, shell settings or editor
configuration was changed. Reproduce with `EDITOR_PRO_LSP_INSTALL_DIR` as described
in [tools/lsp/README.md](tools/lsp/README.md).

### Platform and operational limits

Official Kotlin SHA-256 files were fetched for Linux/macOS archives.
Range reads confirmed the Mac ARM64 `.sit` archive is a ZIP file; its central
directory/product metadata confirmed `bin/intellij-server` and
`jbr/Contents/Home/bin/java`, including relative JBR license symlinks. Native macOS
execution, Linux ARM64, minimum-Rust execution, hosted CI, physical-terminal
appearance and representative corporate proxies remain unverified here.

The installer provisions servers only. JDK 21+, .NET SDK 10+ and project-specific
build toolchains remain user prerequisites. Downloads have a cooperative ten-minute
budget and 1 GiB cap; tar/ZIP expansion is bounded to 100,000 entries/4 GiB.
Cancellation checks happen between chunks/files, with up to ten seconds for an
idle network read. Filesystem stalls and OS process scheduling are not hard-time
bounded. No Windows/musl installation, editor self-update, arbitrary package
registry, SDK installation or server update/removal commands were added.

### Release checks and fixture corrections

The v0.2.0 candidate passed the main-branch CI suite on Linux and macOS,
and native macOS release archives passed Homebrew installation
and all 28 installed-command PTY sessions. Its separate release CI run exposed
two intermittent fixture failures and did not publish a release:

- The local HTTP server closed immediately after writing its response, racing
  the client's socket setup/read and producing `Invalid argument (os error 22)`.
  It now consumes complete request headers and waits for the client to finish
  before closing, bounded by its existing socket timeout. A new test sends
  fragmented headers and verifies no response arrives before they are complete.
- The LSP error-scenario test applied a 150 ms initialization budget to all
  scenarios, including ones requiring a successful Python server startup.
  Only the intentional initialization-timeout scenario now uses that short budget;
  the others retain the fixture's normal two-second budget.

The v0.2.1 follow-up changes these test fixtures and the package version. Local
validation passed `cargo fmt --check`, Clippy with warnings denied, the debug
build, all **257 Rust tests** (seven opt-in tests ignored), the release version
check and `git diff --check`. The failed v0.2.0 tag is preserved; v0.2.1 runs the
release pipeline again. Native real-server installation on macOS remains outside
this CI suite; the Linux real-server evidence above remains separate.

The new fragmented-request regression then exposed that accepted sockets on
macOS inherit the listening socket's nonblocking mode. The fixture now makes each
accepted socket blocking before applying read/write timeouts, so a partial
request waits for the remaining bytes instead of closing on `WouldBlock`.
This is consistent with [Apple's accept documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/accept.2.html).
The v0.2.1 release run was cancelled before publication. Version v0.2.2 retains
this regression and the corrected fixture; its release tag is created only after
the main-branch CI suite passes.
The final socket correction passed all 12 local library tests, formatting,
Clippy with warnings denied, the debug build and the release version check.

## Completion while typing

2026-10-15, Linux x86_64. Insert-mode character edits now request completion after
120 ms of quiet and display a popup beside the cursor. Tab/Enter selects an item;
ordinary identifier replacements apply as one undoable edit. Additional edits,
multiline replacements and server commands retain the existing preview workflow.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 270 passed; seven opt-in tests ignored. |
| `cargo build --locked` | Passed. |
| Python tool/harness and Kotlin probe unit tests | All 14 passed. |
| `python3 tools/terminal_smoke.py` | All 29 sessions passed. |
| `git diff --check` | Passed. |

Thirteen new deterministic completion regressions cover debounce and startup,
server trigger characters, incomplete-list refresh, filtering/sorting, selection,
Tab/Enter acceptance, optional resolution, server replacement ranges, Unicode
graphemes/UTF-16 and CRLF, undo, disk conflicts, cancellation during requests and
resolution, empty/error/timeout results, unsupported capabilities, and additional
edit/command previews. Rendering checks cover a popup above/below the cursor,
scrolling through candidates, wide characters and tiny terminals. The new PTY
session types a prefix, resizes with suggestions open, accepts with Tab, undoes,
dismisses, and verifies source bytes and terminal restoration. Its capture is
`/tmp/editor-pro-autocomplete-terminal/automatic-completion.ansi`.

The three extended real-server fixtures also passed with the existing pinned
installations and cached project dependencies:

| Fixture | Final elapsed time | Automatic completion behavior |
| --- | --- | --- |
| `csharp_project` | 11.24 s | Popup, direct Tab acceptance and undo passed. |
| `java_maven` | 24.42 s | Popup, Tab acceptance through its required preview and undo passed. |
| `kotlin_maven` | 55.80 s | Popup, Tab acceptance through both command/edit previews and undo passed. |

These tests also retain explicit completion, diagnostics, navigation, rename,
formatting and restart checks. Source/build files remained unchanged on disk.
Reports and commands are in `/tmp/editor-pro-autocomplete-real`; the reproducible
fixture interface is documented in `tools/lsp/README.md`. Cargo used
`CARGO_HOME=/tmp/editor-pro-m3-cargo` and `CARGO_NET_OFFLINE=true`.

The sandbox initially blocked the installer tests' localhost HTTP sockets and
stalled the real C# project import; the final suites ran with normal socket/runtime
access. The new PTY assertion was corrected to tolerate split ANSI screen updates
before the full successful run. macOS and physical terminals were not tested.

Release preparation for v0.2.3 updated the Cargo package/lockfile version and
passed the CLI version/symlink tests, release packager tests, stable-tag validation,
formatting and whitespace checks. Native macOS acceptance is gated by the release
workflow before publication.

### Completion popup overlay fix

2026-10-15, Linux x86_64. The completion popup now clears its full rectangle before
painting its background, candidates and hint. A regression reproduced underlying
source characters appearing after suggestion labels before the fix. It now passes
for menus above and below the cursor, both selections, long Unicode/CRLF source
lines, cursor preservation and complete source-frame restoration on dismissal.
The PTY completion fixture also places populated source lines beneath the menu.

Formatting, Clippy with warnings denied, the debug build, all **271 Rust tests**
(seven opt-in tests ignored), all **14 Python tests**, and all **29 PTY sessions**
passed. The full Rust suite required localhost socket access for the existing
installer fixtures after the sandbox denied their binds. Cargo used the same
offline cache as the completion validation above. Terminal capture:
`/tmp/editor-pro-completion-overlay-terminal/automatic-completion.ansi`.
macOS and physical terminals were not tested.

## Code navigation shortcuts

2026-10-15, Linux x86_64. [Issue #2](https://github.com/perrness/editor-pro/issues/2)
adds Normal-mode `gd` for definitions and `gr` for references. A single local
result jumps directly; multiple results use the existing chooser. Jumps retain
ordinary buffers and navigation history, including `Ctrl-o`/`Ctrl-i` return and
forward navigation. Explicit `:Definition` and `:References` keep their lists.

Formatting, Clippy with warnings denied, the debug build, all **276 Rust tests**
(seven opt-in tests ignored) and all **14 Python tests** passed. Five new
regressions cover shortcut input routing, count reset, single and multiple
locations, `Location`/`LocationLink` responses, UTF-16/Unicode and CRLF, dirty
target reuse, source/viewports/history, empty results, unsupported capabilities
and virtual URIs, missing targets, and late replies after cancellation, movement,
edits, buffer switches or mode changes. The final focused navigation tests also
passed after strengthening history, command isolation and buffer-switch checks.

All **31 PTY sessions** passed, including two new sessions for single jumps and
multiple-result selection/cancellation with both shortcuts, narrow resizing,
back/forward navigation, unchanged source bytes and terminal restoration. Captures:
`/tmp/editor-pro-issue-2-terminal/navigation-normal.ansi` and
`/tmp/editor-pro-issue-2-terminal/navigation-navigation-multiple.ansi`.
Initial smoke checks exposed raw-output assertions missing split ANSI updates
or an unchanged frame after a same-size resize. Alternating terminal dimensions
before those assertions fixed the harness, and the full run then passed.

Cargo used `CARGO_HOME=/tmp/editor-pro-m3-cargo` and `CARGO_NET_OFFLINE=true`.
The full Rust suite needed localhost socket access for the existing installer
fixtures after the sandbox denied their listeners. Navigation tests use the
protocol double; real language servers, macOS and physical terminals were not
retested for this change.

## Repository-relative status paths

2026-10-15, Linux x86_64. [Issue #3](https://github.com/perrness/editor-pro/issues/3)
changes the source statusline to show paths relative to the Git worktree root
after asynchronous discovery. The cached name is tied to the inspected document
path and survives edits/refreshes. Before discovery or outside Git, the full path
remains available; unnamed buffers retain their label.

Formatting, Clippy with warnings denied, the debug build, all **280 Rust tests**
(seven opt-in tests ignored), all **14 Python tests**, and all **32 PTY sessions**
passed. Four new regressions cover the issue's example, nested repositories,
linked worktrees, buffer switches, save-as, nonexistent files, comparison errors,
repository removal, Unicode/control characters, narrow terminals and unchanged
file identity/CRLF bytes. The focused tests also passed after making the
buffer-switch assertion independent of worker completion timing.

The new PTY session launches from a repository subdirectory and checks the relative
path through editing, undo and resizing down to one column. Its capture is
`/tmp/editor-pro-issue-3-terminal/repository-relative-status.ansi`. The initial
assertion assumed contiguous bytes after a wide character; Ratatui emits a cursor
movement there, so the final assertion checks the prefix and filename separately.

Cargo used `CARGO_HOME=/tmp/editor-pro-m3-cargo` and `CARGO_NET_OFFLINE=true`.
The full Rust suite passed with localhost socket access after the sandbox denied
the existing installer fixtures' listeners. macOS and physical terminals were
not tested.

## Repeated undo and redo

2026-10-15, Linux x86_64. [Issue #4](https://github.com/perrness/editor-pro/issues/4)
adds a regression for three Insert sessions separated by reselecting the active
buffer. Before the fix, reselecting exited Insert without committing the open
transaction, so the first undo removed all three sessions. Context resets now
commit the existing buffer transaction before returning to Normal mode. The
existing history stacks and per-buffer ownership remain unchanged.

Formatting, Clippy with warnings denied, the debug build, all **283 Rust tests**
(seven opt-in tests ignored), all **14 Python tests**, and all **33 PTY sessions**
passed. Three Rust regressions cover repeated undo/redo and exhaustion, counted
undo, multiline operator changes, Unicode graphemes and CRLF, redo branching,
savepoint identity, disk preservation and independent per-buffer redo stacks.
The new terminal session sends three separate undo and redo keypresses, saves
and checks exact file bytes after every step, and checks narrow resizing and
terminal restoration. Capture:
`/tmp/editor-pro-issue-4-terminal/multi-step-undo-redo.ansi`.

The initial full build exhausted disk space and the linker crashed. Clearing
generated Cargo artifacts and rerunning with `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0` and two build jobs completed
successfully. macOS and real language servers were not retested locally.
Version v0.2.4 is prepared for the existing release workflow, which gates native
macOS assets and Homebrew publication on hosted acceptance checks.

## Keyboard file tree

2026-10-15, Linux x86_64. [Issue #5](https://github.com/perrness/editor-pro/issues/5)
adds a hidden contextual tree, toggled by Normal-mode Space p v or `:tree`.
Ctrl-p retains the fuzzy finder. Directory expansion uses one bounded worker;
file operations refuse paths belonging to open buffers. Creation and rename
never replace destinations; deletion requires `yes` and directories must be empty.

Formatting, Clippy with warnings denied, the debug build, all **293 Rust tests**
(seven opt-in tests ignored), all **14 Python tests**, and all **34 PTY sessions**
passed across the full existing terminal run and a focused rerun of the new tree
session. The first tree session raced expansion and selected the source file;
its attempted rename was correctly refused because that buffer was open. The
harness now waits for child rows before selecting. Its focused rerun passed
creation, folding, rename, deletion cancellation/confirmation, narrow resizing,
Space p v, unchanged Ctrl-p behavior, exact source bytes and terminal restoration.
Capture: `/tmp/editor-pro-issue5-terminal/file-tree.ansi`.

Ten Rust regressions cover keyboard routing and cancellation, original source
cursor/viewport and dirty text, nested Unicode/CRLF files, file/directory moves,
collisions, permissions and missing parents, open-buffer/ancestor protection,
closed-file navigation after rename, removed-file selection, symlinks replaced
after listing, control-character rendering, tiny terminals and the 10,000-entry
limit. A history regression exposed a trailing slash on exact file path updates;
the corrected update now reopens moved closed files through ordinary history.

Cargo used `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0` and
`CARGO_INCREMENTAL=0`. Tests ran with normal localhost socket access for existing
installer fixtures. macOS, real language servers and physical terminals were not
retested. Open paths must be closed before rename/delete; cross-filesystem moves
and recursive directory deletion are deliberately unsupported. Individual
filesystem calls remain synchronous and can stall on slow filesystems.

Release preparation bumps the package and lockfile to v0.2.5. The stable-tag
check, all three release-tool tests, all three CLI/version tests, formatting,
whitespace and an offline Cargo check passed before commit. Hosted release
checks gate native macOS archives and the Homebrew update before publication.

## Configured relative line numbers

2026-10-15, Linux x86_64. [Issue #6](https://github.com/perrness/editor-pro/issues/6)
extends the existing optional JSON configuration with
`editor.relative_line_numbers` (false by default). The source cursor line remains
absolute and surrounding rows show their distance from it. The existing gutter
width, Git/diagnostic markers, theme and LSP settings are retained; diff panes
continue to show source coordinates.

All **296 Rust tests** passed (seven opt-in tests ignored), including three new
regressions for settings compatibility/defaults, actionable boolean parsing
errors and rendering through movement, scrolling, 100,000-line Unicode/CRLF
buffers and tiny terminal resizing. A focused PTY run loaded Gruvbox and relative
numbers from the native configuration path, exercised movement and scrolling,
and verified exact CRLF source bytes and terminal restoration. Capture:
`/tmp/editor-pro-issue6-terminal.ansi`. macOS and physical terminals were not tested.

Formatting, Clippy with warnings denied and `git diff --check` also passed.

## One-step completion acceptance

2026-10-16, Linux x86_64. [Issue #7](https://github.com/perrness/editor-pro/issues/7)
makes Enter/Tab accept plain local completions from both the popup and explicit
`:Complete`/Ctrl-n list. Server replacement ranges may include punctuation;
UTF-16 conversion and the existing complete edit-plan, disk and freshness guards
still apply. Escape dismisses the popup without editing or leaving Insert.
Additional edits, multiline text and commands retain the guarded preview path;
Tab cannot approve those previews.

Formatting, Clippy with warnings denied, the debug build, all **301 Rust tests**
(seven opt-in tests ignored), and all **14 Python tests** passed. The **23 completion
regressions** cover both acceptance keys and lists, optional resolution and
documentation, selection, mode/cursor restoration, text and insert/replace edits,
emoji/combining text/CRLF, undo/redo, key fallback, disk conflicts, malformed
additional edits, command approvals and stale candidates/replies after typing,
buffer switches, reloads and document changes.

All **35 PTY sessions** passed. The new session exercises Up/Down selection,
Enter acceptance from the popup, Tab acceptance after explicit documentation,
Escape dismissal, ordinary Tab/newline fallback, undo, exact Unicode/CRLF saved
bytes, narrow resizing and terminal restoration. Its capture is
`/tmp/editor-pro-issue7-terminal/completion-acceptance.ansi`. The harness forces a
repaint before checking the acceptance message because incremental ANSI updates
can split that text across cursor movements.

All three pinned real-server fixtures passed using managed server installations:

| Fixture | Elapsed time | Explicit / popup completion previews |
| --- | --- | --- |
| `java_maven` | 30.07 s | 1 / 1; guarded approval retained. |
| `kotlin_maven` | 102.20 s | 2 / 2; command and subsequent server edit approvals retained. |
| `csharp_project` | 40.06 s | 0 / 0; Enter and Tab applied directly. |

These fixtures also check diagnostics, navigation, rename, formatting, restart,
undo and unchanged source/build files on disk. Reports are in
`/tmp/editor-pro-issue7-real`; servers are in `/tmp/editor-pro-issue7-servers`.
Java used the installed OpenJDK 25.0.4.1, Kotlin its bundled runtime, and C# the
checksum-verified .NET SDK 10.0.402 under `/tmp/editor-pro-issue7-dotnet`.
Cargo used `CARGO_NET_OFFLINE=true`; server installation/import used network access.
The broken sandbox could not initialize its loopback interface, so commands ran
with normal process/socket access. macOS, physical terminals and the larger
Gradle/solution fixtures were not retested.

Release preparation bumps the Cargo package and lockfile to v0.2.7. The stable
tag check, all three CLI/version tests, all three release-tool tests, an offline
Cargo check, formatting and whitespace checks passed. The tag-triggered release
workflow gates native macOS archives and Homebrew publication on hosted checks.

## Repository language-server preparation

2026-10-16, Linux x86_64. [Issue #14](https://github.com/perrness/editor-pro/issues/14)
starts bounded project detection after the first frame for directory/no-argument
launches. Up to two likely sessions use the existing LSP manager; matching sources
claim them without another initialize/process. Explicit file launches remain lazy.
`lsp.warmup` defaults to true and can be disabled independently of installation.

Formatting, Clippy with warnings denied, the debug build, all **317 Rust tests**
(eleven opt-in tests ignored), all **14 Python tests**, and all **37 PTY sessions**
passed. New regressions cover Gradle Kotlin DSL versus source languages, mixed and
nested projects, Maven/solution roots, overrides, ambiguity, scan limits and
symlinks; initialization/file-opening races, Unicode/CRLF unsaved synchronization,
cancellation/replacement, installation ownership/retry, missing executables, crash
recovery, documentless shutdown and eviction at the eight-session limit.

The terminal sessions launch both `editor-pro .` and with no arguments, hold the
server's initialize response, inspect preparation status, open/edit/resize before
releasing initialization, and verify one initialize, current unsaved text, unchanged
disk bytes and terminal restoration. Captures:
`/tmp/editor-pro-issue14-terminal/lsp-warmup-directory.ansi` and
`/tmp/editor-pro-issue14-terminal/lsp-warmup-cwd.ansi`.

The initial full Rust run exposed queued membership events restoring a closed
session's UI state. Membership is now filtered against current routes and warm
sessions; the focused regression and complete rerun passed. Concurrent session
tests also required locking the protocol double's process counter. The first
full PTY run exposed an existing footer assertion split by ANSI cursor movements;
matching the completion view title fixed the assertion and the complete rerun passed.

### Real-server latency investigation

The new opt-in `lsp_startup_real` suite measures the first definition response
pointing to the expected other source file. It records failed/empty attempts,
initialization, nonempty diagnostics when observed, session epochs and startup
phase logs. Each fixture alternates lazy/warm/warm/lazy with fresh project copies
and processes and a ten-second browsing interval in both modes. Reports record
latency from launch as well as file opening. JDT workspace indexes are fresh;
dependency/SDK caches are shared. These are small fixture measurements with two
samples per mode, not large-project or empty-cache download guarantees.

All four fixtures passed in the initial run (sixteen successful sessions, each
with one session epoch). In that exploratory run, executable discovery rounded
to 0.000 s, runtime probing took 0.032–0.282 s, process spawning 0.000–0.003 s,
and initial document synchronization 0.000–0.002 s. Initialize responses took
0.57–0.64 s for C#, 6.45–8.94 s for Java and 4.48–8.46 s for Kotlin, while useful
definitions arrived later. The first Java Gradle import took 78.98 s after file
open: progress showed Gradle API JAR generation, Kotlin DSL compilation and model
import after initialize had already completed. Later lazy startup took 16.57 s.
Those cache/setup effects are recorded separately from preparation benefits.
The initial reports are preserved in `/tmp/editor-pro-issue14-real`.

The repeat comparison ran after Rust/Python/PTY/lint work finished, with the
same installed distributions and populated dependency caches. All four tests
passed in 360.89 s, covering sixteen useful definitions and one session epoch per
run. The two observed samples per mode give these ranges, in seconds **after
file opening**, with ten seconds of browsing before opening in both modes:

| Real fixture | Lazy startup | Background preparation |
| --- | --- | --- |
| Kotlin multi-module Gradle | 39.49–42.93 | 27.82–29.72 |
| Java Maven | 7.10–11.20 | 0.72–1.01 |
| Java multi-module Gradle | 9.09–9.46 | 0.69–0.70 |
| C# multi-project solution | 3.74–4.13 | 3.10–3.30 |

Add approximately ten seconds to each figure for elapsed time from launch.
These measure protocol responses through the production manager, with terminal
responsiveness checked separately above. Final reports, including phase logs,
attempts and fixture fingerprints, are in `/tmp/editor-pro-issue14-real-final`.

Preparation spends
CPU and memory earlier, even if a source is never opened, so it remains optional
and capped at two candidates. It cannot remove import/indexing work, and an
immediate file open offers little opportunity to hide that work. The C# runs in
particular still take several seconds after opening despite early initialization.

Managed distributions were reused from `/tmp/editor-pro-issue7-servers`: Kotlin
263.6379.0 with bundled JBR 25, JDT LS 1.60.0 with installed OpenJDK 25, and
csharp-ls 0.28.0 with .NET SDK 10.0.402 in `/tmp/editor-pro-issue7-dotnet`.
Gradle fixtures use 9.1.0 and the installed JDK 21 project toolchain. No server
download was needed; installation cancellation/retry is covered separately with
local HTTP fixtures. Reproduction is documented in `tools/lsp/README.md`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands used normal
process/socket access because the sandbox could not initialize its loopback
interface. macOS, physical terminals, uncached downloads and large production
repositories were not tested. Filesystem stalls remain outside hard deadlines.

Release preparation bumps the Cargo package and lockfile to v0.2.8. The stable
tag check, all three CLI/version tests, all three release-tool tests, an offline
Cargo check, formatting and whitespace checks passed. The tag-triggered release
workflow gates native macOS archives and Homebrew publication on hosted checks.

## Issue 8 — Git keyboard navigation

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. Normal-mode `]c`/`[c`
navigate hunks, `]f`/`[f` navigate changed files, and `Space g d`/`Space g c`
open the existing diff/Changes views. Prefix hints, `:help`, CLI help and README
document the bindings. No dependency, diff algorithm or Git worker was added.

Nine new regressions cover first-changed-line coordinates instead of context
headers; counts, boundaries and file wraparound; HEAD/index/revision comparisons;
unsaved Unicode/CRLF content; deletion and EOF anchors; deleted-file history;
buffer, cursor, viewport and undo preservation; source location history;
paired/narrow rendering; empty/error feedback; prefix cancellation and ordinary
Insert, Visual, operator, text-object and find input. Freshness checks reject
buffer/base changes before the next worker tick and report pending refreshes
without queuing a later jump. The Changes list retains its existing disk/index
and separately labeled unsaved-buffer semantics.

Formatting, Clippy with warnings denied, the debug build, all **326 Rust tests**
(eleven opt-in tests ignored), and all **14 Python tests** passed:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
```

All **38 PTY sessions** passed with the debug binary, including a new keyboard
workflow that jumps forward by two hunks, moves backward, opens a diff, browses
another file, resizes through paired/narrow/tiny layouts, then edits the restored
source line. Existing dirty-origin and changed-file sessions now use the new
shortcuts. The fixtures check saved bytes, retained unsaved text and the unchanged
Git index, along with terminal restoration. Reproduction and captures:

```sh
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue8-terminal python3 tools/terminal_smoke.py
```

- `/tmp/editor-pro-issue8-terminal/git-keyboard-navigation.ansi`
- `/tmp/editor-pro-issue8-terminal/git-shortcut-dirty-origin.ansi`
- `/tmp/editor-pro-issue8-terminal/git-shortcut-changed-files.ansi`

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process/socket access because the sandbox failed to initialize its loopback
interface. macOS, physical terminals and real language servers were not retested
for this keyboard/navigation change.

Release preparation bumps the Cargo package and lockfile to **v0.2.9**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

## C# generated and decompiled definition navigation

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. The client enables
csharp-ls metadata URIs and fetches `csharp:/` definition/reference targets using
`csharp/metadata`. These open as read-only, syntax-highlighted buffers with
original URI/session routing, UTF-16 positions and ordinary navigation history.
They have no disk path, cannot be edited/saved, and never become editable server
documents. Repeated jumps refresh generated output in its existing buffer slot.

Eight new deterministic regressions cover generated/decompiled Location and
LocationLink targets, Unicode/CRLF, explicit and multiple-result lists, further
navigation with original encoded URIs, source-buffer closure, dirty-source
preservation, read-only keys/core mutations/save rejection, buffer reuse/refresh,
restart epochs, Help deferral, rendering, empty/error/oversized/NUL/invalid-position
responses, timeouts and late replies after cancellation/movement/edits/switches.
Retained virtual buffers cannot prevent fresh source diagnostics after restart.
Existing unsupported schemes still fail explicitly.

| Command/check | Result |
| --- | --- |
| `cargo fmt --check`, `git diff --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed after final changes. |
| `cargo test --locked --all-targets` | **369 passed**, zero failed, 13 opt-in tests ignored. |
| `cargo build --locked` | Passed; rebuilt debug executable. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | **8 passed**. |
| `python3 tools/terminal_smoke.py` | All **46 PTY sessions** passed, including the added C# session. The old printed count was corrected afterward. |
| Final focused C# PTY rerun | Passed with the rebuilt executable: virtual jumps, `[RO]`, edit refusal, back navigation, unchanged disk bytes and terminal restoration. |
| `cargo test --locked --test lsp_real csharp_ -- --ignored --nocapture --test-threads=1` | All **3 real C# fixtures passed** in 36.05 seconds. |

Actual csharp-ls **0.28.0** with .NET SDK **10.0.402** resolved `StringBuilder`,
its `Append` members, a custom generator's `GeneratedWidget` class and `Message`
method, and a further jump from decompiled `StringBuilder` to `ISerializable`.
The new temporary fixture builds its analyzer from SDK Roslyn references, with
no third-party NuGet dependencies. Returned declaration lines, read-only state,
jump-back cursor restoration and unchanged original fixture bytes are asserted.
The existing project and multi-project solution fixtures also passed their
editing/completion/diagnostics/rename/restart workflows with metadata enabled.

The first constructor probe returned the server's documented file-start fallback.
Explicit type references resolved to declarations. The `Append` call returned
multiple overload locations, so acceptance exercises the chooser; exact overload
and constructor precision remains a server limitation, not proof of universal
symbol resolution. Project import, dependency resolution and source-generator
builds still determine which symbols the server can provide. Kotlin/Java virtual
sources and virtual workspace-symbol/call-hierarchy navigation remain unsupported.
macOS, physical terminals, other C# server implementations and the user's project
were not tested.

Reports and captures:

- `/tmp/editor-pro-csharp-metadata-reports/csharp-metadata.json`
- `/tmp/editor-pro-csharp-metadata-reports/csharp-project.json`
- `/tmp/editor-pro-csharp-metadata-reports/csharp-solution.json`
- `/tmp/editor-pro-csharp-terminal/csharp-metadata-final.ansi`
- `/tmp/editor-pro-csharp-final-tests.log`

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Actual servers used the
existing `/tmp/editor-pro-issue7-servers` installation and SDK under
`/tmp/editor-pro-issue7-dotnet`, with a temporary CLI home. Normal process/socket
access was required because the filesystem sandbox could not initialize loopback.

Release preparation bumps the Cargo package and lockfile to **v0.2.15**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

## Issue 9 — Searchable keyboard help

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. F1 opens a read-only
help overlay in any mode/view; `:help [QUERY]`, `:ai help` and `:review help`
provide command access. The categorized reference covers editing, files,
language navigation/diagnostics, Git, review notes and Codex questions. Normal
definition/reference and leader commands share metadata with the reference.
No dependency, external lookup or document copy is needed to open help.

Seven new help tests cover representative bindings, case-insensitive multiword
search, category cycling, paging, query bounds and Unicode grapheme deletion;
narrow wrapping and zero/one-cell layouts; read-only keys/paste; all editing
modes, pending operators, selections, search, viewport and Insert undo groups;
and buffer/file-picker selection and file-tree prompt restoration. A new LSP
regression checks that a late definition response stays deferred under help and
is handled after dismissal. Existing Git/LSP/review tests now check preservation
of diff/list selections and note drafts, including a note save that becomes
durable while help is open. Workers continue running, but their results cannot
move the underlying context until help closes.

Formatting, Clippy with warnings denied, the debug build, all **334 Rust tests**
(eleven opt-in tests ignored), and all **14 Python tests** passed:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
```

All **39 PTY sessions** passed. The new workflow opens help during Insert, finds
definition, Git diff and Codex question bindings, cycles categories and resizes
through narrow/one-cell layouts, then resumes at the original cursor. One undo
removes the text typed both before and after help; saved CRLF/Unicode bytes and
terminal restoration are checked. Reproduction and capture:

```sh
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue9-terminal python3 tools/terminal_smoke.py
```

- `/tmp/editor-pro-issue9-terminal/keyboard-help.ansi`
- `/tmp/editor-pro-issue9-rust-tests.log`
- `/tmp/editor-pro-issue9-terminal-tests.log`

Final review corrected the reference's inherited Normal/Visual label for
half-screen movement to match its actual Normal-mode handler; help and CLI
regressions, formatting, Clippy and the debug build passed again afterward.
The early help assertion was adjusted for wrapped text, and the note-save test
waits for on-disk durability rather than expecting a deferred UI reply to apply.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process/socket access because the sandbox could not initialize its loopback
interface. macOS, physical terminals and real language servers/providers were
not retested for this help/input change.

Release preparation bumps the Cargo package and lockfile to **v0.2.10**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

## Issue 10 — Safe external file refresh

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. Background disk checks
replace the main loop's synchronous reads. One worker samples at most 16 named
buffers per rotating batch every two seconds, with early focus checks. Content
observations update `!DISK` and invalidate Git/review snapshots without opening
prompts or replacing text. The source retained behind a review note is included.

`:refresh` compares retained text against a stable disk sample. Reload is explicit;
dirty text requires a separate confirmation. Cancel preserves the original context
and undo group, including cancellation after confirmation while I/O is pending.
Reload rechecks the compared bytes, document identity, baseline and mutation
generation. Replies deferred behind Help are checked again. Successful reloads
preserve line/column and viewport where possible; missing or unreadable files
retain the source for save-as. The existing `:e` reload also retains position and
refuses to empty a document whose file disappeared.

All **344 Rust tests** passed (eleven opt-in tests ignored), including nine new
disk regressions and a review regression. These cover restored-mtime same-size
writes, rapid atomic replacements, clean/dirty buffers, inactive-buffer rotation,
save conflicts/save-as, reload/save races, cancellation, Help, Unicode/CRLF and
shorter files, invalid UTF-8/NUL content, symlinks, deletion/rename/recreation,
tiny-terminal rendering, and repeated Git/review invalidation while a conflict
is already flagged. Existing save and undo tests remain intact. The focused disk
and snapshot suites also passed after the final rendering and dependency cleanup.

Formatting, Clippy with warnings denied, whitespace checks, the debug build,
all **14 Python tests**, and all **41 PTY sessions** passed:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue10-terminal python3 tools/terminal_smoke.py
```

The two new terminal workflows verify dirty comparison/reload/cancel, save refusal,
a newer disk version rejecting an approved reload, narrow/tiny resizing, Unicode
and CRLF bytes, deletion recovery through save-as, and terminal restoration.
An initial harness assertion missed a ready message split across incremental ANSI
writes. Retrying a repaint, without sending editing/approval keys, fixed the
assertion; both focused workflows and the complete terminal run then passed.

- `/tmp/editor-pro-issue10-terminal/disk-refresh-conflict.ansi`
- `/tmp/editor-pro-issue10-terminal/disk-refresh-deletion.ansi`
- `/tmp/editor-pro-issue10-rust-tests.log`
- `/tmp/editor-pro-issue10-focused-tests.log`
- `/tmp/editor-pro-issue10-terminal-tests.log`

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands used normal
process/socket access because the sandbox could not initialize its loopback
interface. macOS, physical terminals and real language servers/providers were
not retested. Comparisons reuse Git's 2 MiB-per-side limit; valid text can still
reload up to the 64 MiB document limit. Filesystem stalls have no hard deadline,
and matching samples cannot exclude later concurrent writes. Exact save-baseline
checks remain in place. Ordinary open/save and edit-approval disk checks retain
their existing synchronous behavior; this change moves polling and `:refresh`
I/O off the input/render loop.

Release preparation bumps the Cargo package and lockfile to **v0.2.11**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

## Issue 11 — Contextual Codex explanation shortcuts

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. `Ctrl-g` prepares an
explanation preview from a Visual selection, selected Outline symbol, or active
Git hunk. In Normal mode it requests the smallest containing symbol through the
existing LSP worker. `:ai explain [QUESTION]` provides command access, and
`:ai ask QUESTION` customizes the draft or asks a follow-up. Only `:ai send`
starts Codex. The existing read-only provider invocation and separate
`:ai edits`/`:ai apply` approval remain unchanged.

Previews show the repository/project, exact source range, content fingerprint,
and Git comparison provenance. Captured hunks include full base/blob/content
identities and retain a runtime identity check before sending. Matching new-side
source excerpts support ordinary source links; differing staged/historical
content and deleted files stay diff-only evidence. Default questions distinguish
supported behavior from speculation about an author or agent's intent.

All **354 Rust tests** passed (eleven opt-in tests ignored), including seven new
AI regressions and three LSP regressions. Coverage includes exact unsaved
Unicode/CRLF selections, control-character-safe previews, zero/one-cell layouts,
stale selection anchors and edit/undo generations, containing versus selected
symbols, cancelled or obsolete symbol replies, exact hunk scope and revision
changes, nested-project repository roots, deleted files, staged/unsaved separation,
custom follow-ups, provider failure/cancellation, source links/history, and
cursor/viewport/diff-row restoration. Existing edit-approval and undo tests pass.

Formatting, Clippy with warnings denied, whitespace checks, the debug build,
and all **14 Python tests** passed:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
```

All **43 PTY sessions** passed, including the two new workflows with the Codex
protocol double. Reproduction:

```sh
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue11-terminal python3 tools/terminal_smoke.py
```

The new workflows exercise
selection and hunk shortcuts, provenance previews, custom questions, answers,
narrow resizing, source/diff restoration, undo, unchanged CRLF/Unicode source and
Git index bytes, and terminal restoration. Captures and logs:

- `/tmp/editor-pro-issue11-terminal/ai-explain-selection.ansi`
- `/tmp/editor-pro-issue11-terminal/ai-explain-hunk.ansi`
- `/tmp/editor-pro-issue11-rust-tests.log`
- `/tmp/editor-pro-issue11-terminal-focused.log`
- `/tmp/editor-pro-issue11-terminal-tests.log`

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process/socket access because the sandbox could not initialize its loopback
interface. macOS, physical terminals, real language servers and the actual Codex
provider were not retested. Current-symbol lookup requires a ready supported
language server; selection and hunk capture do not. Git checks use the existing
bounded sampled comparisons and cannot exclude an external write after a sample;
captured evidence retains its exact revision identity.

Release preparation bumps the Cargo package and lockfile to **v0.2.12**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

The v0.2.12 hosted run passed Linux checks, native macOS builds, and the
Homebrew installation/terminal suite. The macOS debug terminal suite exposed a
timing assumption in the new hunk workflow: review loading invalidated the first
Git snapshot just before Ctrl-g, and the editor correctly refused stale context.
Publication was skipped. The harness now retries only that read-only shortcut
until the preview opens; Ctrl-g cannot send from the AI view. Both focused PTY
workflows passed again locally, along with all six release/harness Python tests,
all three CLI/version tests, the stable-tag check, offline Cargo check, formatting
and whitespace checks. Their log is `/tmp/editor-pro-v0.2.13-terminal-focused.log`.
The corrected release uses **v0.2.13**, preserving the existing v0.2.12 tag.


## Issue 13 — JVM startup and indexing visibility

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. The existing session
manager now reports Starting, Indexing, Ready, Degraded and Failed explicitly.
Concurrent numeric/string work tokens retain independent progress; initialization
and unrelated task completions cannot erase ongoing imports. Only server-provided
percentages are displayed. Elapsed state time remains live during quiet startup.
Typed server errors/warnings and JDT status failures remain actionable until an
explicit restart. LspInfo, live LspLog, Doctor and searchable help expose the
troubleshooting path. Crash retries remain capped; failed sessions do not restart
on edits. Capabilities/diagnostics are invalidated before process teardown, and
explicit restart rejects queued results from the old epoch.

All **361 ordinary Rust tests** passed, including seven new lifecycle/UI tests.
Coverage includes delayed initialization, overlapping tasks before/after initialize,
unknown/partial-result progress, numeric versus string tokens, retained titles,
server percentages, bounded oversized progress, elapsed redraws, typed import/SDK
errors, Java status notifications, missing executables, repeated crashes, retry
limits, explicit recovery, a full UI event queue, Unicode/CRLF unsaved text and
undo, tiny layouts and control-safe rendering. Existing runtime checks cover old,
failed and hanging SDK processes. All-target Clippy passed with warnings denied;
the focused restart tests also passed after the final command-submission guard.
All **14 Python tests** and **45 PTY sessions** passed. Commands:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --test lsp restart
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue13-terminal python3 tools/terminal_smoke.py
```

The two new terminal sessions exercise missing-server failure, Starting/Indexing,
concurrent import tasks, Degraded errors, Doctor/help/log discovery, explicit
restart, unsaved resynchronization, undo, tiny resizing and terminal restoration.
The capture harness needed the existing alternating-width repaint technique to
match messages split across incremental ANSI writes; the complete run then passed.

- `/tmp/editor-pro-issue13-terminal/lsp-startup-recovery.ansi`
- `/tmp/editor-pro-issue13-terminal/lsp-startup-missing.ansi`
- `/tmp/editor-pro-issue13-rust-tests.log`
- `/tmp/editor-pro-issue13-restart-tests.log`
- `/tmp/editor-pro-issue13-terminal-tests.log`

### Actual JVM server evidence

Three opt-in tests passed against installed pinned servers, using fresh temporary
two-module Gradle projects and cached SDK/distribution/dependency artifacts:

| Test | Server/runtime | Observations |
| --- | --- | --- |
| `kotlin_gradle` | Kotlin 263.6379.0, bundled JBR 25; project JDK 21 | Passed in 98.52 s. Observed Starting, Indexing and Ready; first type diagnostic 78.89 s; unsaved restart/resync 6.34 s. |
| `java_gradle` | JDT LS 1.60.0, OpenJDK 25; project JDK 21 | Passed in 33.46 s. Observed Starting, Indexing and Ready; first type diagnostic 13.89 s; unsaved restart/resync 11.48 s. |
| `kotlin_gradle_import_failure_and_recovery` | Same Kotlin server and project toolchain | A deliberate GradleException in the temporary root build produced Degraded with “Unable to import a Gradle project” after 13.92 s. Restoring the build and restarting restored Ready and fresh diagnostics in 39.26 s while retaining unsaved source. Passed in 53.72 s. |

The healthy tests retain completion, hover, navigation, symbols, formatting,
rename, edit approvals/undo and unchanged-disk assertions. The failure test restores
the build file and checks all original fixture bytes. Reports capture status
transitions, progress, initialization/capability availability and failure messages:

- `/tmp/editor-pro-issue13-real/kotlin-gradle.json`
- `/tmp/editor-pro-issue13-real/java-gradle.json`
- `/tmp/editor-pro-issue13-real/kotlin-gradle-import-failure.json`

The final import-failure/recovery rerun, including the assertion that an unrelated
warning cannot satisfy the failure check, passed in 43.48 s. Its report is
`/tmp/editor-pro-issue13-real-final/kotlin-gradle-import-failure.json`.

The Kotlin initialize response arrived at 7.94 s; reported import started at
8.25 s, before later indexing tasks. Ready therefore means protocol readiness
with no currently reported work, not complete project indexing. The observed
maximum editor tick was 4.66 ms in the Kotlin run and 3.12 ms in Java. These are
small fixture observations, partly concurrent with other checks, not latency or
large-project guarantees. The two Kotlin tests can be reproduced with:

```sh
JAVA_HOME=/usr/lib/jvm/java-21-openjdk-amd64 \
EDITOR_PRO_LSP_INSTALL_DIR=/tmp/editor-pro-issue7-servers \
EDITOR_PRO_LSP_REPORT_DIR=/tmp/editor-pro-issue13-real \
cargo test --locked --test lsp_real kotlin_gradle -- --ignored --nocapture --test-threads=1
```

Use `java_gradle` and the installed JDK 25 runtime for the Java reproduction.
The new failure test is opt-in; ordinary tests never require real servers or
project dependency downloads. See tools/lsp/README.md for provisioning elsewhere.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process/socket access because the sandbox could not initialize its loopback
interface. macOS, physical terminals, empty dependency caches, large production
projects and real crash injection were not tested. Crash/retry limits and missing
runtime behavior are covered with deterministic process doubles. Server messages
and progress are bounded observations; unreported indexing cannot be inferred.

Release preparation bumps the Cargo package and lockfile to **v0.2.14**. The
stable-tag check, all three CLI/version tests, all three release-tool tests,
offline Cargo check, formatting and whitespace checks passed. The tag-triggered
release workflow gates publication and the Homebrew tap update on hosted checks.

## Issue 15 — ASCII startup splash

Validated on Linux x86_64, 2026-10-16, with Rust 1.99.0. Launching without paths
shows a centered, themed ASCII wordmark, package version and working shortcut
hints. Small terminals use compact text. The first decoded key or paste dismisses
the splash and follows the existing editor behavior; file/directory arguments
open directly. Background preparation, status messages and terminal restoration
retain their existing paths. The artwork never becomes document text.

All **372 ordinary Rust tests** passed (13 opt-in tests ignored), including three
new rendering regressions for all palettes, full/compact/zero-cell resizing,
an untouched empty buffer and control-safe background errors. All **14 Python
tests**, formatting, Clippy with warnings denied and the debug build passed.
All **55 PTY sessions** passed. The nine new sessions cover immediate Insert,
Unicode paste, undo/redo, exact saved bytes, focus/resize, hidden splash cursor,
F1 help, Ctrl-p, Normal-mode paste rejection, immediate `:q`, no reappearance
after `:enew`, and direct existing-file/new-path/directory/`-- FILE` startup.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue15-terminal python3 tools/terminal_smoke.py
```

Terminal captures are `/tmp/editor-pro-issue15-terminal/splash-{editing-resize,help,picker,paste,quit}.ansi`.
The reconstructed initial frame is also saved as `splash.txt` and `splash.svg` in
that directory. Full logs are `/tmp/editor-pro-issue15-rust-tests.log` and
`/tmp/editor-pro-issue15-terminal-tests.log`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands needed normal
process access because the sandbox failed to initialize its loopback interface.
macOS and physical-terminal appearance were not tested; real language servers
and AI providers were not retested for this startup presentation change.

Release preparation bumps the Cargo package and lockfile to **v0.2.16**. The
stable-tag check, all three CLI tests, all eleven rendering tests, all three
release-tool tests, offline Cargo check, formatting and whitespace checks passed
again. The tag-triggered workflow gates publication and the Homebrew tap update
on hosted checks.

## Issue 19 — ARM64 macOS builds and releases

Validated on Linux x86_64, 2026-10-16, with Rust/Cargo 1.99.0 and Python 3.12.
All workflow jobs now use only `ubuntu-latest` or regular `macos-latest`.
macOS release packaging, Homebrew installation and managed language-server
recipes target ARM64. `AGENTS.md` records the runner and platform restrictions.

| Check | Result |
| --- | --- |
| `actionlint` 1.7.12 | Both workflows passed. |
| YAML runner validation and `bash -n` | Only permitted runners; all workflow shell blocks parsed. |
| `python3 -W error -m unittest discover -s tools -p 'test_*.py' -v` | All six release/harness tests passed. |
| Release CLI fixture | Version check, packaging, local/published formula generation and `SHA256SUMS` passed with only the ARM64 archive. |
| `cargo test --locked --lib lsp_install::tests` | All 11 installer tests passed. |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `git diff --check` | Passed. |

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process access because the sandbox failed to initialize its loopback interface.
The release CLI check used a fixture executable; hosted macOS builds, Homebrew
execution and release publication were not run for this change.

## Issue 16 — Whole-directory status paths

Validated on Linux x86_64, 2026-10-16, with Rust/Cargo 1.99.0 and Python 3.12.
Long statusline paths now drop complete outer directories only when they exceed
the available cells. The issue's example shows
`Gjensidige.Alarm.Bff.Notifications/Functions/NotificationOutboxFunction.cs`
without a fragment of its outer directory. Wider terminals restore the longer
path; an overlong filename retains the existing grapheme-safe tail clipping.

The new example regression and expanded Unicode regression failed with the
previous renderer. Coverage includes dirty buffers, pending/completed Git
refreshes, CRLF bytes, undo, Unicode directory names, combining/emoji graphemes,
control characters, and widths from one to 160 columns. The existing repository,
linked-worktree, buffer-switch and save-as regressions also pass. Rendering keeps
the original document identity and performs no filesystem work.

| Check | Result |
| --- | --- |
| `cargo fmt --check` and `git diff --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 373 passed; 13 opt-in real-service tests ignored. |
| `cargo build --locked` | Passed. |
| `python3 -m unittest discover -s tools -p 'test_*.py' -v` | All six tests passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | All eight tests passed. |
| `python3 tools/terminal_smoke.py` | All 56 PTY sessions passed, including the new long-path edit/undo/resize session. |

The terminal suite used
`EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue16-terminal`. The new capture
is `shortened-status-path.ansi`; a reconstructed 195-column frame is saved beside
it as `shortened-status-path.txt` and `shortened-status-path.svg`. Full Rust and
PTY logs are `/tmp/editor-pro-issue16-rust-tests.log` and
`/tmp/editor-pro-issue16-terminal-tests.log`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process access because the sandbox failed to initialize its loopback interface.
macOS, physical terminals, real language servers and AI providers were not
retested for this display-only change.

Release preparation updates the Cargo package and lockfile to **v0.2.17**.
Offline Cargo check, all three CLI tests, all eleven rendering tests, all three
release-tool tests, the matching-tag check, formatting and whitespace checks
passed after the version bump. The tag-triggered workflow gates publication and
the Homebrew tap update on its hosted checks and installed-binary acceptance.

The first hosted run blocked **v0.2.17** before publication: runners configure
Git filters, adding `[filters off]` to the statusline and leaving less path space
than the new regression assumed. The renderer correctly dropped another directory.
The Rust and PTY fixtures now configure a filter and an unavailable LSP explicitly;
their widths include the filter label, and the PTY waits for the LSP failure
before checking a width that also remains stable through Git refreshes.

The correction prepares **v0.2.18**, preserving the earlier tag. All 55 focused
Git/CLI/rendering tests, all-target Clippy, the six release/harness tests, the
debug build, version, formatting and whitespace checks passed. All 56 terminal
scenarios completed across the main run and a focused retry: an existing
file-deletion marker timed out once, then both disk-refresh sessions and the
remaining AI sessions passed without further source changes. Recovery logs are
`/tmp/editor-pro-issue16-recovery-{tests,clippy,terminal-tests,terminal-retry}.log`;
captures are in `/tmp/editor-pro-issue16-recovery-terminal`.

## Issue 17 — Colorful dark interface

2026-10-16, Linux x86_64, Rust/Cargo 1.99.0. All three existing dark themes now
provide darker navigation panels and brighter secondary text. The file tree,
picker and buffer list share accented headers, file-type text badges and a
selection rail. The tree adds nesting guides, an active-source marker, entry
counts and colored shortcut hints. Source status segments distinguish language,
Git/LSP state, dirty buffers and position. Tree-sitter colors remain unchanged.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 375 passed; 13 opt-in tests ignored. |
| `cargo build --locked` | Passed. |
| Python tool/harness and Kotlin probe unit tests | All 14 passed. |
| `python3 tools/terminal_smoke.py` | All 58 sessions passed. |
| `git diff --check` | Passed. |

Three new Rust regressions exercise tree selection and the active-file marker
across palettes, deep Unicode/control-character filenames, prompt clipping and
cursor bounds, and picker/buffer selection with unchanged source, CRLF bytes and
undo history. Existing theme tests now check status segment colors as well as
syntax/search/selection composition. Tree rendering also covers zero-cell and
one-cell layouts. The three new PTY sessions inspect actual RGB output while
switching between the tree, buffer list, picker and source, resizing to one cell,
and checking edit/undo, unchanged disk bytes and terminal restoration. These
sessions also passed separately after making initial navigation wait for the
directory scan to finish.

Raw ANSI captures are in `/tmp/editor-pro-issue17-terminal`, including
`colorful-ui-{tokyo-night,catppuccin,gruvbox}.ansi`. Representative real PTY frames
were reconstructed with pyte and inspected as images; the Tokyo Night
[tree](docs/images/file-tree.png) and [source/status](docs/images/editor.png)
captures are included in the repository. Other palette previews and their raw
captures use the `preview-tree-*` prefix in the same temporary directory.
No screenshot-generation dependency was added to the package or test suite.

Full logs are `/tmp/editor-pro-issue17-{rust-tests,clippy,python,terminal-tests}.log`.
Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process access because the sandbox could not initialize its loopback interface.
macOS, physical terminals and real language/AI servers were not retested for this
presentation change.

## Issue 18 — Binary assets and snapshot capture timeouts

Validated on Linux x86_64, 2026-10-16, with Rust/Cargo 1.99.0 and Python 3.12.
Snapshots now retain exact binary/non-UTF-8 bytes alongside text. Binary changes,
additions, deletions, exact renames and text/binary transitions remain visible in
snapshot deltas; a selected binary diff reports unsupported text content without
preventing text review elsewhere. Existing text-only version-2 stores retain
their representation. Binary payloads use bounded hexadecimal encoding, with
content hashes and decoded byte limits checked on load.

Baseline capture uses the original NUL-delimited tree/index listing and two Git
batch calls per sample, checking sizes before requesting blob contents. A
512-file regression exercises HEAD, index and revision baselines with a 20 ms
delay per Git invocation. Each capture finishes within the ordinary five-second
test configuration and uses at most 20 Git processes, including both baseline
samples and discovery. Product capture retains its 30-second deadline. These
fixture results are not a guarantee for slow filesystems or larger repositories.

All **378 ordinary Rust tests** passed; 13 opt-in tests were ignored. The snapshot
suite now has **21 tests**, including six new regressions for mixed binary/text
capture and restart, immutable binary checkpoints, transitions and renames,
encoding/hash validation, batched collection at the path limit, size preflight,
malformed/truncated batch output and timeout cleanup/retry. Existing source,
index, ref, dirty-buffer, Unicode/CRLF and persistence checks also passed.
The debug build, formatting, Clippy with warnings denied, whitespace checks and
all **14 Python tests** passed.

```sh
cargo build --locked
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
git diff --check
python3 -m unittest discover -s tools -p 'test_*.py' -v
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
```

The new `run_binary_snapshot_session` fixture in `tools/terminal_smoke.py` passed
independently against the debug binary. It captures tracked PNG and untracked
non-UTF-8 assets, navigates full/since and binary diffs, verifies exact persisted
bytes, unchanged source/index/HEAD and terminal restoration. Its ANSI capture is
`/tmp/editor-pro-issue18-terminal/binary-review-snapshot.ansi`. The complete
56-session PTY harness was not rerun in this change. Rust test and lint logs are
`/tmp/editor-pro-issue18-all-tests.log` and `/tmp/editor-pro-issue18-clippy.log`.

Cargo used `CARGO_TARGET_DIR=/tmp/editor-pro-issue18-target`,
`CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands needed normal
process access because the sandbox could not initialize its loopback interface.
macOS, physical terminals and real language servers were not retested. File/tree,
history and serialized-store limits still apply to binary assets. No release
version, integration policy or publication workflow changed.

## Issue 20 — Complete suggestions without another selection view

Validated on Linux x86_64, 2026-10-18, with Rust/Cargo 1.99.0. Enter and Tab now
accept completions with additional imports, multiline text and advertised server
commands directly from either list. Command-provided edits use the existing
version, session, project, size, overlap and disk checks without opening a preview.
Acceptance adds or reuses a trailing space, preserves the originating mode and
places the Insert cursor after that space. Command offsets are preserved by adding
the space after the command's edit. Code-action, rename and format previews remain.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 376 passed; 13 opt-in tests ignored. |
| Completion regressions | All 27 passed; expanded no-edit/import-only/other-file command cases also passed. |
| `cargo build --locked` | Passed. |
| Python tool/harness and Kotlin probe unit tests | All 14 passed. |
| Focused completion PTY sessions | All four passed. |
| `python3 tools/terminal_smoke.py` | All 58 sessions passed. |
| `git diff --check` | Passed. |

The regressions cover both lists and acceptance keys, optional resolution,
additional edits, multiline text, command synchronization and follow-ups,
Unicode/UTF-16/CRLF, existing whitespace, cursor placement, undo/redo, unchanged
disk bytes and cancellation after typing, movement, Escape, dismissal or buffer
switches. Invalid edits, disk conflicts, unadvertised commands and late edits
following a failed command remain rejected. The real-server fixtures now require
zero completion previews for Kotlin, Java and C#; they were compiled but not run.

The focused terminal sessions save exact expected bytes, undo both acceptance
paths, resize narrowly and verify terminal restoration. Captures are in
`/tmp/editor-pro-issue20-terminal/`: `completion-acceptance.ansi`,
`auto-completion-extra.ansi`, `auto-completion-multiline.ansi` and
`auto-completion-command.ansi`. A smoke assertion initially missed a message
split across incremental ANSI updates; forcing a resize before the assertion
fixed the harness.

The full terminal suite also passed editing, Git/review, LSP navigation and
installation, disk refresh, help, project navigation, AI and terminal restoration.
Logs are `/tmp/editor-pro-issue20-tests.log`, `/tmp/editor-pro-issue20-clippy.log`
and `/tmp/editor-pro-issue20-terminal.log`.

Cargo used `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_INCREMENTAL=0` and two build jobs. The first build exhausted the host disk;
this attempt's partial build was removed and final build/test artifacts used
`/dev/shm/editor-pro-issue20-target` and `/dev/shm/editor-pro-issue20-tmp`.
Commands required normal process access because the sandbox could not initialize
its loopback interface. macOS, physical terminals and real language servers were
not tested. No release version or workflow changed.

## Issue 21 — Automatic delimiter pairing

Validated on Linux x86_64, 2026-10-18, with Rust/Cargo 1.99.0 and Python 3.12.
Insert typing pairs parentheses, square brackets, braces, double/single quotes
and backticks, leaving the cursor inside. Existing closers can be overtyped;
Backspace removes an empty pair. Lexical neighbor and escape checks preserve
ordinary word/apostrophe editing, and paste, registers and prompts remain literal.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 383 passed; 13 opt-in tests ignored. |
| `cargo build --locked` | Passed. |
| Python unit-test discovery in `tools` and `tools/kotlin-lsp` | All 14 passed. |
| `python3 tools/terminal_smoke.py` | All 57 PTY sessions passed. |
| `git diff --check` | Passed. |

Eleven new Rust regressions cover all six pairs, nesting, closer overtyping,
atomic empty-pair deletion, escaped quotes, apostrophes, Unicode graphemes, CRLF,
undo/redo and movement groups, literal paste/registers/prompts, completion inside
an automatic pair, and atomic rejection at the note size limit. The mixed-edit
grapheme-boundary test also exercises every delimiter. The two new PTY sessions
verify nested Unicode typing, undo/redo, empty-pair Backspace, literal bracketed
paste, exact saved CRLF bytes and terminal restoration.

Terminal captures are `/tmp/editor-pro-issue21-terminal/auto-pairs-nested.ansi`
and `auto-pairs-backspace-paste.ansi` in the same directory. `auto-pairs.svg` and
`auto-pairs.txt` reconstruct the captured Insert frame with the cursor between
nested braces. Logs are `/tmp/editor-pro-issue21-{tests,clippy,build,terminal}.log`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_DEV_STRIP=symbols`,
`CARGO_PROFILE_TEST_STRIP=symbols` and `CARGO_INCREMENTAL=0` to fit the available
disk space. Commands used normal process access after the sandbox failed to
initialize its loopback interface. macOS, physical terminals and the opt-in real
language-server/AI fixtures were not tested; completion coverage uses the
existing protocol double.

## Issue 22 — Automatic newline indentation

Validated on Linux x86_64, 2026-10-18, from `98761c8`. Enter and Normal `o`/`O`
inherit leading spaces/tabs. Supported code delimiters add an indentation level;
Enter between paired delimiters moves the closer below the indented blank line.
Nearby code supplies the indentation unit, excluding comment-only margins such
as Javadoc/KDoc. Defaults are four spaces for Rust/Java/Kotlin/C# and two for
JSON/YAML/TOML. Unknown languages, Markdown and unavailable syntax copy existing
whitespace. Framework settings and `.editorconfig` are not read.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | All 388 ordinary tests passed; 13 opt-in tests ignored. |
| `cargo build --locked` | Passed. |
| `python3 -m unittest discover -s tools -p 'test_*.py' -v` | All six tests passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | All eight tests passed. |
| `python3 tools/terminal_smoke.py` | All 57 PTY sessions passed. |
| `git diff --check` | Passed. |

Fifteen indentation regressions cover language defaults and incomplete syntax,
local tabs/space widths, documentation-comment margins, nested pairs, `o`/`O`,
strings/comments, closer alignment, plain/blank lines, splitting whitespace,
Unicode scalar cursors, CRLF, grouped undo/redo and incremental/fresh syntax
equivalence. They also cover literal paste/Tab, syntax-size fallback, save-as
language detection and save conflicts. An additional note regression rejects
the complete indented insertion at the size limit without changing text/cursor
or entering Insert. The completion-popup regression now verifies that Enter
after dismissal inherits a tab and places the cursor after it.

The two new PTY sessions verify exact saved bytes for nested Java braces with
Unicode, CRLF, save/undo/redo and resize, plus tab-indented `o`/`O`. Captures are
`/tmp/editor-pro-issue22-terminal/autoindent-java-crlf.ansi` and
`/tmp/editor-pro-issue22-terminal/autoindent-tabs-open-lines.ansi`, produced with
`EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue22-terminal`. Full logs are
`/tmp/editor-pro-issue22-{clippy,rust-tests,build,python,terminal-tests}.log`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process access because the sandbox could not initialize its loopback interface.
macOS, hosted CI and physical-terminal appearance were not tested here. Real
language servers and AI providers were not retested for this local editing change.

## Issue 23 — Readable source diagnostics

Validated on Linux x86_64 with Rust/Cargo 1.99.0, 2026-10-18, from `11db49c`.
Source lines now show severity-colored diagnostic text after the code, with the
highest severity first and a count of additional messages. The cursor line also
gets a bounded, wrapped panel above the statusline; long source lines cannot hide
that explanation. The existing diagnostic list and navigation remain available.
Versionless reports retain their unverified marker. Rendering uses the existing
current-document guard, so edits invalidate both messages and gutter marks.

| Check | Result |
| --- | --- |
| `cargo fmt --check` and `git diff --check` | Passed. |
| `cargo test --locked --all-targets` | 418 tests passed; 13 opt-in tests ignored. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed. |
| `cargo build --locked` | Passed. |
| `python3 -m unittest discover -s tools -p 'test_*.py' -v` | Six tests passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | Eight tests passed. |
| Focused diagnostic PTY session | Passed after correcting the wait marker for the existing diagnostics list's path clipping. |
| `EDITOR_PRO_SMOKE_CAPTURE_DIR=/tmp/editor-pro-issue23-terminal python3 tools/terminal_smoke.py` | All 68 PTY sessions passed. |

Five new Rust regressions cover severity priority and all themes, multiple
messages, missing severity, wrapping alongside long source lines, command/search
prompts and save errors, Unicode graphemes and tab/cell alignment, CRLF/control
sanitization, truncation, unverified provenance, multiline/end-exclusive and
zero-length ranges, stale/cleared reports, buffer switches and resizing down to
zero cells. They assert that rendering preserves source text, generation and
cursor. The new PTY scenario exercises inline and wrapped messages, diagnostic
navigation, resizing, editing away the error, exact saved CRLF bytes and terminal
restoration using the existing protocol double.

The diagnostic capture is
`/tmp/editor-pro-issue23-terminal/lsp-diagnostics.ansi`; `lsp-diagnostics.txt` and
`lsp-diagnostics.svg` reconstruct its first complete diagnostic frame. Logs are
`/tmp/editor-pro-issue23-{rust-tests,clippy,build,python,terminal-tests}.log`.

Cargo used `CARGO_NET_OFFLINE=true`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_INCREMENTAL=0`. Commands required normal
process access because the sandbox could not initialize its loopback interface.
macOS, hosted CI, minimum Rust 1.88, physical terminals, real language servers and
AI providers were not tested here. No release version or workflow changed.

## Issue 24 — Shared macOS and Linux configuration path

Validated on Linux x86_64, 2026-10-18. Configuration now defaults to
`~/.config/editor-pro/config.json` on macOS and Linux. An absolute, nonempty
`XDG_CONFIG_HOME` takes precedence; the former macOS Application Support config
path is no longer read. The README, installation guidance and architecture record
describe the lookup and manual migration of existing settings.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed. |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed without warnings. |
| `cargo test --locked --all-targets` | 373 passed; 13 opt-in tests ignored. |
| `cargo build --locked` | Passed. |
| `python3 -m unittest discover -s tools -p 'test_*.py' -v` | Six release/harness tests passed. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | Eight probe tests passed. |
| `python3 tools/terminal_smoke.py` | All 56 PTY sessions passed. |

Path regressions cover absolute XDG precedence, unset/empty/relative XDG
fallbacks, and missing or invalid home directories. CLI tests verify both the
reported path and startup configuration errors, including XDG without `HOME`.
The terminal harness loads Gruvbox from both `~/.config` and an XDG override,
checks the expected path independently of the editor, and preserves settings
and Unicode/CRLF source bytes while verifying terminal restoration. Captures:
`/tmp/editor-pro-issue24-captures/installation-config.ansi` and
`/tmp/editor-pro-issue24-captures/installation-config-xdg.ansi`.

Native macOS execution, Homebrew installation and real language servers were not
tested locally. The existing Linux/macOS CI runs the configuration regressions
and terminal harness on both supported platforms.

## Release v0.2.19 preparation

The release integrates source diagnostics with the latest `main` configuration
change, retaining both validation records and all 69 PTY scenarios. It also
includes the previously merged navigation colors, batched/binary review
snapshots, inline completion acceptance, delimiter pairing and automatic
indentation since v0.2.18. Published v0.2.18 assets remain unchanged.

The Cargo package and lockfile now report **0.2.19**. On the integrated code,
all four CLI tests, seven configuration tests and six release/harness Python
tests passed, along with the matching-tag check, offline Cargo check, debug build,
binary version output, formatting and whitespace checks. The tag-triggered
workflow gates publication and the Homebrew tap update on Linux/macOS CI and
native ARM64 Homebrew/PTY acceptance. Preparation logs are
`/tmp/editor-pro-v0.2.19-prepare.log`.
