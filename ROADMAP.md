# Roadmap

Build small complete slices. Acceptance requires observed behavior and checks,
not only code or an advertised server capability. Milestones 1, 2, 3 and 4 are implemented
in this delivery. Editing modes remain independent of contextual views throughout.

## Milestone 1 — Minimal editor

**Status:** foundation build, formatting, Clippy, 34 Rust tests and the Linux PTY
smoke test passed before M2 work. Later PTY checks also exercised real resize
events, wide text and external-write conflicts. macOS/physical visual checks and
the real Kotlin feasibility gate remain open.

- Terminal startup/restoration, open/new UTF-8 file, render/scroll text.
- Basic Vim navigation; Normal and Insert; insert/delete; grouped undo/redo.
- Save/quit and guarded single-buffer open/reload; basic statusline and theme.
- Grapheme correctness, multiline edits, safe-save conflict checks and visible
  external changes. No automatic reload of open buffers.
- Architecture/risks/library research and isolated official Kotlin LSP experiment.

**Acceptance:** reliably edit and save a source file on Linux and macOS. Formatting,
Clippy and Rust tests pass. PTY test verifies real key input, saved UTF-8 bytes,
undo/redo, dirty quit refusal and terminal restoration. Manually resize, inspect
wide text and provoke an external-write conflict. Record a real Kotlin probe run
or keep its feasibility gate explicitly open.

**Remaining validation:** macOS/physical visual checks and a real standalone
Kotlin probe. See [VALIDATION.md](VALIDATION.md).

## Milestone 2 — Modern editing

Tree-sitter highlighting (Java, Kotlin, C#, Rust, JSON, YAML, TOML, Markdown),
Visual selections, word motions/counts/composable operators/text objects,
yank/paste, search, multiple buffers, fuzzy file navigation, and improved themes
(Tokyo Night, Catppuccin and Gruvbox inspired palettes) and statusline. Add basic
splits only when a concrete workflow needs them.

**Acceptance:** comfortable everyday text editing and code reading; selections
and operators work on Unicode/multiline text; highlighting remains responsive
while typing and scrolling; switching buffers preserves edits and location.

**Delivery status:** all six M2 slices are implemented in local commits. The
Linux automated evidence covers Unicode/multiline operators, incremental syntax
versus fresh parses, search, dirty-buffer preservation, fuzzy navigation and all
three themes. All 92 Rust tests, seven Python probe tests, ten PTY sessions,
formatting and Clippy pass; debug and release builds succeed. The release timing
example checks typing/scrolling across all eight grammars and a 10,000-file picker.

**Acceptance:** implemented and supported by Linux automated checks and measured
workloads. Large Markdown is slower than the other languages; the measured limits
are recorded in [VALIDATION.md](VALIDATION.md). Full platform/interactive comfort
sign-off remains open until macOS and physical-terminal use are observed. The
missing Kotlin server executable keeps the separate feasibility gate open.

**Remaining validation:** close those platform/physical-terminal gaps before a
broader release. Milestone 3 is implemented below; it does not close the separate
real Kotlin feasibility gate.

## Milestone 3 — Git-aware editing

Branch/status indicators; added/modified/deleted gutter marks; untracked files;
HEAD/index/selected-revision diffs; changed hunks/files navigation; changed-file
list; inline and side-by-side diff with synchronized scrolling, narrow-terminal
fallback and return-to-source location. Refresh Git state asynchronously.

**Acceptance:** efficiently inspect working-tree changes without leaving the
editor. Compare against each supported base; deletion/rename/untracked cases work;
closing a diff restores the source position. Git does not depend on LSP.

**Delivery status:** implemented. `:changes`, `:diff HEAD|index|REV|staged`,
`:gitbase`, `:gitrefresh`, `:gnext`/`:gprev`, semantic source gutters, hunk/source
navigation and synchronized paired panes are available. Tab switches layouts;
widths below 100 columns fall back to inline. Escape restores the original buffer,
cursor and viewport. Disk/index status and unsaved buffer changes are distinct.

**Acceptance:** supported by Linux regression/PTY checks and a reproducible Git
responsiveness example. HEAD/index/selected revisions, staged plus working edits,
unsaved buffers, deletion/rename/untracked cases, linked worktrees, cancellation,
errors and location/history preservation are covered. No Git process work runs in
the input/render loop and no LSP is required. See [VALIDATION.md](VALIDATION.md)
for exact final counts, measurements and limits. macOS and physical-terminal
appearance remain unobserved. The M3 delivery preceded the M4 persistence, review marks and notes described below.

## Milestone 4 — Code review

File/hunk reviewed and needs-attention marks; persistent progress; notes on files,
lines and hunks; content-based staleness detection; Markdown feedback export;
improved diff navigation. Use ordinary editing modes and contextual views.

**Acceptance:** review an agent-generated change, resume later, and export useful
feedback entirely in the terminal, without AI. Tests prove changed content cannot
retain a fresh review mark accidentally.

**Delivery status:** implemented. Full structured comparison/content identities,
file/hunk states, persistent notes on files/hunks/old/new lines, explicit correction
of stale attachments, filtered navigation and deterministic Markdown export are
available. Note drafts reuse the ordinary editing engine; Git, source saves and
review persistence remain separate. Metadata is private to the worktree's Git
directory, bounded and versioned, with atomic writes and record-level conflicts.

**Acceptance evidence:** Linux Rust regression tests and a 16-session PTY workflow
exercise inspection, annotation, restart/resume, content-change staleness, useful
export, failures and terminal restoration. `review_performance` measures hashing,
progress refresh, notes, persistence and export. Exact final counts/timings are in
[VALIDATION.md](VALIDATION.md). The automated M4 workflow is complete without AI
or a language server. macOS, hosted CI, minimum-Rust and physical-terminal comfort
remain unobserved; the separate real Kotlin probe remains blocked by its missing
server executable. M7 immutable baselines and automatic reanchoring remain future
work. Validation samples at most 32 annotated paths per refresh; excess paths and
other bases are visibly unavailable, and stale annotations require explicit action.

## Milestone 5 — Language intelligence

Robust LSP lifecycle/synchronization with Kotlin (official JetBrains), Java
(JDT LS), and C# (Roslyn-based). SDK/root/server discovery, asynchronous import,
progress/status/logging, crash recovery, stale-response suppression, completion
and documentation, signature help, diagnostics/navigation/list, definitions,
references, hover, rename, quick fixes/code actions, formatting, document symbols,
and `:LspInfo`, `:LspRestart`, `:Doctor`. Gate features by negotiated capability.

**Acceptance:** open representative Gradle, Maven, `.sln` and `.csproj` projects
and use essential intelligence reliably. Test missing SDK/server, slow indexing,
crashes, restart/resync, unsaved edits, multi-file edits, UTF-16 ranges and
out-of-order results. Do not infer KMP/Android or framework support from JVM probes.

## Milestone 6 — Code comprehension

Symbol outline, workspace symbols and call hierarchy where supported; ripgrep
project search; recent files; backward/forward navigation preserving locations.

**Acceptance:** follow a multi-file execution path and return to each starting
location; search/navigation never blocks editing. Unsupported server features
have an explicit, useful response.

## Milestone 7 — Review snapshots

Immutable review baselines, full versus since-last-review diffs, detect external
agent revisions, preserve/re-anchor notes where valid and mark stale progress.
Live filesystem/Git refresh must preserve unsaved text and viewport state.

**Acceptance:** review successive revisions without re-reviewing unchanged code.
Snapshot creation never changes the working tree. Content-based invalidation,
renames/deletions, external refresh and dirty buffers have regression tests.

## Milestone 8 — AI-assisted comprehension

Investigate and integrate one officially supported Copilot CLI or Codex CLI path.
Context preview for selected code, symbol, file, hunk/diff and optional references,
tests and diagnostics; conversation view, streaming if supported, source links,
persistent conversation context and sending exported review feedback.

**Acceptance:** inspect changes, ask contextual questions, navigate cited code
and direct revisions in the terminal. Verified observations and inferred intent
are distinct. Comprehension is read-only by default; edits/destructive operations
need approval; unsaved buffers cannot be overwritten. All core workflows remain
usable without an AI provider.
