# Architecture

Decision record, 2026-10-14. Scope: Milestones 1–2 and an isolated Kotlin feasibility
experiment. The product is one terminal editor for editing, navigation, Git review
and eventual AI comprehension. There is no separate Review Mode.

## Current structure

One Cargo package with a small library and executable. No workspace hierarchy,
plugin API, generic backend trait, event bus, dependency injection, or GUI layer.

```text
main.rs      terminal lifetime, CLI, event loop, disk checks and picker polling
input.rs     Crossterm events -> terminal-independent Key values
editor.rs    editing modes, pending counts/operators, prompts, unnamed register
buffer.rs    Rope, grapheme cursor, bounded undo, edit descriptions and generation
motion.rs    motions and text-object ranges in Unicode scalar coordinates
file.rs      Document, file identity/baseline, guarded loading/saving
syntax.rs    incremental Tree-sitter state, viewport captures, plain fallback
search.rs    literal matches, generation-based cache and navigation
buffers.rs   buffer identities, inactive documents/viewports, guards and list
picker.rs    cancellable discovery and Nucleo background matching
text.rs      shared grapheme/cell rules
ui.rs        projects editor state into Ratatui; maintains derived state/viewport
theme.rs     semantic colors and three selectable palettes
```

`Document` owns a `Buffer`, its disk baseline and derived `Syntax`. `Editor` holds
an active document/viewport and a `BTreeMap` of inactive slots with stable IDs.
Switching swaps those fields, keeping one editing engine and independent histories,
syntax trees, cursor positions and scroll offsets. Opening an existing normalized
path reuses its buffer; failed reads leave the active state untouched. A save-as
cannot take another open buffer's identity, even if that file is not on disk yet.
The unnamed register and last search are session-wide; the search cache is
invalidated on every buffer switch.

`Mode` (Normal, Insert, Visual, VisualLine, Command, Search) and `View` (Editing,
Buffers, Files) are separate. Contextual views intercept their own keys and do not
edit the underlying text. Closing a picker/list returns to the originating
location. No split or general view-management framework is needed for M2.

## Dependency choices

| Concern | Decision and reason |
| --- | --- |
| Terminal | Ratatui 0.30.2 with its re-exported Crossterm backend. Established widget/rendering and input infrastructure; `TestBackend` supports deterministic UI tests. One backend version avoids incompatible event types. [Installation](https://ratatui.rs/installation/). |
| Text | Ropey 1.6.1, with only LF/CRLF line boundaries and SIMD enabled. Unicode scalar indices, efficient splices, and cheap cloned trees for undo. Graphemes remain the editor's responsibility. [Ropey](https://docs.rs/ropey/latest/ropey/). |
| Unicode | `unicode-segmentation` for extended graphemes, `unicode-width` for cells. Bytes, scalar indices and terminal cells are distinct. [Segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/). |
| Files | `tempfile` supplies safe temporary-name creation and same-filesystem persistence. The editor supplies baseline checks, fsync and explicit error handling. [Persistence API](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist). |
| Highlighting | Tree-sitter 0.26 with upstream grammar crates and queries for eight languages. The Markdown crate supplies its combined block/inline parser. Byte edit descriptions and old trees support incremental parsing. [Incremental parsing](https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html). |
| File picker | Nucleo 0.5 supplies background fuzzy matching/ranking and immutable result snapshots. The `ignore` walker supplies ignore-file handling; a small discovery thread feeds Nucleo. [Nucleo](https://github.com/helix-editor/nucleo). |
| External services, later | Tokio + `serde_json`/`lsp-types` when LSP/project processes become real requirements. No async runtime in M2. |
| Git/project search, later | Prefer Git CLI and ripgrep subprocesses initially. Machine-readable output, explicit arguments and cancellation; avoid shell interpolation. Revisit `git2` only when process overhead or APIs justify it. |
| Configuration, later | A small serde/TOML schema when users need settings. M2 selects among three semantic palette values at runtime; no configuration framework. |

Helix's separation of core text/history from terminal concerns is useful prior
art, as is its explicit grapheme handling. Its mature transaction/history system
is substantially broader than this milestone needs. Here a snapshot is just a
shared Rope, cursor and revision; no undo tree or transaction algebra yet.
[Helix history](https://github.com/helix-editor/helix/blob/master/helix-core/src/history.rs),
[Helix graphemes](https://github.com/helix-editor/helix/blob/master/helix-core/src/graphemes.rs).

## Editing invariants

1. Rope edits use absolute Unicode scalar indices. A cursor lies on an extended
   grapheme boundary, never inside UTF-8, a CRLF pair, or an emoji sequence.
2. Insert allows the line-end insertion position; Normal clamps to the last
   grapheme (or column zero for an empty line). Vertical movement remembers the
   desired cell column through shorter lines, including tabs and wide text.
3. Input decoding does not mutate text. Commands operate on the core; rendering
   does not edit it. There is no terminal dependency in buffer/file logic.
4. Insert sessions, movement breaks and pastes define undo groups. New committed
   edits invalidate redo; no-op groups do not. Savepoint identity survives
   undo/redo. History is bounded to 1,000 groups, not persisted.
5. File bytes are preserved on a no-edit save. Newline insertion follows the first
   observed LF/CRLF style; final newlines are not added implicitly.
6. Paths and source text are rendered as data. Control bytes are displayed as
   replacement glyphs; tabs are expanded. Bracketed paste never runs commands.

Segmenting one whole line is an intentional starting simplification. Extremely
long/minified lines require later profiling and chunk-aware segmentation. Text
width is shared by movement and rendering. Tree-sitter byte offsets are explicitly
converted at the mutation boundary; they never become the core cursor representation. Future LSP UTF-16 conversion
belongs at the protocol boundary.

## Derived syntax, search and background navigation

Every buffer splice records byte offsets and old/new `(row, byte-column)` points
in a small `TextEdit`; undo/redo emit replacements too. The core knows nothing
about Tree-sitter. `Document::sync_syntax` consumes these records before rendering,
edits the previous tree and parses Rope chunks through a callback. Cursor-only
redraws reuse the tree. Saving under a different extension redetects the language.

Queries come from the grammar packages, without a local grammar distribution
framework. Rust was integrated first, followed by Java, Kotlin, C#, JSON, YAML,
TOML and Markdown. Kotlin uses the `tree-sitter-kotlin-sg` package of the fwcd
grammar with its compatible bundled queries. Unknown general query predicates
are rejected instead of silently ignored; built-in equality/regex predicates
are evaluated by Tree-sitter's query iterator.

Markdown uses `tree-sitter-md`'s `MarkdownParser` and `MarkdownTree`, preserving
independent inline trees for paragraphs and table cells. This prevents emphasis
from crossing paragraph boundaries or parsing fenced code as inline content.
Both block and inline queries are restricted to the viewport. A Rope-backed
text provider supplies query text without flattening the document. Captures map
to semantic colors; the smaller capture wins overlaps, then the later pattern.

Files above 2 MiB skip syntax parsing. A failed grammar/query or parse leaves an
editable plain-text document; parse failures retry on the next edit. Parsing has
a cooperative 100 ms budget; viewport queries have a 20 ms budget and 20,000-span
cap. These bound ordinary work but are not hard real-time limits, especially
across Markdown's many small inline parses. No injection or semantic-token layer
is included. See [VALIDATION.md](VALIDATION.md) for measured latency.

Search stores scalar match ranges at grapheme boundaries, cached by the buffer's
monotonic mutation generation (separate from undo/savepoint revision identity).
Changed text or queries rebuild the cache synchronously; movement reuses it.
Only single-line, literal, case-sensitive, non-overlapping matches are supported,
with a reported 100,000-result cap. Search prompts commit on Enter and cancel
without altering the prior query/location. Search and Visual overlays are composed
by the renderer, with selection taking background precedence.

File discovery runs on a cancellable thread; Nucleo owns two matching workers.
The `ignore` walker respects ignore rules, includes non-ignored hidden files,
excludes VCS metadata/symlinks, and caps candidates at 100,000. Workers do not
mutate documents. The UI polls snapshots every 16 ms while the picker is open;
stale query snapshots cannot be shown or selected. Closing the picker sets a
cancellation flag; no worker join blocks the editing thread. Opening a match uses
the same guarded buffer-opening path as `:e`. No asynchronous service abstraction
or process manager was introduced for these concrete requirements.

## Persistence and terminal lifetime

Saving compares current disk bytes to the last loaded/saved baseline, writes a
temporary file in the target directory, copies existing permissions, syncs it,
checks the baseline again, renames, and syncs the directory. New destinations use
no-clobber persistence. A failed save preserves the buffer and prevents `:wq`
from quitting. A directory-sync failure after rename updates the disk baseline
but leaves the buffer dirty, explaining that persistence is uncertain.

Opening a path with `:e path` retains the previous buffer, including unsaved
edits. Reloading or closing a dirty buffer requires explicit `!`. `:q` and `:wq`
check every buffer; `:q!` discards all unsaved edits. Explicit `!` never force-writes
external changes. Disk polling and focus checks cover inactive buffers as well
and report changes without replacing text. This is basic conflict detection, not
the future live-agent refresh workflow.

The final check and rename cannot atomically compare-and-swap against arbitrary
external writers. Symlink/hardlink restrictions and metadata limitations are
documented in the README. Atomic rename is not an autosave or crash-recovery
system. File I/O remains synchronous and bounded to 64 MiB; disk polling should
move to a worker if real latency measurements justify it.

Terminal setup has a scope guard created before raw mode starts. Both screen
state and raw mode restoration are attempted on errors. A panic hook restores
before printing the panic. Cursor shape and bracketed-paste/focus reporting are
restored too. Signals/job control require a separate future hardening change.

## Future boundaries, not current infrastructure

- **M3–4:** Git diff data is independent of language intelligence. Review state
  keys include base revision, path, and content fingerprints. A content change
  invalidates a review mark; timestamps alone are insufficient.
- **M5:** One language-server manager owns subprocess lifecycle and protocol I/O.
  Requests carry document version, buffer ID and request generation. Ignore stale
  results, support cancellation, bounded queues, stderr logs, timeouts and crash
  recovery. UI commands submit work; no process waits on the UI thread. Capability
  negotiation controls available actions. Root discovery must respect Gradle,
  Maven, solution and project boundaries. See the feasibility report.
- **M6–7:** Navigation history records buffer, cursor and viewport. Review
  snapshots record immutable content without altering the working tree.
- **M8:** Context collection and provider integration stay independent of editing
  and review notes. Use a documented agent CLI, user-visible context and explicit
  edit approval. No provider dependencies or placeholder APIs in this milestone.

## Principal risks and gates

| Risk | Mitigation / next evidence |
| --- | --- |
| Unicode boundary and screen-width errors | Core tests for combining characters, emoji, CJK, CRLF and mixed edits; real-terminal checks on both OSes. |
| Lost work | Conflict tests, failed-save tests, dirty quit guards; document remaining race, metadata and crash-recovery limits. |
| Project import and SDK mismatches | Isolated Kotlin test now; capability and real-project matrix before M5. Runtime JDK and project target JDK are separate. |
| Server churn or proprietary protocol assumptions | Pin tested standalone versions, record capabilities, run standard-LSP probes. Kotlin feasibility is still unverified here. |
| Stale async state | Picker snapshots must match the current query before selection. Future protocol requests must carry buffer/version/generation; never trust arrival order. |
| Scope and architecture growth | Only M1–2 code in the binary; no Git/LSP/AI stubs. Split modules only at concrete responsibilities. |
| Platform and responsiveness evidence | Linux formatting, Clippy, Rust/Python tests, ten PTY sessions and release timing measurements pass. macOS, physical-terminal inspection and representative user workloads remain to be evaluated. |
