# Architecture

Decision record, 2026-10-15. Scope: Milestones 1–8, pinned real-server acceptance
fixtures and an isolated Kotlin feasibility experiment.
The product is one terminal editor for editing, navigation, Git review
and optional AI comprehension. There is no separate Review Mode.

## Current structure

One Cargo package with a small library and executable. No workspace hierarchy,
plugin API, generic backend trait, event bus, dependency injection, or GUI layer.

```text
main.rs      terminal lifetime, CLI, event loop, focus notifications and worker polling
config.rs    optional bounded user settings and native configuration paths
input.rs     Crossterm events -> terminal-independent Key values
help.rs      shared shortcut metadata, bounded search and read-only help overlay
editor.rs    editing modes, pending counts/operators, prompts, unnamed register
buffer.rs    Rope, grapheme cursor, bounded undo, edit descriptions and generation
motion.rs    motions and text-object ranges in Unicode scalar coordinates
file.rs      Document, file identity/baseline, guarded loading/saving
disk.rs      bounded disk polling, content observations and explicit refresh overlay
syntax.rs    incremental Tree-sitter state, viewport captures, plain fallback
search.rs    literal matches, generation-based cache and navigation
buffers.rs   buffer identities, inactive documents/viewports, guards and list
picker.rs    cancellable discovery and Nucleo background matching
navigation.rs session-local location history and recent-file view
project_search.rs bounded ripgrep worker, disk-match validation and result view
git.rs       repository/status discovery, bounded CLI worker, comparison snapshots
git_snapshot.rs bounded immutable tree capture and live review comparisons (Git child module)
diff.rs      unified coordinates, deletion anchors, paired alignment and source mapping
git_view.rs  changed-file/diff navigation over existing documents
git_ui.rs    contextual Git lists, inline rows and synchronized paired panes
review.rs    versioned records, full content identities and unique attachment keys
review_snapshot.rs bounded baseline/checkpoint schema and content validation
review_store.rs bounded loading, record CAS, atomic persistence and Markdown worker
review_view.rs  review commands, progress/navigation and contextual record list
lsp.rs         protocol positions, immutable document stamps, capabilities and roots
lsp_worker.rs  bounded multi-project server lifecycle, transport and synchronization
lsp_warmup.rs  bounded cancellable project hints for the existing session manager
lsp_install.rs pinned per-user server provisioning, runtime selection and installation jobs
lsp_edit.rs    complete workspace-edit validation and ordinary buffer transactions
lsp_view.rs    language commands, diagnostics and contextual results/edit previews
lsp_completion.rs Insert-mode completion scheduling and popup (LSP view child module)
ai.rs          bounded captured context, structured answers and conversation schema
ai_worker.rs   optional Codex exec lifecycle, JSONL progress and transcript I/O jobs
ai_store.rs    private transcript loading, atomic persistence and conflict checks
ai_view.rs     context collection, conversations, citations and revision previews
text.rs      shared grapheme/cell rules
ui.rs        projects editor state into Ratatui; maintains derived state/viewport
splash.rs    responsive ASCII startup artwork, separate from document contents
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
Buffers, Files, Recent, ProjectSearch, Changes, Diff, Review, Lsp, Ai) are separate. Contextual views intercept their own keys and do not
edit the underlying text. Closing a picker/list returns to the originating
location. M3 adds only a paired diff projection, not a general window framework.

Help is an optional overlay above both `Mode` and `View`, opened by F1 or
`:help [QUERY]`. It intercepts keys, paste and rendering before the underlying
context without resetting it or committing its undo group. The source cursor,
viewport, selection, prompt, pending operator and contextual list selection stay
in place. `Editor::tick` defers applying worker results until dismissal, so a
definition response or completed note save cannot change the origin under help;
workers themselves continue and retain their existing bounds and deadlines.
Disk-conflict checks still run without replacing source text.

One categorized catalog supplies the in-editor reference, including mode/view
scope. Normal `gd`, `gr` and leader command dispatch share that catalog's mappings;
the existing state machine retains operator/count/mode guards. Help searches
keys, context and descriptions case-insensitively with a 256-byte query cap and
grapheme Backspace. Wrapped rows are rebuilt only when the query, category or
terminal width changes, and scroll independently even in a one-cell terminal.
Opening/searching/rendering help performs no document edits or external work.

## Dependency choices

| Concern | Decision and reason |
| --- | --- |
| Terminal | Ratatui 0.30.2 with its re-exported Crossterm backend. Established widget/rendering and input infrastructure; `TestBackend` supports deterministic UI tests. One backend version avoids incompatible event types. [Installation](https://ratatui.rs/installation/). |
| Text | Ropey 1.6.1, with only LF/CRLF line boundaries and SIMD enabled. Unicode scalar indices, efficient splices, and cheap cloned trees for undo. Graphemes remain the editor's responsibility. [Ropey](https://docs.rs/ropey/latest/ropey/). |
| Unicode | `unicode-segmentation` for extended graphemes, `unicode-width` for cells. Bytes, scalar indices and terminal cells are distinct. [Segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/). |
| Files | `tempfile` supplies safe temporary-name creation and same-filesystem persistence. The editor supplies baseline checks, fsync and explicit error handling. [Persistence API](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist). |
| Highlighting | Tree-sitter 0.26 with upstream grammar crates and queries for eight languages. The Markdown crate supplies its combined block/inline parser. Byte edit descriptions and old trees support incremental parsing. [Incremental parsing](https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html). |
| File picker | Nucleo 0.5 supplies background fuzzy matching/ranking and immutable result snapshots. The `ignore` walker supplies ignore-file handling; a small discovery thread feeds Nucleo. [Nucleo](https://github.com/helix-editor/nucleo). |
| Language services | Tokio process/I/O/timers/channels on one private runtime thread, plus `lsp-types` 0.95.1 for LSP 3.17 positions, edits, diagnostics and file URIs. No runtime enters the editing, Git or review core. |
| Git | Git CLI subprocesses with porcelain-v2 NUL status, immutable blob reads, and internally generated no-index diffs. One bounded worker owns cancellation/timeouts. M3 added no dependencies; M4 adds the review dependencies below. |
| Project search | Installed ripgrep CLI with literal patterns, NUL-terminated filenames and vimgrep byte columns. A concrete standard-library worker owns requests/processes; no new crate or generic process framework. |
| Review | `sha2` 0.10 supplies SHA-256; `serde`/`serde_json` supply a strict versioned local format. A small standard-library worker owns I/O; no new runtime or backend framework. |
| AI | Installed Codex CLI `exec` with JSONL and structured output. A concrete standard-library worker uses existing serde/tempfile/hash dependencies. No SDK, additional runtime, provider trait or second provider. |
| Configuration | An optional, bounded JSON file using existing serde/serde_json dependencies. Native macOS Application Support and Linux XDG paths; startup theme and LSP installation policy, with strict validation. No configuration framework. |
| Server installation | ureq/rustls for HTTPS, tar/flate2 and zip for bounded archive extraction, fs2 for cross-process installation locks. Existing sha2 verifies pinned hashes. Concrete installation threads keep network/filesystem/SDK work outside the protocol loop. |

Helix's separation of core text/history from terminal concerns is useful prior
art, as is its explicit grapheme handling. Its mature transaction/history system
is substantially broader than this milestone needs. Here a snapshot is just a
shared Rope, cursor and revision; no undo tree or transaction algebra yet.
[Helix history](https://github.com/helix-editor/helix/blob/master/helix-core/src/history.rs),
[Helix graphemes](https://github.com/helix-editor/helix/blob/master/helix-core/src/graphemes.rs).

## Installation and release boundary

Cargo builds and installs one executable named `editor-pro`; `ep` is a symlink
created by the development helper, release archive or Homebrew formula. Version
output comes from the Cargo package version and works without a terminal or
configuration file. One directory argument opens the existing picker and sets
the process working directory before starting workers; file arguments keep the
existing buffer-opening path. There is no new project/session model.

`config.rs` resolves macOS `~/Library/Application Support/editor-pro/config.json`
or Linux `$XDG_CONFIG_HOME/editor-pro/config.json` (absolute paths only, otherwise
`~/.config`). The optional file is read before terminal initialization, bounded
to 64 KiB and never rewritten. Unknown fields/themes and invalid JSON are errors;
a missing file uses defaults. `editor.relative_line_numbers` defaults to false; the source renderer derives
relative distances from the current cursor while retaining its absolute line
and the existing file-sized gutter. Diff coordinates remain absolute.
Only `main.rs` applies user settings, so library
editors and core tests do not read environment-dependent settings. Runtime theme
changes stay session-local. No configuration lives in a binary installation or
is discovered in a project directory.

Stable version tags gate reusable Linux/macOS checks, native macOS ARM64
builds, installed Homebrew/PTY tests, complete GitHub release publication and a
tap formula update using published SHA-256 values. Published assets remain
unchanged on retries. The tap update is a separate job with a repository-scoped
credential; the editor has no release update check or self-updater. Its LSP
installer downloads only pinned language-server distributions.
Linux binary distribution is deferred. See [release setup](docs/RELEASING.md).

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
   undo/redo. Leaving an editing context commits its group, including reselecting
   the active buffer without swapping documents. History is bounded to 1,000
   groups, not persisted.
5. File bytes are preserved on a no-edit save. Newline insertion follows the first
   observed LF/CRLF style; final newlines are not added implicitly.
6. Paths and source text are rendered as data. Control bytes are displayed as
   replacement glyphs; tabs are expanded. Bracketed paste never runs commands.

Segmenting one whole line is an intentional starting simplification. Extremely
long/minified lines require later profiling and chunk-aware segmentation. Text
width is shared by movement and rendering. Tree-sitter byte offsets are explicitly
converted at the mutation boundary; they never become the core cursor representation.
LSP UTF-16 conversion lives in `lsp.rs`, outside the core.

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

Navigation chrome uses semantic panel/border colors from `theme.rs`, shared
headers and filename-only text badges in `ui.rs`. Badge classification never
reads files or creates syntax parsers. Styled labels retain the shared grapheme
clipping/control-sanitizing rules. Tree indentation is capped by screen width,
and selection/active-file decorations do not change navigation or source state.
Statusline segments have independent styles; source Tree-sitter colors and
selection/search precedence remain unchanged.

## Comprehension navigation and project search

Source history stores stable buffer ID, fallback path, scalar cursor and viewport.
Public opens/switches and explicit result jumps record the departure only after a
successful move. Restoring history uses the same activation path without recording
another jump; it retains current documents and their histories. Save-as updates
fallback paths for the same buffer ID. Closed named buffers can reopen; failed
targets leave source untouched and remove only the inaccessible history entry.
Back/forward stacks cap at 256 each, and new jumps clear the forward stack. Recent
paths are unique, most-recent-first and capped at 100. Both are session-local.
Scalar positions clamp to current graphemes after edits; no symbol tracking,
persistent session format, or navigation transaction framework was introduced.

Project search has one persistent worker with one running request, one replaceable
pending request and one result slot. Request serials cancel obsolete jobs and
suppress late results. Discovery, spawning, subprocess polling and result parsing
run off the editing thread. Escape or leaving the view cancels without joining;
shutdown joins after terminal restoration. Unix subprocesses have a separate
process group, killed/reaped on cancellation or deadline. stdout/stderr use private
temporary files with a 16 MiB capture limit and a cooperative ten-second deadline.
Output can overshoot between polls; filesystem stalls remain outside hard timing
guarantees, as with Git. There are no reader-thread or request-queue accumulations.

Ripgrep runs without user CLI configuration or a shell, with raw-byte encoding,
literal case-sensitive single-line patterns, ignore rules, hidden files and explicit
VCS exclusions. It skips symlinks, uses default binary detection, and caps input
files at 2 MiB. Controlled `--vimgrep --null` output separates raw path bytes from
line/byte-column/text records, preserving non-UTF-8 Unix paths and unusual names.
Non-UTF-8 matched lines are counted as skipped. At most 4,096 matches are retained,
with visible truncation. Results explicitly describe disk contents, independently
of unsaved text. Selection checks the complete matched line against the current
target document before changing the active source, converts UTF-8 bytes to scalars,
and records the jump. It never reloads an open buffer. This is a line check, not a
filesystem snapshot; unobserved external changes elsewhere are not freshness proof.
Prepared unopened targets are activated once, avoiding a second read between
validation and navigation. Ordinary file loading remains synchronous and bounded.

## Git inspection and contextual diffs

Git and language intelligence are independent. `GitState` belongs to `Editor`,
not `Document`: it contains only derived snapshots and view navigation. Source
text, independent undo history, disk baselines and safe-save behavior stay in the
existing core. Each diff records its originating stable buffer ID, scalar cursor
and viewport. Its own selected row, horizontal offset and viewport never update
the source during drawing. Escape restores the origin; Enter explicitly opens
or reuses the corresponding source buffer and jumps by a clamped line coordinate.

A lazily started worker has one running request, one replaceable pending slot,
and one result slot. Each request carries a serial, absolute path, buffer ID,
monotonic mutation generation, dirty flag, cheap Rope snapshot and comparison.
The worker resolves repository identity as worktree root plus absolute Git dir,
so linked worktrees stay distinct. Results carry that identity, the input request,
comparison kind, full structured commit/blob IDs, exact content fingerprints and separate display labels. Buffer replacement
and save invalidate requests even when generation values might coincide. The UI
checks request serials after first observing current buffer/base state; the worker
also suppresses obsolete completions. No subprocess spawn, wait, object read or
diff calculation runs on the input/render thread.

Requests debounce for 75 ms. Edits, saves, focus gain, explicit refresh and
buffer/base changes invalidate or replace pending work; idle two-second polling
samples external Git changes. The worker checks cancellation and its five-second
job deadline between commands and while polling subprocesses. stdout/stderr go
to private temporary files, avoiding pipe backpressure and unbounded reader
threads; output has a 16 MiB limit. Status and patches cap at 100,000 files/rows;
each compared content side caps at 2 MiB. On Unix each process gets a separate
process group; cancellation kills it and reaps the child. Dropping the worker
signals cancellation and joins only at editor shutdown, after terminal restoration.
These checks are cooperative: filesystem stalls and OS scheduling are not a hard
real-time guarantee. Ordinary document loading and saving remain synchronous;
disk polling and explicit refresh use the separate read-only disk worker.

Repository discovery uses `rev-parse` on the nearest existing parent. Git's
[porcelain-v2 status](https://git-scm.com/docs/git-status) identifies branch/HEAD,
staged/unstaged/untracked/conflicted entries and rename paths. Parsing is bytewise
and NUL-delimited; paths stay `OsString`/`PathBuf`, including non-UTF-8 Unix names.
Display escapes control characters. Commands use argument arrays, literal
pathspecs and `--` boundaries. Selected revisions resolve to commits using
[`rev-parse --verify --end-of-options`](https://git-scm.com/docs/git-rev-parse).
Missing Git, inaccessible repositories, invalid revisions and unsupported content
are explicit states, never substituted with a successful clean status.

HEAD or a selected commit is read through `ls-tree` and `cat-file`; index blobs
through `ls-files --stage`. The index itself is never rewritten. A working-side
comparison uses the open buffer (including unsaved edits) or disk if unopened.
A clean loaded copy of a now-deleted file is treated as absent; a dirty copy stays
an unsaved working side. `:diff staged` instead compares HEAD to index and maps its
new-side coordinates through a separate index-to-working diff for source jumps.
Status flags describe only disk/index. Dirty buffers get separate `B+` indicators,
including buffer-only changed-list entries. Unborn HEAD means an empty baseline.
Renames use Git's detection plus the status-recorded original path where available.

Git's [no-index diff](https://git-scm.com/docs/git-diff) runs on private temporary
copies with fixed context and algorithm options. It supplies the diff algorithm;
`diff.rs` parses only this controlled unified output into numbered rows, hunks,
source anchors, gutter marks and paired alignment. No second editing engine or
generic patch/service framework is introduced. CRLF bytes remain intact; changed
CRLF rows and missing final newlines are annotated. Binary/non-UTF-8 data,
symlinks/submodules, conflicts and size/row limits give inspectable errors.
Optional Git locks, fsmonitor, external diff and text conversion are disabled.
Configured clean/process filters are also suppressed and reported as `filters off`;
filter-dependent repositories may therefore show raw-content status differences.
No user configuration, worktree file or index is modified by inspection.

Inline and paired views share one canonical row selection. Paired rows align
contiguous additions/deletions and share both scroll offsets; under 100 columns
they project back to inline. Refresh reanchors by source line (and preserves a
selected hunk header), not by arbitrary display row. Deleted-only lines use the
following surviving line, clamped at EOF. A deleted file without a dirty buffer
has a historical view but no current source destination. Identical/empty and
mode-only changes have no text hunks. Reanchoring after substantial external edits
is approximate. Review identity is separate and never uses that reanchoring as evidence of freshness.

Status is sampled before/after comparison to detect changes during inspection.
This is not an atomic repository snapshot: concurrent external writes can still
race, and later polling refreshes them. Stale/loading/error labels remain visible;
stale gutters and source jumps are disabled. Neither polling nor refresh reloads
an open document or alters its history, cursor or viewport.

Normal-mode `]c`/`[c` navigate the first changed row of each existing diff hunk,
using its source anchor and clamping deletions at EOF. Counts clamp to the first
or last hunk; `]f`/`[f` use the same changed-file list as `:gnext`/`:gprev`, with
counted wraparound. Both paths validate the current document/base stamp before
navigating, including mutations before the next worker tick. Pending work reports
stale/loading feedback without queuing a jump or waiting on Git. Source hunk jumps
enter the existing location history. `Space g d`/`Space g c` call the ordinary
diff/Changes commands; diff dismissal retains the established origin restoration.
These prefixes are Normal-mode commands, separate from operators and find motions.

The source statusline uses a repository-relative path from the latest accepted
Git discovery for that exact document path. This display-only value survives
comparison refreshes and edits; buffer switches and save-as cannot reuse another
path's root. Before discovery or outside Git, it falls back to the document's
absolute name. Rendering performs no repository discovery or filesystem access,
and document identities and save targets remain absolute.
Paths that exceed the available statusline cells lose whole leading directory
components until they fit. Only an overlong filename is clipped within a component,
using the shared grapheme/cell rules; non-file labels retain ordinary text clipping.

## Durable review without another editor

`review.rs` owns review identities and records; the store reads versions 1 and 2. Repository identity includes
both worktree root and absolute Git directory. Each identity records comparison
kind/requested revision, full resolved base/old/new blob IDs, old/new paths/modes,
and each side's SHA-256, byte length and presence flag. Raw Unix path bytes are
serialized losslessly. Unsaved provenance is recorded for explanation but buffer
IDs/generations are acceptance guards, not persistent identity. Missing versus
empty, CRLF, final-newline and mode changes remain distinct. Exact matching after
undo is allowed only after rehashing; old records never become fresh merely
because a navigation anchor, size, timestamp or status letter matches.

The Git worker calculates content and location fingerprints. Hunk signatures hash
row kinds, complete row text and final-newline metadata, excluding header line
numbers. Line signatures add an old/new side and a bounded neighboring-row window.
Duplicate signatures are removed from the actionable location map. Stored
coordinates/excerpts are display hints only. Freshness requires an exact containing
identity plus an unambiguous attachment key for ordinary Git comparisons. M7
immutable baselines additionally allow the conservative content reattachment
described below; navigation anchors never supply that evidence.

Requests additionally hold an immutable `Arc<Store>` and cheap Rope snapshots of
open documents. The request stamp covers a monotonic review generation and all captured
buffer identities/generations. Acceptance observes current stamps before polling;
review commands independently reject an obsolete comparison even before the next
tick. Results validate the current file and at most 31 other annotated paths for
the selected comparison, within the existing cooperative five-second Git budget.
Per-record validation includes its revision, so an obsolete record/result cannot
validate a newer edit. Unvalidated bases, unsupported/failed paths and work beyond
the limit remain unavailable. The original main-comparison status sampling stays
intact; auxiliary validations are further non-atomic samples. No filesystem-wide
snapshot or protection from unobserved concurrent external writes is claimed.

`review_store.rs` is a concrete worker with a bounded queue and one UI action in
flight. Loads are read-only and capped at 32 MiB; absent metadata is an empty store.
Records cap at 4,096, note text at 16 KiB, annotation context at 512 bytes. Parsing,
validation, serialization, hashing and fsync run outside input/render. Metadata is
`<absolute-git-dir>/editor-pro-review.json`; linked worktrees therefore separate
stores, and each stored identity independently checks repository/base isolation.

A create-new `editor-pro-review.json.lock` serializes cooperating writers. Under
that lock, reread and validate the current store, compare the expected whole record
and revision, merge independent changes, assign monotonically increasing IDs, sync
a private temporary file, compare bytes again, rename and sync the directory.
Duplicate mark creation and edits/removals of a changed record fail instead of
losing another writer's annotations. Corruption, unsupported versions, bounds,
symlinks, hard links and permissions fail without replacing data. No stale lock
is stolen automatically: a crashed writer requires explicit operator cleanup.
Arbitrary non-cooperating writers can race the final comparison/rename interval.
Directory-sync failures report uncertain durability, not success. The UI retains
failed actions/drafts for retry or explicit cancellation/reload. Pending actions
block normal quit; worker Drop drains accepted work after terminal restoration.

Notes swap an ephemeral `Document` into the same Editor, preserving the complete
source document/history, viewport, view, register, search, pending motion state
and origin. Ordinary modes, graphemes, paste, undo and search operate on the note.
Source/file commands are guarded until note save/cancel; failed saves leave the
draft intact. Unchanged notes can close with Escape; dirty notes require an
explicit decision. Returning restores the source and invalidates Git for a new
sample. The contextual record list navigates exact fresh attachments; stale or
ambiguous notes open their comparison with an unresolved message, never jump to
an approximate line. File and hunk reviewed counts remain independent; attention
and stale/unavailable counts are shown separately.

Export captures the loaded store and current validation results, then the worker
writes deterministic Markdown with full identities, counts, sides, bounded context
and every loaded annotation, including stale/unavailable bases. It states that
scope explicitly. HTML entities escape punctuation; controls use visible escapes
and unusual path bytes use byte notation. New-file persistence refuses all existing
outputs and destinations in the current worktree, Git metadata or open source
paths. Export never saves source, changes the index or transmits feedback.

## Immutable review baselines and live revisions

M7 adds concrete tree capture/comparison functions in `git_snapshot.rs` and a
bounded schema in `review_snapshot.rs`. No new worker, runtime, dependency or
editing engine is introduced. `:review snapshot` sends an immutable Git request
and open-buffer Ropes to the existing review I/O worker. It captures a full Git
baseline once and appends current trees. `:review full` uses that fixed baseline;
`:review since [ID]` pins one checkpoint. Trees contain exact file bytes, raw paths,
SHA-256, presence by membership, executable modes and unsaved provenance. Baseline
Git object IDs are explanatory; stored bytes remain usable if Git prunes objects.
Text retains the existing version-2 representation. Binary/non-UTF-8 files use an
optional hex-encoded byte payload with empty text; validation checks its content
hash, exclusive representation and the same decoded byte limits. Text-only stores
remain compatible; older readers reject the new binary field. Binary files take
part in delta/rename detection, but a selected binary comparison reports the
ordinary unsupported-text-diff error without blocking other paths.

Discovery includes tracked/non-ignored untracked paths, all retained paths and
named dirty buffers. Dirty text comes from request-time Ropes; clean/unopened text
comes from disk. Source documents, disk baselines, histories and viewports never
change. Two content samples must agree before capture; the initial Git baseline
is checked twice too. Baseline collection parses modes and object IDs from one
NUL-delimited tree/index listing, checks all blob sizes with `cat-file --batch-check`,
then reads them with `cat-file --batch`. The batch protocol contains only validated
object IDs; filenames and binary payload delimiters cannot become commands.
Sizes and aggregate limits are checked before reading content. Private temporary
stdin/stdout files share the existing runner's deadlines, cancellation and output
bounds. Git process count stays constant as baseline file count grows.
Bounds are 512 union paths, 2 MiB per file, 8 MiB per tree,
32 checkpoints, 24 MiB retained source bytes and 32 MiB serialized metadata.
Binary bytes count toward those limits; symlinks, submodules, conflicts and
exceeded limits refuse the whole capture. This is sampled
consistency, not a filesystem transaction. Capture has a cooperative 30-second
Git deadline; filesystem stalls and serialization/fsync are not hard-time bounded.

The first successful capture upgrades metadata to version 2, retaining all v1
annotations. Trees live in the same private worktree-specific review JSON so an
append and its baseline are one atomic write under the existing lock. The writer
checks the expected latest checkpoint and baseline, merges unrelated records,
validates content hashes/limits and fsyncs as before. Earlier checkpoints are
never updated or automatically pruned. Pending capture participates in the
existing quit guard; a failed capture requires reload/retry. No separate pointer,
object store, garbage collector or cross-file persistence protocol is needed.

The Git worker polls live trees for review comparisons, keeping the usual request,
review-generation and open-buffer guards. A separate hash of sampled disk bytes
and modes detects external revisions even under dirty buffers. This describes
content changes, not their author. `:changes` projects deltas from the selected
saved tree, separately from disk/index status. Git rename hints and unique
nonempty content matches relate missing paths; unsupported/ambiguous moves remain
additions/deletions. Original Git objects are optional rename hints. Source jumps
map disk coordinates back through a diff into retained open documents; this is
navigation only. Gutters are withheld for differing clean source buffers.

Review reattachment requires the same repository/baseline, complete old content
and blob identity, modes and presence, plus a unique unchanged hunk/line signature
on the current comparison. Whole-file marks remain stale after content/path
changes. Original-base annotations can be projected into the full snapshot view;
other comparisons remain isolated. The stored record is not rewritten: current
location/progress are derived and still guarded by record revision and sampled
content. Repeated or changed signatures cannot count as fresh. Markdown exports
include checkpoint metadata and historical annotation context, never the stored
source trees. Validation keeps the current-plus-31-path budget.

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
and report changes without replacing text, including the source held behind a
review-note draft.

`disk.rs` has one lazily started worker, one replaceable pending job and one result
slot. Every two seconds (or on focus), the UI schedules up to 16 named documents
in rotating stable-ID order. Requests share immutable `Arc` disk baselines;
reads, byte comparisons and SHA-256 observations run on the worker, bounded to
64 MiB per file. No timestamp shortcut can hide same-size/restored-mtime writes.
Document path, buffer instance and a baseline generation reject obsolete results
after reload, close, save or save-as, including directory-sync failures. New
content/error observations invalidate Git, hence review freshness and gutters,
even while a document is already flagged. Only the first conflict transition
posts a message; polling never opens a prompt or reloads text. Unopened repository
paths remain covered by the existing Git/review sampling.

`:refresh` opens a read-only overlay over the current context and requests a
buffer-to-disk comparison. It reuses the existing Git diff runner and its content,
output, cancellation and timeout limits, including outside repositories. A missing
file is explicitly labeled, and comparison errors do not hide reload/cancel
choices. Reload is an explicit action; dirty text needs a separate confirmation.
The worker requires two identical samples 150 ms apart, compares them to the
preview bytes and constructs the replacement document off the input thread.
A final read checks preparation races. UI acceptance also checks the buffer
mutation generation. Missing, changing, unsupported or obsolete results preserve
the source. A rename is a disappearance at the original path, never implicit
path adoption. Cursor line/scalar column and viewport survive reload where
possible; the new buffer clamps to a grapheme boundary and gets a fresh history.
Cancel retains the original undo group and context. Help and refresh defer
navigation-producing worker replies; the disk worker itself continues running.
Cancellation/shutdown signals the read-only worker without waiting on filesystem
stalls. The bounded queue prevents blocked reads from spawning more workers.

The final check and rename cannot atomically compare-and-swap against arbitrary
external writers. Symlink/hardlink restrictions and metadata limitations are
documented in the README. Atomic rename is not an autosave or crash-recovery
system. Ordinary open/save and edit-approval checks remain synchronous and
bounded to 64 MiB. Background polling and refresh remove their own disk reads
from input/render, but cannot promise hard deadlines for filesystem calls.

Terminal setup has a scope guard created before raw mode starts. Both screen
state and raw mode restoration are attempted on errors. A panic hook restores
before printing the panic. Cursor shape and bracketed-paste/focus reporting are
restored too. Signals/job control require a separate future hardening change.

Pathless startup shows themed ASCII artwork over the empty unnamed buffer.
The event loop owns this one-shot presentation flag; the first decoded key or
paste clears it and follows normal input handling. Resize, focus and worker
events keep it visible while the existing status/message rows remain live.
Rendering does not create text, undo history or a new editor mode/view. Named
files and directory pickers bypass the splash, and small terminals use compact
text. There is no timer or extra keypress before editing.

## Language-server integration

One manager owns at most eight sessions keyed by project root and language.
Directory/no-argument startup queues optional `lsp.warmup` (default true) only
after the first terminal frame. A latest-value directory channel feeds one
cancellable blocking scan owned by the same manager. The scan uses the nearest
Git boundary (including worktree files), or nearest build marker outside Git,
and examines at most two child levels, 64 directories, 4,096 entries, 512 KiB of
marker content (64 KiB/file), and one second between filesystem calls. Hidden,
generated/vendor directories, nested repositories and directory symlinks are
skipped. Filesystem calls themselves are not hard-time bounded. Gradle Kotlin DSL
filenames alone are not Kotlin evidence. Conservative layout/plugin hints feed
the existing root resolver, including root overrides; no alternate root policy
or second server manager is used. More than two candidate sessions or exhausted
scan budgets fall back to demand-driven startup.

Warm sessions have empty document snapshots, use ordinary startup/install/retry
and shutdown paths, and are keyed exactly like sessions for opened documents.
Opening a matching source claims its session, after which closing its last
document retains normal teardown behavior. Other warm sessions are evicted before
denying capacity to source documents. Replacing/cancelling preparation invalidates
scan results, closes unused warm sessions and cancels installations initiated only
for them; an open document or explicit install command retains a shared install.
Session-membership events let the existing statusline/info view display progress
before any source opens. Phase timings in bounded session logs distinguish
executable discovery, runtime probing, process spawning, initialize response and
first document synchronization; initialize completion does not prove indexing is
finished. Explicit file startup and library editors remain lazy unless preparation
is explicitly requested.

A latest-value watch channel coalesces immutable Rope snapshots; bounded channels
carry commands and results. Discovery, process spawning, protocol framing,
serialization, synchronization and SDK probes run on its private Tokio thread.
Each session has separate asynchronous stdout/stderr readers, so a stalled pipe
does not block editing or another server's I/O. No generic service/backend layer
was added. Ordinary document loading, edit-preview validation and safe-save I/O
remain synchronous, under the existing file limits and a 2 MiB LSP document limit.

Default PATH argv uses official Kotlin `intellij-server --stdio`, Java `jdtls`, and
Roslyn-based `csharp-ls`. Environment JSON argv overrides require no shell.
Discovery prefers overrides, executable PATH servers, then a complete managed
installation. Broken explicit overrides are errors, not installation triggers.
Startup and Doctor probe the selected runtime with a three-second deadline and
bounded output; wrapper overrides supply their own runtime. Servers, rather than the editor,
interpret project toolchains and perform dependency imports. Discovery uses
Gradle settings, the nearest Maven POM, or a containing C# solution/project,
stopping at a Git boundary. Ambiguous solutions fail explicitly; a root override
plus server arguments can select a solution. Each JDT session has private temporary
workspace data, removed at shutdown; it does not persist warm indexes between runs.
JDT initialization/configuration explicitly enables signature help, which its
default preferences disable despite advertising the capability. These concrete
server settings do not introduce a general configuration framework.

`main.rs` alone resolves the native user data directory and enables the configured
`lsp.auto_install` policy (default true). Library editors default to no installation
directory and no automatic downloads. Missing servers trigger one attempt per
language per editor process; `:LspInstall [LANGUAGE]` retries explicitly and
`:LspInstallCancel [LANGUAGE]` cancels the shared language job. Progress uses
latest-value channels, separate from request deadlines and document epochs.
At most three installation threads exist. A per-language OS file lock coordinates
independent editors and releases on process death. Jobs have cooperative ten-minute
deadlines; runtime/install subprocesses have bounded captured output and their own
process groups. HTTP connect/resolve waits are five seconds, response/idle reads
ten seconds; cancellation is checked between reads/files. ureq is pinned exactly
because its idle-read adapter uses the unversioned transport API. TLS verification
and HTTPS-only redirects remain enabled. Shutdown cancels and joins install jobs
after terminal restoration, subject to network/filesystem stalls.

The catalog pins version, URL, SHA-256, archive format and platform. Managed
installations support macOS ARM64 and glibc Linux x86_64/ARM64. Linux uses
`$XDG_DATA_HOME/editor-pro/lsp` or `~/.local/share/editor-pro/lsp`; macOS uses
Application Support. Downloads cap at 1 GiB; extraction at 100,000 entries/4 GiB.
Absolute/parent paths, special files, duplicate regular files and escaping links
are rejected; all links are deferred until regular files finish. Extraction happens
in a private staging directory. A bounded receipt and atomic rename publish a
complete installation; stale staging directories are removed under the lock on
retry. Managed versions are retained and never modified by another install job.

Kotlin uses its bundled JBR, including macOS ZIP-format `.sit` archives. Managed
JDT uses Java directly with shared read-only distribution configuration and private
per-session configuration/data. csharp-ls is installed from a checksum-verified
NuGet package with a private local-only feed, SDK cache and tool path; it runs via
the selected dotnet host, with major runtime roll-forward. No project NuGet config
or tool manifest participates in installation. JDK 21+ and .NET SDK 10+ are
prerequisites; project toolchains remain separate. On completion all sessions of
the language restart from current Rope snapshots, preserving unsaved edits.

The [LSP 3.17 protocol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
uses UTF-16 positions here. Surrogate splits, CRLF interiors and out-of-range edits
are rejected. Full synchronization and incremental range replacements both work.
Incremental changes are computed between Rope snapshots, so LSP never consumes
the syntax edit queue. A monotonic buffer instance ID distinguishes reloads with
equal buffer-slot IDs/generations; a document save counter drives didSave.
Reload/save-as/close produce the appropriate close/open events. Review notes keep
synchronizing their retained source, never the note draft.
Versions advance by one for each synchronized change to a document. A session
maximum seeds newly opened paths above prior versions, including closed paths;
versions remain checked alongside instance/generation guards. This avoids
cross-document version jumps rejected by the tested Kotlin server.

Initialization negotiates static capabilities and has a 180-second deadline;
feature requests default to 15 seconds. Cancelled/obsolete requests are removed,
and responses carry session epoch, request serial, source instance/generation and
captured document versions. Server command deadlines pause while their edit
preview awaits user input. UI acceptance checks the active buffer, path, cursor,
context and all captured open-document stamps before displaying or applying work.
Crash recovery retries twice, reopening current unsaved snapshots; explicit
restart resets the session. Shutdown closes documents, sends shutdown/exit,
closes stdin so stdio transport loops can finish, then kills/reaps remaining
processes (and the Unix process group). The manager
joins at editor destruction after terminal restoration.

Startup visibility uses typed Starting/Indexing/Ready/Degraded/Failed states on
that same session, with worker timestamps and bounded work-done progress keyed by
string or numeric token (distinct namespaces). At most 32 tasks retain 512-character
titles/messages and server-supplied percentages; tokens cap at 1 KiB. Unknown end
notifications and partial-result progress cannot complete other work. Initialization
never clears active tasks; finishing tasks before initialize cannot claim readiness.
Ready explicitly means protocol readiness with no reported work, not proven complete
project indexing. JDT `language/status` startup/error notifications and typed
window errors/warnings feed the same state. Issues remain Degraded until explicit
restart so ordinary progress cannot hide an import/SDK error. Unsupported client
actions in server messages are not executed.

Lifecycle/progress status delivery waits for bounded queue capacity, cancellable
by the session stop signal, so Help or notification bursts cannot discard the final
state. Logs remain lossy and bounded. Stopping invalidates capabilities/diagnostics
before process teardown; explicit restart also rejects queued results from the old
epoch immediately. The UI redraws active elapsed time once a second, and refreshes
LspInfo/LspLog while preserving their scroll position. No process, SDK probe or
indexing work runs on the input/render thread.

Headers cap at 8 KiB and messages at 8 MiB; command/frame queues have 32 slots,
the UI result queue 128. Stderr/messages retain sixteen chunks of at most 4,096
characters per session. Queue saturation and timeouts fail visibly. Pull
diagnostics are tied to captured versions; push diagnostics with old versions are
discarded. Versionless push diagnostics have no protocol freshness proof and are
explicitly labelled unverified and invalidated on known edits.
The client advertises diagnostic refresh support, cancels superseded pulls and
requests fresh reports after indexing refreshes. Servers declaring inter-file
dependencies also trigger repulls after other synchronized documents change;
those reports retain the captured session document stamps through UI acceptance
and display invalidation.
No dynamic capability registration, watched-file registration or semantic tokens
are advertised. C# enables its metadata-URI extension and configuration setting.

Language results use one contextual list with source-preserving scroll/selection.
Insert-mode completion additionally uses a cursor-adjacent popup. Its full area is
cleared before drawing so source text cannot show through the suggestions or hint;
dismissal restores the source on the next frame. Actual character edits and
deletions schedule one replaceable request after 120 ms of quiet;
identifier prefixes and negotiated trigger characters drive requests, including
incomplete-list refresh context. Results are filtered by filterText/label and the
server edit range, sorted by sortText, and bounded by the existing 4,096-item cap.
Typing keeps the Editing view and Insert mode. Movement, paste, dismissal, mode
and buffer changes cancel scheduled/pending work; ordinary context, session,
cursor and all-document guards reject stale replies and resolution results.
Server-reported automatic request failures stay in logs; explicit acceptance errors are
visible. The event loop polls at 16 ms while completion is scheduled or pending.
Tab/Enter accepts a selected completion in either list; arrows/Ctrl-n/Ctrl-p
navigate. Escape/Ctrl-e dismiss the popup while retaining Insert; Escape with no
popup exits Insert. Only an explicitly accepted, single-line edit whose range
contains the request cursor on the same line applies directly, using
the same complete edit-plan validation, disk conflict checks and undo transaction
as a preview. Server ranges can include punctuation and use the replace range of
an insert/replace edit; implicit ranges use the identifier prefix. Additional edits,
multiline replacements and server commands retain the preview path, which still
requires Enter to approve. Explicit :Complete/Ctrl-n without a popup retains the
contextual list and restores its originating mode on acceptance or dismissal.
Snippets and commit-character acceptance remain unsupported.
Completion resolution supplies documentation and additional edits; snippets are
refused. Advertised completion commands are shown in the initial edit preview
and run only after approval. Their subsequent server edits require another
guarded preview, reusing the code-action continuation path. Hover/signatures, definitions/references,
document symbols and diagnostic navigation operate on the ordinary documents.
Normal-mode `gd`/`gr` reuse the definition/reference requests, marking their
request context to jump directly when exactly one supported location is returned.
Multiple results use the contextual chooser; explicit `:Definition` and
`:References` retain their lists. Both paths use the same guarded location jump
and session navigation history. The request marker is subject to the existing
serial, session, cursor and document guards, so obsolete replies cannot navigate.
C# definition/reference targets using `csharp:/` are fetched with `csharp/metadata`
on the same worker, with the existing serial, epoch, document, cursor, cancellation
and deadline guards. A null/invalid/oversized response or invalid UTF-16 position
leaves the source/history untouched. `Document` stores the original virtual URI,
no disk path or baseline, and a read-only Buffer. Mutation entry points and saving
refuse changes; ordinary movement, search, copy, syntax, buffer switching and
back/forward history remain available. No temporary source files are created.

The LSP view binds each virtual buffer instance to its owning server and epoch.
Snapshots use an opaque relative routing token distinct from absolute file paths;
`Snapshot::uri()` supplies the original URI. Manager routing bypasses filesystem
root discovery for these snapshots. The worker tracks them for request freshness
and session lifetime without sending didOpen/didChange/didSave, pull diagnostics
or edits to the server-owned source. Definition/reference requests from them use
the original URI. Ordinary disk, Git, edit guards and recent-file discovery see
no path. At most 32 virtual buffers retain 2 MiB each. Repeated jumps fetch again
and reuse the same session/URI buffer slot, replacing changed generated text.
Old-epoch buffers remain readable after restart but cannot issue navigation
requests; a fresh jump from source obtains a new snapshot. Closed virtual buffers
cannot be reopened through file history. Other virtual URI schemes, workspace
symbols and call-hierarchy virtual targets remain explicitly unsupported.

M6 reuses that list for the nested symbol outline, workspace symbols and call
hierarchy. Workspace symbols use the active project/language session and negotiate
lazy `location.range` resolution. Call items preserve the server's opaque data
through incoming/outgoing requests; recursive browsing retains at most 16 parent
panels and restores their selection/scroll. Enter jumps to declarations; call-site
ranges are displayed in caller coordinates. Existing session/request/source/open-
document guards apply before expansion, resolution or navigation. Capability
absence and virtual/decompiled locations remain explicit. These are read-only
features, with no new server-edit authorization path.

Rename, formatting, completions requiring review and quick fixes create an edit
proposal. Validate every URI, project boundary, known version, captured buffer stamp, scalar range,
overlap and size before constructing a preview. All affected buffers and disk
baselines are rechecked before any splice. Applying makes one undo group per
changed document. An open target absent from that server's synchronized snapshots
is refused, including unsynchronized documents owned by another language session.
Applying retains unopened targets as new dirty buffers; it never
saves files. Unversioned unopened targets are read when preparing the proposal:
their state before the server response cannot be proven. Resource operations,
annotated edits, duplicate entries for one URI and edits outside the project
are refused. Limits are 32 files/4,096 replacements and 2 MiB per resulting file.
Previews show up to 128 replacements per file with bounded text; truncation is
labelled. Multi-file undo remains per buffer, without a new transaction engine.

Only a selected, advertised code-action command or a preview-approved completion
command can authorize a server
workspace/applyEdit request, and it still requires a preview. Unsolicited, stale
or concurrent server edit requests receive applied=false. Applying a proposal
invalidates Git/review derived state through the established paths.

## AI context and approval boundary

M8 uses documented `codex exec` rather than implementing another interactive
agent protocol. The installed 0.162.1 CLI and an actual provider passed a small
Linux fixture; protocol-double coverage remains separate. See
[tools/ai/README.md](tools/ai/README.md) for the exact command, binary fingerprint,
official references and reproduction. Core features never depend on a provider.

`AiState` belongs to `Editor` and owns only captured context, conversation data,
view navigation and a pending proposal. `View::Ai` intercepts keys without editing
source. Context collection reads the active selection/file, a current LSP symbol
range/reference list or diagnostic set, or an existing fresh Git diff/review export.
Optional test files are selected explicitly, not discovered or executed. Review
note drafts cannot become AI source context. The preview shows the question,
attachments, provenance and number of prior turns that will be sent; the
conversation view exposes every prior turn's context. Only `:ai send` launches
inference. Unsaved text comes from open buffers, never substituted disk reads.

`Ctrl-g` and `:ai explain` prepare a default or custom explanation question using
the same draft and preview. Visual capture uses the exact live selection and
clears its anchor when entering AI, so a later command cannot reuse an abandoned
selection. Outline capture uses the selected result; Normal mode requests document
symbols through the existing LSP worker and picks the smallest validated range
containing the original cursor. LSP serial, document, cursor and mode guards discard
late replies after movement, edits, buffer switches or cancellation. No Codex
request starts during lookup or preview. Unsupported/missing symbols ask the user
to select code explicitly.

Previews identify the repository/project, half-open line/scalar-column range and
source hash. Git capture includes the exact repository/worktree, paths, comparison,
resolved base commit, blob IDs and full content hashes. A runtime comparison
identity is retained with the draft and rechecked against current Git state before
sending; adding context or changing the question does not discard that guard.
The hunk shortcut additionally captures the matching new-side source range when
its complete working content matches the comparison. This permits ordinary source
citations without mislabelling a differing index/historical side as working text.
Deleted files and differing sides retain diff-only evidence. Repository roots
take precedence over nested Cargo manifests when starting a conversation.

Source attachments retain the path, full-content hash, scalar slice origin,
source line and runtime buffer instance/generation. Persistence retains content
and provenance but drops live edit authority. Each response citation/replacement
names a one-based context ID scoped to that turn, never a new filesystem path.
Source jumps require unchanged full text, validate the captured range and use the
existing prepared-buffer/history path. Diff/diagnostic/feedback rows are captured
evidence with no source jump or editing authority. Model observations and cited
evidence are displayed separately from inferred intent; semantic claims are not
automatically verified. Editing while waiting preserves a historical answer;
source links and proposed edits independently reject changed targets.

One lazily started worker owns one job/result slot and one replaceable progress
slot. Inference is cancellable by serial, never by joining on the input thread.
Subprocess stdout/stderr use private regular files; stdin is a bounded private
prompt file, avoiding blocked writes and reader-thread accumulation. JSONL is
parsed incrementally with bounded events and output. Complete structured message
updates may be shown before process exit; only `turn.completed` plus successful
exit makes an answer actionable. This exec interface does not guarantee token
streaming. Missing CLI/authentication, unsupported output, provider failures,
timeouts and attempted tool calls fail visibly. Unix process groups are killed
and reaped on completion/cancellation. Shutdown joins after terminal restoration;
accepted transcript I/O drains, while inference is cancelled.

The CLI runs outside the source tree in a private temporary directory, with
read-only sandboxing, approval policy `never`, shell tools disabled, web search
disabled, and apps/hooks/remote plugins/memories/subagents disabled. User/project
CLI configuration and execpolicy rules are excluded; authentication and managed
policy remain Codex's responsibility. There is no fallback to weaker permissions.
Provider-side edits/commands are unsupported. A structured suggestion is data,
not a command to execute or a patch applied by the provider.

`:ai edits` validates unique exact original text within captured source slices,
complete source identities, runtime stamps, nonoverlapping scalar ranges and
extended grapheme/CRLF boundaries. It constructs the existing `EditPlan` in
`lsp_edit.rs`; `apply_edit_plan` is the shared final source transaction, while LSP
protocol validation stays in its existing preparation path. `:ai apply` is a
separate approval after preview. It rechecks all buffers and disk baselines before
any splice, keeps unopened targets as dirty buffers, provides ordinary per-buffer
undo, and never saves. There are no new file resource operations, shell approvals
or cross-buffer undo machinery. Reloaded transcripts require freshly captured
context and another answer before edits can be proposed again.

Conversations save explicitly outside the source project, independently of review
metadata and Codex's ephemeral sessions. Version-1 JSON stores sent questions,
context, completed answers and failed/cancelled statuses; unsent drafts are not
saved. Follow-ups replay that bounded history explicitly. A private create-new
lock, expected-content hash, temporary file, fsync and atomic rename protect
cooperating writers. Existing unrelated destinations, symlinks, hard links,
corruption and unsupported versions fail without replacement. No lock stealing,
autosave or crash recovery is included. A non-cooperating writer can still race
the final comparison/rename; uncertain directory-sync durability is reported.
Normal quit guards unsaved sent turns and pending jobs; force quit discards memory.

Bounds are 16 attachments/256 KiB/8,192 lines per turn, 64 KiB per attachment,
2 MiB source files, 8 KiB questions, 32 turns, 1 MiB/32,768 lines per prompt,
128 KiB answers/128 replacements, and 4 MiB stored transcripts. Inference has a
cooperative 180-second deadline, 1 MiB event limit, 4 MiB stdout and 256 KiB stderr
capture limits. File output may overshoot between polls. Source reads/hashing and
proposal validation remain synchronous under these limits; worker filesystem
stalls and shutdown joins are not hard-time guarantees.

## Future boundaries, not current infrastructure

- **M5 acceptance:** Opt-in tests run the ordinary editor against pinned real
  Kotlin/JDT/Roslyn servers on disposable Gradle/Maven/solution/project fixtures.
  Protocol doubles remain separate evidence. Broader workloads, platforms and
  physical-terminal comfort require additional observations. See the server report.
- **M8 acceptance:** The actual CLI/provider fixture covers one account and small
  conversation. Broader model/managed-policy combinations, macOS and physical
  terminal use require new observations. No further provider infrastructure is
  introduced ahead of demonstrated workflows.

## Principal risks and gates

| Risk | Mitigation / next evidence |
| --- | --- |
| Unicode boundary and screen-width errors | Core tests for combining characters, emoji, CJK, CRLF and mixed edits; real-terminal checks on both OSes. |
| Lost work | Conflict tests, failed-save tests, dirty quit guards; document remaining race, metadata and crash-recovery limits. |
| Project import and SDK mismatches | Real fixture checks supplement protocol tests; SDK targets and server runtimes are recorded separately. Larger/mixed projects still need validation. |
| Server churn or proprietary protocol assumptions | Pinned standalone versions/checksums and opt-in editor checks expose compatibility failures; upgrades must rerun them. |
| Stale async state | Picker queries, Git generations, and LSP session/request/document stamps guard acceptance. Versionless diagnostics remain explicitly unverified. |
| Scope and architecture growth | M1–8 code only; one optional CLI path, no provider framework. Split modules at concrete responsibilities. |
| Platform and responsiveness evidence | Linux check counts, real fixture results and import/restart timings are recorded in VALIDATION.md. macOS, physical-terminal inspection and representative user workloads remain to be evaluated. |

## File-tree view

`file_tree.rs` owns a separate contextual `View::Tree`, toggled by Normal-mode
Space p v or `:tree`. It does not change the source viewport. Each expansion
spawns one bounded directory-read worker; only one scan may be outstanding,
including while the view is hidden. Results apply on the UI tick. No recursive
walk or recursive deletion occurs. The visible tree caps at 10,000 entries;
symlinks are leaves and ignored/hidden files are included except `.git`.

Mutations use create-new semantics, empty-directory deletion and rustix atomic
no-replace rename on Linux/macOS. Open document paths and ancestors cannot be
renamed/deleted, so buffers and LSP documents keep their existing identities.
Targets cannot collide with open documents, escape the root or traverse symlink
parents. Successful moves update closed-file navigation paths; deletions prune
those paths, and either invalidates Git. Filesystem calls and parent validation
are synchronous and may stall on slow filesystems; discovery is off-thread.
