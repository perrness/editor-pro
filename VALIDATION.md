# Delivery validation — 2026-10-14

## Foundation revalidation before Milestone 2

Starting commit: `864f6ab`. Linux x86_64, Rust/Cargo 1.99.0, Python 3,
OpenJDK 25.0.4.1, Maven 3.8.7. Stable Rust, rustfmt and Clippy were installed
from the official Rust distribution. The persistent toolchain is under
`/opt/editor-pro-rust/stable`, exposed in `/usr/local/bin`; the build cache
used for these checks is `/tmp/editor-pro-cargo`.

| Observed check | Result |
| --- | --- |
| Cargo resolution and fetch | Passed; generated Cargo.lock, reviewed direct dependencies, CI now uses --locked. |
| cargo fmt / cargo fmt --check | Passed after applying rustfmt to the uncompiled foundation. |
| cargo clippy --locked --all-targets -- -D warnings | Passed. |
| cargo test --locked --all-targets | All 34 Rust tests passed. |
| cargo build --locked | Passed. |
| Python probe unittest discovery | All 7 tests passed. These exercise the harness and a test double. |
| python3 tools/terminal_smoke.py | Passed after fixing the PTY harness to answer the terminal cursor-position query. Dirty quit now explicitly checks that the process stays alive. |
| Isolated Kotlin probe | Blocked, exit 2: Kotlin LSP executable is not installed or on PATH. Report: /tmp/editor-pro-kotlin-m2-result.json. |

The initial PTY run failed because a raw PTY does not emulate terminal DSR
responses; no compiler or editing-test failures were found. The harness now
supplies the same cursor-position response a terminal emulator supplies.

Rust downloads succeeded with authorized network access. The previous claim
that downloads are impossible no longer applies. JDK 25 and Maven exist, but
no standalone Kotlin server archive/launcher is installed; no real Kotlin
capabilities or functional behavior were observed. See
[LANGUAGE_SERVERS.md](docs/LANGUAGE_SERVERS.md).

macOS, hosted CI, and interactive visual inspection in a physical terminal have
not been run here. These remain platform/visual validation limits.
