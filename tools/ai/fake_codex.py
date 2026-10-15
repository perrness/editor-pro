#!/usr/bin/env python3
"""Codex exec JSONL protocol double. Does not establish real-provider support."""
import json
import os
from pathlib import Path
import sys
import time

args = sys.argv[1:]
assert args[0] == "exec"
for flag in ("--ignore-user-config", "--ignore-rules", "--ephemeral", "--json"):
    assert flag in args, flag
assert args[args.index("--sandbox") + 1] == "read-only"
assert 'approval_policy="never"' in args
assert 'web_search="disabled"' in args
for feature in ("shell_tool", "hooks", "apps", "multi_agent", "remote_plugin", "memories"):
    assert any(args[i:i + 2] == ["--disable", feature] for i in range(len(args)))
assert json.loads(Path(args[args.index("--output-schema") + 1]).read_text())["additionalProperties"] is False
prompt = sys.stdin.read()
assert "CURRENT TURN" in prompt
variant = args[args.index("--model") + 1] if "--model" in args else "edit"
variant, _, marker = variant.partition(":")
if marker:
    Path(marker).write_text(str(os.getpid()))


def emit(kind, **values):
    print(json.dumps(dict(type=kind, **values), ensure_ascii=False), flush=True)


emit("thread.started", thread_id="protocol-double")
emit("turn.started")
if variant in ("hang", "slow"):
    time.sleep(30 if variant == "hang" else 0.4)
if variant == "malformed":
    print("not JSON", flush=True)
    sys.exit(0)
if variant == "oversize":
    print("x" * (1024 * 1024 + 1), flush=True)
    sys.exit(0)
if variant == "error":
    emit("turn.failed", error={"message": "Fixture authentication unavailable"})
    sys.exit(0)
if variant == "tools":
    emit("item.started", item={"id": "tool", "type": "command_execution", "command": "must not run"})
    time.sleep(30)
if variant == "followup":
    assert "PREVIOUS TURN 1" in prompt and "Captured text received" in prompt
answer = {"summary": "Captured text received", "observations": [
    {"text": "The supplied source contains text.", "citations": [{"context": 1, "line": 1}]}],
    "inferences": ["Intent is uncertain from this small sample."], "edits": []}
if variant in ("edit", "slow", "multi"):
    answer["edits"] = [{"context": 1, "before": "old", "after": "new"}]
if variant == "multi":
    answer["edits"].append({"context": 2, "before": "second", "after": "revised"})
if variant == "invalid":
    answer["observations"][0]["citations"][0]["context"] = 99
if variant == "partial":
    emit("item.updated", item={"id": "answer", "type": "agent_message", "text": json.dumps(answer)})
    time.sleep(0.5)
if variant == "update-only":
    emit("item.updated", item={"id": "answer", "type": "agent_message", "text": json.dumps(answer)})
    emit("turn.completed")
    sys.exit(0)
emit("item.completed", item={"id": "answer", "type": "agent_message", "text": json.dumps(answer)})
if variant != "no-completion":
    emit("turn.completed", usage={"input_tokens": 1, "output_tokens": 1})
if variant == "nonzero":
    print("fixture failed after output", file=sys.stderr)
    sys.exit(1)
