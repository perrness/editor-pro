# Delivery validation — 2026-10-14

The implementation is present, but Milestone 1 is **not yet accepted as a
validated working binary**. The host environment has Python and Java but no Rust
toolchain or Kotlin LSP executable. This distinction should remain visible until
actual checks succeed.

| Check attempted | Result |
| --- | --- |
| `rustc --version`, `cargo --version` | Rust/Cargo not found; toolchain search also found no installed binaries. |
| `cargo fmt`, `cargo fmt --check` | Blocked: `cargo: command not found`. |
| `cargo clippy --all-targets -- -D warnings` | Blocked: `cargo: command not found`. |
| `cargo test --all-targets` | Blocked: `cargo: command not found`. |
| Python probe compilation | Passed. |
| Python AST, TOML/XML, local documentation links, diff whitespace | Passed. These do not validate Rust syntax or behavior. |
| `python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v` | 7 passed; includes a protocol test double, not a Kotlin server. |
| `python3 tools/kotlin-lsp/probe.py --output /tmp/editor-pro-kotlin-result.json` | Blocked, exit 2: Kotlin LSP executable missing. |
| `python3 tools/terminal_smoke.py` | Blocked, exit 2: Rust binary has not been built. |
| Shell download reachability | `curl` failed to resolve `static.rust-lang.org`; no Rust or server install was possible. |
| Git diff/status | Initial inspection reported “not a git repository”; source files were reviewed directly. Git was subsequently initialized on `main` for the requested foundation commit. |

## Supplied checks

34 Rust tests cover Unicode/grapheme cursor invariants, CRLF joining, multiline edits,
undo grouping/savepoints/redo branches, paste isolation, command guards, disk
conflicts, save-as collisions, file permissions/link restrictions, byte-preserving
round trips, viewport behavior and narrow terminals. They are written but unrun.

The PTY test launches the actual editor, types Unicode source text, undoes/redoes,
saves/quits, exercises dirty quit refusal, compares exact output-file bytes, and
checks terminal attributes and alternate-screen/paste cleanup. It needs a compiled
binary and Linux/macOS. The CI workflow contains the Rust and Python checks for
both platforms; no hosted CI result is claimed.

## Complete these checks before acceptance

```sh
cargo generate-lockfile
cargo fmt
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build
python3 -m unittest discover -s tools/kotlin-lsp -p 'test_*.py' -v
python3 tools/terminal_smoke.py
cargo build --release
```

Resolve any compiler, formatter, lint or test failures, review the dependency
graph, and commit `Cargo.lock`. Switch CI to `--locked` after that. Record actual
Linux and macOS results here. Manually edit a Java/Kotlin/C# source file, resize
the terminal, check CJK/emoji/tabs, and modify the same file externally while the
buffer is dirty: the buffer must stay intact and `:w` must refuse the conflict.

Finally run the real Kotlin probe from
[LANGUAGE_SERVERS.md](docs/LANGUAGE_SERVERS.md). Its missing executable is an
environmental blocker, not evidence that the language server is incompatible.
