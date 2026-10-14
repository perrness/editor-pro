# Architecture

Decision record, 2026-10-14. Scope: Milestone 1 and an isolated Kotlin feasibility
experiment. The product is one terminal editor for editing, navigation, Git review
and eventual AI comprehension. There is no separate Review Mode.

## Current structure

One Cargo package with a small library and executable. No workspace hierarchy,
plugin API, generic backend trait, event bus, dependency injection, or GUI layer.

```text
main.rs   terminal lifetime, event loop, CLI, periodic disk checks
   |
input.rs  Crossterm events -> terminal-independent Key values
   |
editor.rs modes, key dispatch, command prompt, messages, viewport
   |                         |
buffer.rs                 file.rs
Rope, cursor, undo        Document, disk baseline, load/save
   |
text.rs   shared grapheme/cell rules

ui.rs     projects editor state into Ratatui widgets; adjusts viewport only
theme.rs  semantic color palette
```

`Document` owns a `Buffer` plus file identity/baseline. `Editor` owns one document
and one viewport. Mode and viewport are separate data. Future contextual views
should refer to shared documents; add document IDs and a buffer collection only
when multiple buffers arrive. Do not put a second editing engine in diff or
review views. Closing a contextual view should restore its originating location.

## Dependency choices

| Concern | Decision and reason |
| --- | --- |
| Terminal | Ratatui 0.30.2 with its re-exported Crossterm backend. Established widget/rendering and input infrastructure; `TestBackend` supports deterministic UI tests. One backend version avoids incompatible event types. [Installation](https://ratatui.rs/installation/). |
| Text | Ropey 1.6.1, with only LF/CRLF line boundaries and SIMD enabled. Unicode scalar indices, efficient splices, and cheap cloned trees for undo. Graphemes remain the editor's responsibility. [Ropey](https://docs.rs/ropey/latest/ropey/). |
| Unicode | `unicode-segmentation` for extended graphemes, `unicode-width` for cells. Bytes, scalar indices and terminal cells are distinct. [Segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/). |
| Files | `tempfile` supplies safe temporary-name creation and same-filesystem persistence. The editor supplies baseline checks, fsync and explicit error handling. [Persistence API](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist). |
| Highlighting, later | Tree-sitter with upstream grammars/queries. Incremental edits and old trees avoid custom parsing. Do not build grammar infrastructure in M1. [Incremental parsing](https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html). |
| File picker, later | Evaluate `nucleo` at M2; its matcher already serves Helix. No fuzzy-search implementation now. [Nucleo](https://github.com/helix-editor/nucleo). |
| External services, later | Tokio + `serde_json`/`lsp-types` when LSP/project processes become real requirements. No async runtime in M1. |
| Git/search, later | Prefer Git CLI and ripgrep subprocesses initially. Machine-readable output, explicit arguments and cancellation; avoid shell interpolation. Revisit `git2` only when process overhead or APIs justify it. |
| Configuration, later | A small serde/TOML schema when users need settings. M1 uses one semantic palette value; no configuration framework. |

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
width is shared by movement and rendering. LSP UTF-16 and Tree-sitter byte offsets
will require explicit conversion at their boundaries; they must never become the
core cursor representation.

## Persistence and terminal lifetime

Saving compares current disk bytes to the last loaded/saved baseline, writes a
temporary file in the target directory, copies existing permissions, syncs it,
checks the baseline again, renames, and syncs the directory. New destinations use
no-clobber persistence. A failed save preserves the buffer and prevents `:wq`
from quitting. A directory-sync failure after rename updates the disk baseline
but leaves the buffer dirty, explaining that persistence is uncertain.

Unsaved buffers cannot be replaced by `:e` or abandoned by `:q`. Explicit `!`
applies only to discard/reload, never force-writing external changes. Disk polling
and focus checks report changes without replacing buffer text. This is basic
conflict detection, not the future live-agent refresh workflow.

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

- **M2:** Introduce edit ranges for Tree-sitter at the buffer mutation boundary;
  keep derived syntax state separate. Share a buffer between views when needed.
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
| Stale async state | Introduce versioned requests with the first async subsystem; test out-of-order responses, never trust arrival order. |
| Scope and architecture growth | Only M1 code in the binary; no Git/LSP/AI stubs. Split modules only at concrete responsibilities. |
| Unverified Rust build | Rust toolchain/network unavailable during implementation. CI and a PTY smoke harness are supplied, but M1 acceptance is pending an actual successful run. |
