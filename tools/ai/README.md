# Codex CLI integration evidence

M8 uses one documented CLI path, `codex exec`, with JSONL events and a final JSON
Schema. No embedded SDK, second provider, provider abstraction or app-server
protocol was introduced. The local CLI's help and generated app-server schema
were inspected before selecting the smaller exec interface. Official sources:
[non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode),
[configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

Observed executable on Linux, 2026-10-15:

```text
codex-cli 0.162.1
SHA-256 628e35487d888a28f0427fc37501da529dd3d81fcbad7a2c3421c1e8670d92e1
```

This records the installed binary used for acceptance, not a distributed installer
or a promise that future CLI versions keep the same interface. Model selection
used the CLI default with existing ChatGPT authentication; no specific model
version or account entitlement is inferred from that default. Re-run acceptance
when upgrading. CLI user settings are intentionally not inherited; select an
account-supported model with `EDITOR_PRO_AI_MODEL` when needed.

The worker invokes the following argument array, without a shell, in a private
temporary directory. The prompt comes from a private file on stdin to avoid pipe
backpressure or exposing source in the process argument list. Credentials remain
managed by Codex.

```text
codex exec --ignore-user-config --ignore-rules --ephemeral
  --skip-git-repo-check --sandbox read-only --json --color never
  -c approval_policy="never" -c web_search="disabled"
  -c project_doc_max_bytes=0 -c mcp_servers={}
  --disable shell_tool --disable hooks --disable apps --disable multi_agent
  --disable remote_plugin --disable memories
  --output-schema PRIVATE_SCHEMA_PATH [--model MODEL] -
```

The documented JSONL interface emits lifecycle/item events; it is not a guaranteed
token delta API. The editor displays progress and valid complete structured
message updates, and requires `turn.completed` plus a successful process exit
before offering revisions. Failed/interrupted/incomplete responses cannot apply
edits. Unexpected tool items stop the request. No terminal escape bytes from a
provider are executed or rendered as controls.

`fake_codex.py` is an executable protocol double, used by `tests/ai.rs` and two
terminal-smoke sessions. It checks arguments and simulates streaming updates,
timeouts, cancellation, malformed/oversized output, tool attempts, invalid
citations, nonzero exits and multi-file suggestions. These tests do **not** prove
actual Codex compatibility.

The ignored `tests/ai_real.rs` test drives the ordinary editor against the actual
installed CLI and authenticated provider. It uses only a disposable greeting
fixture. It asks for cited comprehension and a revision, previews/applies/undoes
the edit, verifies unchanged disk bytes, saves a transcript, creates another
editor, reloads history and asks a follow-up. Old suggestions cannot be reapplied
after reload. Normal `cargo test` does not contact a provider.

```sh
codex login
EDITOR_PRO_REAL_CODEX=/absolute/path/to/codex \
  cargo test --locked --test ai_real -- --ignored --nocapture
```

The final 2026-10-15 actual-provider run passed in **13.95 seconds**; its two inference
requests took about **8.32 s** and **5.50 s**. An earlier run passed in 13.00 seconds.
The first sandboxed attempt could not
initialize the CLI because its own state directory was read-only. The same test
passed with normal CLI state-directory/network access; the child provider's
read-only policy was unchanged. This is evidence for one small fixture and account,
not every model, installation, managed policy or platform.

Manually exercise `:ai context file` → `:ai ask QUESTION` → `:ai send`, Visual
Ctrl-g, selected Outline symbols, hunk/diff context, optional tests/references/
diagnostics and `:ai add feedback`. Inspect the full preview before sending real
project data. Check source citations, changing code during a request, approved
edits and per-buffer undo, external-save conflicts, cancellation, missing Codex,
save/load and narrow layouts. Repeat on a physical Linux terminal and macOS.
