# Roadmap

Build small complete slices. Acceptance requires observed behavior and checks,
not only code or an advertised server capability. Only Milestone 1 is implemented
in this delivery. Editing modes remain independent of contextual views throughout.

## Milestone 1 — Minimal editor

**Status:** implementation written; Rust build, lint, tests and terminal acceptance
are blocked by the current environment. Not accepted yet.

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

**Remaining validation:** install Rust, resolve/commit `Cargo.lock`, run checks,
fix any build/format/test failures, and run the standalone Kotlin probe. See
[VALIDATION.md](VALIDATION.md).

## Milestone 2 — Modern editing

Tree-sitter highlighting (Java, Kotlin, C#, Rust, JSON, YAML, TOML, Markdown),
Visual selections, word motions/counts/composable operators/text objects,
yank/paste, search, multiple buffers, fuzzy file navigation, and improved themes
(Tokyo Night, Catppuccin and Gruvbox inspired palettes) and statusline. Add basic
splits only when a concrete workflow needs them.

**Acceptance:** comfortable everyday text editing and code reading; selections
and operators work on Unicode/multiline text; highlighting remains responsive
while typing and scrolling; switching buffers preserves edits and location.

**Smallest next product slice:** Tree-sitter highlighting for Rust first, including
incremental edits and a graceful plain-text fallback, then the remaining grammars.
Keep this separate from buffer switching and Visual/operator work.

## Milestone 3 — Git-aware editing

Branch/status indicators; added/modified/deleted gutter marks; untracked files;
HEAD/index/selected-revision diffs; changed hunks/files navigation; changed-file
list; inline and side-by-side diff with synchronized scrolling, narrow-terminal
fallback and return-to-source location. Refresh Git state asynchronously.

**Acceptance:** efficiently inspect working-tree changes without leaving the
editor. Compare against each supported base; deletion/rename/untracked cases work;
closing a diff restores the source position. Git does not depend on LSP.

## Milestone 4 — Code review

File/hunk reviewed and needs-attention marks; persistent progress; notes on files,
lines and hunks; content-based staleness detection; Markdown feedback export;
improved diff navigation. Use ordinary editing modes and contextual views.

**Acceptance:** review an agent-generated change, resume later, and export useful
feedback entirely in the terminal, without AI. Tests prove changed content cannot
retain a fresh review mark accidentally.

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
