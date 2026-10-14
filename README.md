# editor-pro

A terminal-native, Vim-inspired code editor in Rust, focused on editing and
understanding code. This initial implementation is **Milestone 1 only**: one
buffer, plain text, basic modal editing, and conservative file saving.

**Validation status:** the Rust implementation has not been compiled in the
delivery environment: Rust/Cargo are absent and shell network access is
restricted. The Kotlin probe's seven Python tests pass; a real Kotlin server is
not installed. Milestone 1 acceptance and Kotlin runtime feasibility remain
unverified. See [VALIDATION.md](VALIDATION.md) for exact results and next checks.

## Build and run

Install stable Rust with Cargo, rustfmt, and Clippy. Rust 1.88 or newer is required
by the selected Ratatui release. Use a terminal on Linux or macOS.

```sh
cargo build --release
./target/release/editor-pro path/to/Main.kt
```

For development:

```sh
cargo run -- src/main.rs
```

Omit the path to start an unnamed buffer. A nonexistent file in an existing
directory opens as an empty buffer. `--help` and `--version` work without a TTY.
Use `editor-pro -- -filename` for a name beginning with `-`.

## Keys

| Mode | Keys | Action |
| --- | --- | --- |
| Normal | `h j k l`, arrows | Move by grapheme or line |
| Normal | `0`, `$`, `gg`, `G` | Line start/end, first/last line |
| Normal | `Ctrl-u`, `Ctrl-d`, Page Up/Down | Move half a screen |
| Normal | `i`, `a`, `I`, `A` | Insert before/after cursor, at first nonblank/end |
| Normal | `o`, `O` | Open a line below/above and insert |
| Normal | `x`, Delete | Delete the grapheme under the cursor |
| Normal | `u`, `Ctrl-r` | Undo/redo |
| Insert | Text, Enter, Tab | Insert text, newline, literal tab |
| Insert | Backspace, Delete | Delete graphemes; join lines at boundaries |
| Insert | Arrows, Home, End | Move and start a new undo group |
| Insert/Command | Escape, `Ctrl-[`, `Ctrl-c` | Return to Normal |
| Normal | `:` | Open command line |

An Insert session is one undo step; movement and bracketed paste create separate
groups. History retains up to 1,000 undo groups in memory. Bracketed paste is
accepted only in Insert and cannot execute editor commands. Tabs display at
four-cell stops. No autoindent is applied yet.

| Command | Action |
| --- | --- |
| `:w` | Save the current file |
| `:w path` | Save to a new path and adopt it as the current file |
| `:q` | Quit; refuse if there are unsaved changes |
| `:q!` | Explicitly discard unsaved edits and quit |
| `:wq [path]` | Save, then quit only if saving succeeds |
| `:e path` | Open a file; refuse if current edits are unsaved |
| `:e` | Reload the current file if the buffer is clean |
| `:e! [path]` | Explicitly discard local edits and reload/open |
| `:help` | Show the compact key reference |

Paths can contain spaces; enter the path directly, without shell quoting. Tilde,
environment variables, wildcards, and shell commands are not expanded. There is
no force-write command. To preserve both versions of a conflicted file, use
`:w new-path`, then open the externally modified file separately.

## What is included

- Normal and Insert editing plus a small command prompt.
- UTF-8 text with grapheme-aware movement/deletion, tabs and wide-character
  display, horizontal/vertical scrolling, line numbers, current-line shading.
- A semantic dark palette, distinct mode colors, filename, dirty/conflict flags,
  cursor position, encoding and line-ending status.
- Rope-backed editing, grouped undo/redo, guarded quit/open, atomic replacement
  saves, and external-change checks on focus, every two seconds, and before save.
- Terminal restoration on normal exit, returned errors, partial startup failure,
  and Rust panics. The editor does not invoke external processes.

## Current limits

- One buffer/view; no syntax highlighting, Visual mode, word motions, counts,
  operators/text objects, search, registers, splits, Git, LSP, review or AI UI.
- UTF-8 only; NUL-containing files, non-regular files and files over 64 MiB are
  rejected. Existing BOMs, mixed LF/CRLF endings and final-newline presence are
  preserved. New lines follow the first existing line ending, or LF in new files.
  A trailing newline has an editable empty final row, unlike Vim's presentation.
- Symlink files are rejected (open the real target); saving hard-linked files,
  read-only files or files owned by another user is refused. Existing permission
  bits are preserved; new files use private temporary-file permissions (0600 on
  Unix). Extended attributes, ACLs and other metadata are not preserved.
- A same-directory temporary file is synced and renamed, then its directory is
  synced on Unix. Disk content is compared before writing and before rename.
  This prevents ordinary stale-buffer overwrites, but cannot exclude a concurrent
  writer in the final check/rename interval. Network filesystems may offer weaker
  guarantees. No concurrent-edit merge, swap file, crash recovery or autosave.
- Loading, saving and disk comparison are synchronous; avoid remote filesystems
  and very large files for now. Line segmentation is linear in line length.
  Terminal emoji/grapheme widths may differ from Unicode width tables. No bidi
  layout or terminal job-control/signal recovery; SIGKILL cannot restore a TTY.

## Development checks

```sh
cargo fmt
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
python3 tools/terminal_smoke.py
```

The first successful Cargo resolution should generate `Cargo.lock`; commit that
file after checking the resolved graph, then use `--locked` in CI. A lockfile was
not fabricated without access to the registry. The included CI runs Rust checks
and the PTY smoke test on Linux and macOS; no CI run has been observed here.

Read [ARCHITECTURE.md](ARCHITECTURE.md), [ROADMAP.md](ROADMAP.md), and the
[language-server feasibility report](docs/LANGUAGE_SERVERS.md). The next useful
product slice is incremental highlighting for one language, after the outstanding
Milestone 1 validation and live Kotlin probe are completed.
