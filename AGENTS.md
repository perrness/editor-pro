# Repository Guidelines

## Project Structure & Module Organization

`editor-pro` is a single Rust 2024 Cargo package for a Vim-inspired terminal editor.

- `src/main.rs` manages terminal lifetime and the event loop; `src/lib.rs` exposes editor modules. Core editing lives in `buffer.rs`, `editor.rs`, `motion.rs`, and `file.rs`; Git, review, LSP, and rendering have separate modules.
- `tests/` contains integration suites grouped by feature, such as `editing.rs`, `rendering.rs`, and `lsp.rs`.
- `examples/` contains editing, Git, and review performance workloads.
- `tools/` contains the terminal smoke harness, LSP test doubles, and Kotlin probe fixtures. Syntax queries come from grammar dependencies; palettes live in `src/theme.rs`.
- Read `ARCHITECTURE.md` for invariants, `VALIDATION.md` for acceptance evidence, and `docs/LANGUAGE_SERVERS.md` for server setup.

## Build, Test, and Development Commands

Use stable Rust (1.88+), rustfmt, Clippy, Git, and Python 3. Interactive execution requires a terminal.

- `cargo run --locked -- src/main.rs`: open a file during development.
- `cargo build --locked`: build the debug binary; add `--release` for optimized builds.
- `cargo fmt --check`: check formatting; use `cargo fmt` to apply it.
- `cargo clippy --locked --all-targets -- -D warnings`: lint with warnings treated as errors.
- `cargo test --locked --all-targets`: run Rust tests across targets.
- `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v`: test the probe harness.
- `python3 tools/terminal_smoke.py`: exercise real terminal sessions after the debug build.
- `cargo run --locked --release --example performance`: measure editing/rendering workloads.

## Coding Style & Naming Conventions

Follow rustfmt defaults and four-space indentation. Use `snake_case` for modules/functions, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Unsafe Rust is forbidden. Keep buffer/file logic independent of terminal rendering. Distinguish Unicode scalar indices, UTF-8 bytes, UTF-16 positions, and terminal cells; preserve grapheme boundaries and existing file bytes.

## Testing Guidelines

Use Rust `#[test]`, Ratatui `TestBackend` for rendering, and Python `unittest` for probe tests. Name tests descriptively in `snake_case` and place regressions in the matching feature suite. Cover relevant Unicode, CRLF, undo, save-conflict, and asynchronous-result cases. CI runs on Linux and macOS; no numeric coverage threshold is configured. Protocol doubles do not establish real-server compatibility.

## Commit & Pull Request Guidelines

Follow history's imperative prefixes: `feat:`, `fix:`, `docs:`, and `chore:`. Keep changes focused. PRs should describe behavior, link relevant issues, report validation commands/results and untested platforms, and include terminal captures for visible changes. Update usage or architecture documentation when behavior or boundaries change.
