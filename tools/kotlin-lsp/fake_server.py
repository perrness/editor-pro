#!/usr/bin/env python3
"""Protocol test double only; this is not Kotlin validation or a language server."""
import sys
import time

from probe import encode_message, read_message


def send(message):
    data = encode_message({"jsonrpc": "2.0", **message})
    # Exercise header/body fragmentation over a real pipe.
    for start in range(0, len(data), 7):
        sys.stdout.buffer.write(data[start:start + 7])
        sys.stdout.buffer.flush()


root = None
while True:
    message = read_message(sys.stdin.buffer)
    method = message.get("method")
    if method == "initialize":
        if "--hang" in sys.argv:
            time.sleep(30)
        root = message["params"]["rootUri"]
        send({"id": "server-request", "method": "workspace/configuration",
              "params": {"items": [{"section": "intellij"}]}})
        response = read_message(sys.stdin.buffer)
        assert response["result"] == [None]
        send({"id": message["id"], "result": {"capabilities": {
            "textDocumentSync": {"openClose": True, "change": 2},
            "completionProvider": {}, "definitionProvider": True, "hoverProvider": True,
        }, "serverInfo": {"name": "TEST DOUBLE - NOT KOTLIN"}}})
    elif method in {"textDocument/didOpen", "textDocument/didChange"}:
        document = message["params"]["textDocument"]
        initial = method == "textDocument/didOpen"
        if not initial:
            change = message["params"]["contentChanges"][0]
            assert "range" in change and '"wrong"' not in change["text"]
            # A stale version must never satisfy the post-edit assertion.
            send({"method": "textDocument/publishDiagnostics", "params": {
                "uri": document["uri"], "version": 1, "diagnostics": [],
            }})
        send({"method": "textDocument/publishDiagnostics", "params": {
            "uri": document["uri"], "version": document["version"],
            "diagnostics": [{"range": {
                "start": {"line": 5, "character": 22},
                "end": {"line": 5, "character": 29},
            }, "severity": 1, "message": "Test double: incorrect type"}] if initial else [],
        }})
    elif method == "textDocument/definition":
        send({"id": message["id"], "result": [{
            "uri": root + "/src/main/kotlin/Greeter.kt",
            "range": {"start": {"line": 2, "character": 4},
                      "end": {"line": 2, "character": 9}},
        }]})
    elif method == "textDocument/hover":
        send({"id": message["id"], "result": {
            "contents": {"kind": "plaintext", "value": "greet(name: String): String"},
        }})
    elif method == "textDocument/completion":
        send({"id": message["id"], "result": {"isIncomplete": False,
                                                "items": [{"label": "length"}]}})
    elif method == "shutdown":
        send({"id": message["id"], "result": None})
    elif method == "exit":
        if "--exit-on-eof" in sys.argv:
            assert sys.stdin.buffer.read() == b""
        break
