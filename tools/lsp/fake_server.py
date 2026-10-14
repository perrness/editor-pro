#!/usr/bin/env python3
"""Deterministic protocol test double. Not evidence of real language support."""
import argparse
import json
import os
from pathlib import Path
import sys
import threading
import time
from urllib.parse import urlparse, unquote

parser = argparse.ArgumentParser()
parser.add_argument("--log", required=True)
parser.add_argument("--scenario", default="normal")
args, _ = parser.parse_known_args()
lock = threading.Lock()
documents = {}
root = ""
executing = None
starts = Path(args.log + ".starts")
count = int(starts.read_text()) + 1 if starts.exists() else 1
starts.write_text(str(count))


def record(message):
    with open(args.log, "a") as log:
        log.write(json.dumps(message) + "\n")


record({"event": "start", "pid": os.getpid(), "count": count})


def send(message):
    body = json.dumps({"jsonrpc": "2.0", **message}, ensure_ascii=False).encode()
    data = f"Content-Length: {len(body)}\r\n\r\n".encode() + body
    with lock:
        for start in range(0, len(data), 11):
            sys.stdout.buffer.write(data[start:start + 11])
            sys.stdout.buffer.flush()


def later(message, delay=.25):
    def deliver():
        time.sleep(delay)
        try:
            send(message)
        except BrokenPipeError:
            pass
    threading.Thread(target=deliver, daemon=True).start()


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            raise EOFError()
        if line == b"\r\n":
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length))


def offset(text, position):
    lines = text.splitlines(keepends=True)
    lines.append("")
    row, col = position["line"], position["character"]
    prefix = sum(len(line) for line in lines[:row])
    units = 0
    for i, ch in enumerate(lines[row]):
        if units == col:
            return prefix + i
        units += len(ch.encode("utf-16-le")) // 2
    assert units == col
    return prefix + len(lines[row])


def span(start=0, end=3):
    return {"start": {"line": 0, "character": start},
            "end": {"line": 0, "character": end}}


def edit(text="good"):
    return {"range": span(), "newText": text}


def diagnostics(uri, version):
    items = [{"range": span(), "severity": 1, "message": "bad value"}] if documents[uri]["text"].startswith("bad") else []
    params = {"uri": uri, "version": version, "diagnostics": items}
    if args.scenario == "unversioned":
        del params["version"]
    send({"method": "textDocument/publishDiagnostics", "params": params})
    if version > 1:
        later({"method": "textDocument/publishDiagnostics", "params": {
            "uri": uri, "version": version - 1,
            "diagnostics": [{"range": span(), "message": "OBSOLETE"}]}})


while True:
    try:
        message = read()
    except EOFError:
        break
    record(message)
    method = message.get("method")
    p = message.get("params", {})
    ident = message.get("id")
    if method == "initialize":
        root = p["rootUri"]
        if args.scenario == "initialize-timeout":
            continue
        if args.scenario == "malformed":
            sys.stdout.buffer.write(b"Content-Length: 999999999\r\n\r\n")
            sys.stdout.buffer.flush()
            continue
        caps = {"textDocumentSync": {"openClose": True, "change": 1 if args.scenario == "full" else 2,
                                    "save": {"includeText": True}},
                "hoverProvider": True, "definitionProvider": True, "referencesProvider": True,
                "completionProvider": {"resolveProvider": True},
                "signatureHelpProvider": {}, "renameProvider": True,
                "documentFormattingProvider": True, "documentSymbolProvider": True,
                "codeActionProvider": {"resolveProvider": True},
                "executeCommandProvider": {"commands": ["fake.fix"]}}
        if args.scenario == "unsupported":
            caps = {"textDocumentSync": 2}
        if args.scenario == "pull":
            caps["diagnosticProvider"] = {"interFileDependencies": False, "workspaceDiagnostics": False}
        if args.scenario == "encoding":
            caps["positionEncoding"] = "utf-8"
        send({"id": "config", "method": "workspace/configuration", "params": {"items": [{"section": "fake"}]}})
        send({"method": "$/progress", "params": {"token": 1, "value": {"kind": "begin", "title": "fake import"}}})
        send({"id": ident, "result": {"capabilities": caps, "serverInfo": {"name": "TEST DOUBLE"}}})
    elif method == "textDocument/didOpen":
        doc = p["textDocument"]
        documents[doc["uri"]] = {"text": doc["text"], "version": doc["version"]}
        if args.scenario != "pull":
            diagnostics(doc["uri"], doc["version"])
        send({"method": "$/progress", "params": {"token": 1, "value": {"kind": "end"}}})
        send({"method": "window/logMessage", "params": {"type": 3, "message": "fake import ready"}})
        if args.scenario == "unsolicited":
            send({"id": 999, "method": "workspace/applyEdit", "params": {
                "edit": {"changes": {doc["uri"]: [edit("UNSOLICITED")]}}}})
    elif method == "textDocument/didChange":
        doc = p["textDocument"]
        old = documents[doc["uri"]]
        assert doc["version"] > old["version"]
        for change in p["contentChanges"]:
            if "range" in change:
                a = offset(old["text"], change["range"]["start"])
                b = offset(old["text"], change["range"]["end"])
                old["text"] = old["text"][:a] + change["text"] + old["text"][b:]
            else:
                old["text"] = change["text"]
        old["version"] = doc["version"]
        record({"event": "content", "uri": doc["uri"], **old})
        if args.scenario != "pull":
            diagnostics(doc["uri"], doc["version"])
    elif method == "textDocument/didClose":
        documents.pop(p["textDocument"]["uri"], None)
    elif method == "textDocument/hover":
        if args.scenario == "crash" and count == 1:
            os._exit(7)
        if args.scenario == "timeout":
            continue
        result = {"contents": {"kind": "markdown", "value": "Documentation 界\nsecond line"}}
        if args.scenario == "slow":
            later({"id": ident, "result": result})
        else:
            send({"id": ident, "result": result})
    elif method == "textDocument/diagnostic":
        send({"id": ident, "result": {"kind": "full", "items": [
            {"range": span(), "message": "pull diagnostic", "severity": 2}
        ] if documents[p["textDocument"]["uri"]]["text"].startswith("bad") else []}})
    elif method == "textDocument/completion":
        send({"id": ident, "result": {"isIncomplete": False, "items": [
            {"label": "good", "detail": "completion detail", "insertText": "good", "data": 42}
        ]}})
    elif method == "completionItem/resolve":
        send({"id": ident, "result": {**p, "documentation": "Resolved completion documentation"}})
    elif method == "textDocument/signatureHelp":
        send({"id": ident, "result": {"activeSignature": 0, "activeParameter": 1,
              "signatures": [{"label": "greet(name, count)", "documentation": "Signature documentation"}]}})
    elif method in ("textDocument/definition", "textDocument/references"):
        send({"id": ident, "result": [{"targetUri": root + "/Other.kt", "targetSelectionRange": span(0, 1),
                                      "targetRange": span()}]})
    elif method == "textDocument/documentSymbol":
        send({"id": ident, "result": [{"name": "outer", "kind": 5, "range": span(), "selectionRange": span(0, 1),
              "children": [{"name": "inner", "kind": 6, "range": span(), "selectionRange": span(1, 2)}]}]})
    elif method == "textDocument/rename":
        send({"id": ident, "result": {"documentChanges": [
            {"textDocument": {"uri": uri, "version": doc["version"]}, "edits": [edit(p["newName"])]}
            for uri, doc in documents.items()
        ]}})
    elif method == "textDocument/formatting":
        send({"id": ident, "result": [edit()]})
    elif method == "textDocument/codeAction":
        uri = p["textDocument"]["uri"]
        send({"id": ident, "result": [
            {"title": "Quick fix", "kind": "quickfix", "edit": {"changes": {uri: [edit()]}}},
            {"title": "Command fix", "command": {"command": "fake.fix", "arguments": [uri]}},
            {"title": "Resolve fix", "data": {"uri": uri}}
        ]})
    elif method == "codeAction/resolve":
        send({"id": ident, "result": {**p, "edit": {"changes": {p["data"]["uri"]: [edit()]}}}})
    elif method == "workspace/executeCommand":
        executing = ident
        send({"id": 777, "method": "workspace/applyEdit",
              "params": {"edit": {"changes": {p["arguments"][0]: [edit()]}}}})
    elif method is None and ident == 777:
        send({"id": executing, "result": None})
    elif method == "shutdown":
        send({"id": ident, "result": None})
    elif method == "exit":
        break
