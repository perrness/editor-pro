# editor-pro

A terminal-native, Vim-inspired code editor in Rust. **Milestones 1–3 are implemented:**
incremental highlighting for eight languages, Visual selections, composable
motions/operators, search, multiple buffers, fuzzy file navigation, and three
dark themes, plus asynchronous Git status, source gutters, changed-file navigation,
and inline/paired diffs. Editing and file safety remain independent of terminal rendering.

**Validation:** the baseline and Git workflow have Linux automated coverage.
macOS, physical-terminal visual inspection, and real Kotlin LSP feasibility remain
open. See [VALIDATION.md](VALIDATION.md) for actual checks, measurements and scope.

## Build and run

Use stable Rust with Cargo, rustfmt, and Clippy (Rust 1.88 or newer), and an
interactive terminal on Linux or macOS. A truecolor terminal shows the intended
palettes. The toolchain installed on the delivery VM is available globally.

```sh
cargo build --locked --release
./target/release/editor-pro src/main.rs Cargo.toml
# Or, during development:
cargo run --locked -- src/main.rs
```

Omit paths to start an unnamed buffer. A nonexistent file in an existing
directory opens empty. Multiple paths open separate buffers; the first is active.
`--help` and `--version` work without a TTY. Use `editor-pro -- -filename` for a
name beginning with `-`.

## Editing

| Mode | Keys | Action |
| --- | --- | --- |
| Normal/Visual | `h j k l`, arrows | Move by grapheme or line |
| Normal/Visual | `w b e`, `W B E` | Next/previous word and word end; capitals use whitespace-delimited WORDs |
| Normal/Visual | `0`, `^`, `$`, `gg`, `G` | Line start, first nonblank, end, first/last line; `12G` goes to line 12 |
| Normal/Visual | `fX`, `FX`, `tX`, `TX` | Find/till character X on this line, forward/backward |
| Normal/Visual | `;`, `,`, `%` | Repeat/reverse character find; matching bracket |
| Normal/Visual | `Ctrl-u`, `Ctrl-d`, Page Up/Down | Move half a screen |
| Normal | `i a I A`, `o O` | Insert before/after, first nonblank/end; open line below/above |
| Normal | `d`, `c`, `y` + motion/object | Delete, change, yank; counts multiply (`2d3w`) |
| Normal | `dd cc yy`, `D C Y` | Line operations; D/C through line end, Y yanks lines |
| Normal | `x`, Delete, `s` | Delete graphemes; s deletes and enters Insert |
| Normal | `p`, `P` | Put the unnamed register after/before cursor or line |
| Normal | `v`, `V` | Start characterwise/linewise Visual selection |
| Visual | Motions, `o`, `d c y` | Extend selection, swap anchor, operate on selection |
| Normal | `u`, `Ctrl-r` | Undo/redo; counts are supported |
| Insert | Text, Enter, Tab | Insert text, newline, literal tab |
| Insert | Backspace, Delete; arrows, Home, End | Delete/join lines; move and start a new undo group |
| Editing/prompt | Escape, `Ctrl-[`, `Ctrl-c` | Cancel pending input or leave the current mode |

Basic text objects work after an operator or in Visual mode: `iw`/`aw`, `iW`/`aW`,
`i"`/`a"`, `i'`/`a'`, backtick quotes, and inner/around `()`, `[]`, `{}` pairs
(`ib`/`ab`, `iB`/`aB` are aliases). For example, `ciw` replaces a word and `di{`
deletes inside braces. Pair objects support nesting and multiline content; quote
objects are line-local and recognize escapes. Pair matching is lexical, so
brackets inside strings/comments can affect it.

An Insert session or change plus replacement is one undo step; movement and
bracketed paste create separate groups. History retains up to 1,000 groups per
buffer. Bracketed paste is accepted only in Insert and cannot execute commands.
Registers are shared across buffers and pasted line endings adapt to the target.
Tabs display at four-cell stops; no autoindent is applied.

## Search and navigation

Use `/text` or `?text`, then Enter, to search forward/backward. `n` repeats in
that direction, `N` reverses it, and counts such as `3n` work. Results wrap.
Search is literal, case-sensitive, single-line and non-overlapping; it never
lands inside a grapheme. Escape cancels without moving or replacing the previous
query. Search can extend a Visual selection. `:noh` hides matches until the next
search navigation. At most 100,000 matches are retained, with truncation reported.

`Ctrl-p` or `:files` opens a fuzzy picker rooted at the process working directory;
`:files path/to/project` chooses another root. Type to filter; arrows or
`Ctrl-n`/`Ctrl-p` select, Enter opens, and Escape returns to the source. Discovery
and Nucleo matching run in workers. The picker respects ignore files, includes
non-ignored hidden files, excludes VCS metadata and symlinks, and reports scan
errors or its 100,000-file limit. Matching uses Nucleo's smart case/normalization.

`:ls` opens the buffer list; use `j`/`k` or arrows, Enter to switch, Escape to
return. Switching preserves unsaved text, undo history, cursor and viewport.

## Git-aware editing

Git is optional; install Git on PATH to enable inspection. No language server is
needed. This workflow is read-only: it has no staging, committing, checkout or
reset commands. Run the editor from any directory and open a file in a repository;
discovery follows its containing repository, including linked worktrees.

| Command | Action |
| --- | --- |
| `:changes` | List repository disk/index changes and separately labeled unsaved buffers |
| `:diff [HEAD\|index\|REV]` | Compare that base with the current file's working content |
| `:diff staged` | Inspect staged changes: HEAD versus index, excluding unsaved edits |
| `:gitbase HEAD\|index\|REV` | Choose the source-gutter and subsequent diff baseline |
| `:gitrefresh` | Request fresh status and comparison data |
| `:gnext`, `:gprev` | Open next/previous changed file, retaining other buffers |
| `:diffclose` | Close a diff and restore its originating source location |

In **Changes**, `j`/`k` or arrows select; Enter opens source, or a historical diff
for a deleted file; `d` opens a diff; `r` refreshes; Escape returns. `XY` reports
index then disk status (`M`, `A`, `D`, `R`, `?`, or conflict flags; `.` is unchanged).
Renames show both paths. `B<ID>+ UNSAVED` is independent of those Git flags; a
buffer-only row can appear even when Git's disk/index status is clean.

In a **diff**, `j`/`k` or arrows scroll, Page Up/Down or Ctrl-u/d move half a screen,
`g`/`G` go to the first/last row, `[`/`]` navigate hunk headers, and `h`/`l` scroll
horizontally. Tab toggles aligned, synchronized old/new panes. Below 100 columns,
the paired view falls back to inline automatically; selection survives resizing.
`1`, `2`, `3` select HEAD, index, or staged; `:diff REV` selects a commit-ish
(e.g. `HEAD~2` or `refs/heads/main`). `n`/`p` inspect the next/previous changed
file within the diff, and `r` refreshes.

**Escape restores the original buffer, cursor and viewport**, including after
browsing other changed files. Enter instead jumps to the selected current source
line. Deleted-only lines anchor to the following surviving line, clamped to EOF;
a wholly deleted file without an open dirty buffer has no current source, so it
stays inspectable and explains why jumping is unavailable. Staged coordinates
are mapped through working/unsaved changes before jumping. These are contextual,
read-only views, not another editing mode; `:w` in a diff is refused.

Both sides are labeled with paths and resolved commit/blob identities. Working
comparisons use open buffer text, including `UNSAVED` edits; otherwise they read
disk. A clean loaded buffer may differ from externally changed disk and is labeled
as such. If the file was deleted, a clean loaded copy does not hide that deletion;
a dirty copy still participates as unsaved content. Staged comparisons always use
index content on the right. Neither comparison stages or saves anything. Unborn
HEAD is an empty baseline. Untracked files are additions. Empty/mode-only changes
have no text hunks. Missing final newlines are annotated. Binary/non-UTF-8 files,
symlinks/submodules and unresolved conflicts have explicit unsupported responses.

Source gutters use theme colors for `+` added, `~` modified and `-` deleted lines,
including EOF deletions. The statusline shows branch or detached identity and
`S` staged, `W` unstaged, `?` untracked and `!` conflicted file counts. Wider
terminals show more detail. These counts describe disk/index only; `[+]` means an
unsaved buffer. Closing a staged diff returns source gutters to HEAD.

Edits, saves, buffer/base changes and focus gain request asynchronous updates.
Idle polling notices external Git changes about every two seconds. Requests
coalesce after 75 ms of quiet; a single worker cancels superseded subprocesses and
discards obsolete results. Loading/stale/error states are visible; stale gutters
and source jumps are withheld. Refresh never reloads text or changes undo history.
`r` / `:gitrefresh` retries errors. Inspection has a cooperative five-second job
budget, 2 MiB per content side, 16 MiB command output and 100,000-file/diff-row
limits. Heavy/remote repositories may exceed them. Renames follow Git's detection;
untracked moves may appear as deletion plus addition. Submodule interiors are
ignored. Existing synchronous file loading/saving/disk checks remain separate.

## Commands and themes

| Command | Action |
| --- | --- |
| `:w [path]` | Save; an optional new path becomes the buffer's file |
| `:q`, `:q!` | Quit if **all** buffers are saved; ! explicitly discards all unsaved edits |
| `:wq [path]` | Save the active buffer, then quit only if all buffers are saved |
| `:e path` | Open or switch to a buffer, retaining edits in the one being left |
| `:e`, `:e!` | Reload the current file; ! explicitly discards its local edits |
| `:e! path` | Reload the requested target, discarding that target's edits; retain other buffers |
| `:enew` | Create an unnamed buffer |
| `:bn`, `:bp`, `:b ID` | Next, previous, or numbered buffer |
| `:ls` | Browse buffers and their dirty/disk-conflict flags |
| `:bd`, `:bd!` | Close the active buffer; ! discards its edits |
| `:files [directory]` | Open the fuzzy file picker |
| `:theme [name]` | Select a theme, or report the current/available themes |
| `:noh`, `:help` | Hide search highlights; show the compact key reference |

Themes are **tokyo-night** (default), **catppuccin**, and **gruvbox**, inspired by
those palettes. Try `:theme gruvbox`. Selection applies to syntax, search,
selections, modes and contextual views; it is not persisted across runs.

The statusline includes buffer ID, filename, dirty/disk flags, mode, language,
UTF-8/line-ending information, grapheme column and position percentage. Wider
terminals also show buffer/dirty counts and pending operators; narrow ones use a
compact layout. `[plain]` identifies a syntax fallback.

Paths can contain spaces: enter them directly, without shell quoting. Tilde,
environment variables, wildcards and shell commands are not expanded. There is
no force-write command. To preserve both versions after an external change, use
`:w new-path`, then open the externally modified file separately. Save-as cannot
overwrite an unrelated existing file or take another open buffer's path.

## Highlighting and limits

Rust (`.rs`), Java (`.java`), Kotlin (`.kt`, `.kts`), C# (`.cs`), JSON, YAML/YML,
TOML, and Markdown (`.md`, `.markdown`) use upstream Tree-sitter grammars/queries.
Edits incrementally update trees; viewport queries provide semantic theme colors.
Markdown uses the grammar's block/inline parser, including tables. Unknown file
types remain editable plain text. Files over 2 MiB and failed/timed-out parses
fall back to plain text. Parsing/query budgets are cooperative, not hard latency
guarantees. There are no embedded-language injections or semantic LSP tokens.
Large Markdown documents are slower; measured results are in the validation log.

- This is a Vim-inspired subset: no dot-repeat, named/system-clipboard registers,
  block Visual selection, regex search, macros, general splits, configuration file,
  integrated LSP, review, AI, plugins or GUI. Escape Visual mode before putting.
  Counts are capped at 10,000.
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
- Loading, saving, disk comparison and search-cache rebuilding are synchronous;
  avoid remote filesystems and very large files for now. Line segmentation is
  linear in line length. Terminal emoji/grapheme widths may differ from Unicode
  width tables. No bidi layout or terminal job-control/signal recovery; SIGKILL
  cannot restore a TTY.

## Development checks

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
python3 tools/terminal_smoke.py
cargo run --locked --release --example performance
cargo run --locked --release --example git_performance
```

`Cargo.lock` records resolved dependencies. CI is configured for Linux and macOS;
no hosted CI run has been observed here. The isolated Kotlin probe and its test
double remain outside the editor; the missing server executable blocks a real
run, not ordinary editing.

Read [ARCHITECTURE.md](ARCHITECTURE.md), [ROADMAP.md](ROADMAP.md), and the
[language-server feasibility report](docs/LANGUAGE_SERVERS.md).
