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
