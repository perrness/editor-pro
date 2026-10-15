# Roadmap

Build small complete slices. Acceptance requires observed behavior and checks,
not only code or an advertised server capability. Milestones 1–6 are implemented;
M5 and M6 have pinned real-server acceptance evidence on Linux. Platform and broader
project limits remain explicit below. Editing modes remain independent of
contextual views throughout. M7 has not been started.

## Milestone 1 — Minimal editor

**Status:** foundation build, formatting, Clippy, 34 Rust tests and the Linux PTY
smoke test passed before M2 work. Later PTY checks also exercised real resize
events, wide text and external-write conflicts. macOS/physical visual checks
remain open; M5 subsequently passed the real Kotlin feasibility probe.

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

**Remaining validation:** macOS/physical visual checks. The standalone Kotlin
probe result is recorded in [VALIDATION.md](VALIDATION.md).

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
separate Kotlin feasibility gate was subsequently exercised in M5.

**Remaining validation:** close those platform/physical-terminal gaps before a
broader release. Milestone 3 is implemented below; real Kotlin evidence appears
in the M5 follow-up.

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
remain unobserved; M5 now supplies the separate real Kotlin probe result.
M7 immutable baselines and automatic reanchoring remain future
work. Validation samples at most 32 annotated paths per refresh; excess paths and
other bases are visibly unavailable, and stale annotations require explicit action.

## Milestone 5 — Language intelligence

**Status:** implemented, with Linux acceptance passing against pinned real servers
on six disposable project fixtures. This closes the missing-server gate; it does
not establish support for every project, framework or platform.

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

**Implemented:** a bounded background manager with independent project/language
sessions, Gradle/Maven/solution/project discovery and explicit root/argv overrides;
SDK/server discovery and Doctor probes; static capability negotiation; asynchronous
import progress/logging; full/incremental UTF-16 synchronization; open/change/save/
close, reload/save-as and unsaved restart/resync; cancellation, deadlines and bounded
automatic crash recovery. Buffer instance IDs prevent equal-generation reloads
from accepting old replies. Review note drafts never enter source synchronization.

Completion/documentation resolution, hover, signature help, definitions,
references, push/pull diagnostics with list/next/previous navigation, rename,
quick fixes/code actions, formatting and document-symbol lists use contextual
views. `:LspInfo`, `:LspLog`, `:LspRestart` and `:Doctor` expose state.
Edits require a preview, validate all affected documents before applying, retain
unopened targets as dirty buffers, preserve per-buffer undo, and never auto-save.

Real-server testing corrected document-local version progression, indexing-driven
diagnostic refresh and dependency invalidation, command-driven completion approval,
JDT signature-help configuration and stdin closure during shutdown. Completion
commands share the existing guarded code-action/edit path; their further edits
require a separate preview. No general configuration or backend framework was added.

**Evidence:** 35 Rust LSP regressions exercise framed stdio subprocesses,
all three language IDs and separate roots, delayed/stale responses, versioned
diagnostics, missing servers, malformed protocols, unsupported capabilities,
timeouts, crash/restart/resync, UTF-16/CRLF, full/incremental synchronization,
multi-file edits and source/note isolation. These are labelled test doubles.
Final full-suite and terminal results are recorded in [VALIDATION.md](VALIDATION.md):
186 deterministic Rust tests, eight Python probe tests and 18 PTY sessions pass.
Six additional opt-in tests drive the actual editor against official Kotlin
263.6379.0 and JDT LS 1.60.0 on Maven and multi-module Gradle projects, and
Roslyn-based `csharp-ls` 0.28.0 on `.csproj` and multi-project `.sln` fixtures.
They exercise import, diagnostics, completion resolution/application, hover,
signature help, definitions/references, symbols, quick-fix previews, formatting,
multi-file rename, cancellation, undo, unsaved restart/resync and unchanged disk
bytes. The independent Kotlin probe also passes, including clean shutdown/exit.
Pinned archives, checksums and reproduction instructions live in
[tools/lsp](tools/lsp/README.md).

**Remaining validation:** macOS and physical-terminal comfort, larger production
projects, mixed Kotlin/Java, real-server crash injection and memory profiling.
Fresh-project and restart timings use cached build dependencies; initial downloads
can exceed feature deadlines. A cold JDT import did so before a cached retry
passed. SDK targets and the tested runtime versions are recorded in VALIDATION.md.
KMP, Android, Razor and older .NET framework targeting remain untested.

**Known scope limits:** manual completion, plain documentation, static capabilities,
no snippets, virtual documents, file resource operations,
annotated edits or watched-file registration. Versionless push diagnostics are
visibly unverified. Limits: eight sessions, 2 MiB documents, 8 MiB messages and
32-file/4,096-replacement proposals; undo remains per buffer. Unopened unversioned
edit targets are guarded from proposal preparation onward, not before the response.
JDT indexes are temporary per session. See README for setup and manual checks.

## Milestone 6 — Code comprehension

**Status:** implemented. Inspection confirmed the M5 client and real-server
fixtures, and all 186 starting deterministic Rust tests passed. The nested
document-symbol list already existed; this milestone extends that implementation.

Symbol outline, workspace symbols and call hierarchy where supported; ripgrep
project search; recent files; backward/forward navigation preserving locations.

**Acceptance:** follow a multi-file execution path and return to each starting
location; search/navigation never blocks editing. Unsupported server features
have an explicit, useful response.

**Delivered incrementally:**

- Source location history and recent files: `Ctrl-o`/`:back`, `Ctrl-i` (Tab)/
  `:forward`, and `:recent`. Successful file/buffer/result jumps retain source
  cursor and viewport; dirty documents and independent undo are preserved.
- `:grep TEXT` runs literal disk searches in a bounded, cancellable ripgrep
  worker. `:grepdir` selects a root. Results preserve raw Unix filenames,
  convert byte columns safely, and validate the matched line before navigation.
  Missing rg, errors, timeouts, limits and obsolete requests are handled explicitly.
- `:Outline`/`:Symbols`, `:WorkspaceSymbols [QUERY]`, and `:Calls` reuse the LSP
  view and capability/session/document guards. Workspace symbols support lazy
  location resolution. Incoming/outgoing calls recurse with a bounded parent
  stack; Backspace restores the parent selection and Enter opens source.

**Acceptance evidence:** 205 deterministic Rust tests pass, including 42 LSP,
six navigation and six project-search regressions. Tests cover cancellation,
out-of-order responses, UTF-16/UTF-8/CRLF, unusual filenames, dirty/external edits,
undo, history branching, closed/save-as buffers, limits and narrow rendering.
Extended opt-in editor tests pass with pinned official Kotlin 263.6379.0 and
JDT LS 1.60.0 on Maven projects, and csharp-ls 0.28.0 on a `.csproj` fixture.
All three returned useful workspace symbols and incoming/outgoing calls and
passed source-location restoration plus the existing M5 checks. Formatting,
Clippy, eight Python tests, debug/release builds and 20 PTY sessions also pass.
Protocol-double
results remain distinct. See [VALIDATION.md](VALIDATION.md) for terminal evidence,
commands, import environment and remaining validation.

**Known limits:** history (256 locations per direction) and recent files (100)
are session-local; scalar locations clamp after edits without tracking symbols.
Search is literal, case-sensitive, disk-only, and limited to 2 MiB files,
4,096 matches, 16 MiB captured output and a cooperative ten-second deadline.
Its line check is not an atomic filesystem snapshot. Ordinary file loads remain
synchronous under the existing size limit. Workspace symbols use the current
project/language session; call lists cap at 4,096 rows and 16 parent levels.
Virtual/decompiled documents remain unsupported. M6 real-server runs did not
repeat the Gradle/solution fixtures; macOS, physical-terminal comfort and large
production-project search/navigation remain manual validation. No M7 snapshots,
live-agent refresh, persistent navigation format or AI features were introduced.

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
