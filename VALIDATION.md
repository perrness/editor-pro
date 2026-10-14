# Delivery validation — 2026-10-14

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
