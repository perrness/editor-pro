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
diagnostic_pulls = 0
refresh_supported = False


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


def call_item(uri, name):
    return {"uri": uri, "name": name, "kind": 12, "range": span(0, 9),
            "selectionRange": span(0, 1), "detail": "test call item",
            "data": {"opaque": [name, uri, 42]}}


def comprehension_reply(ident, result):
    message = {"id": ident, "result": result}
    if args.scenario == "comprehension-slow":
        later(message)
    else:
        send(message)


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
        if args.scenario.startswith("completion-command"):
            assert "command" in p["capabilities"]["textDocument"]["completion"]["completionItem"]["resolveSupport"]["properties"]
        if args.scenario == "java-settings":
            assert p["initializationOptions"]["settings"]["java"]["signatureHelp"]["enabled"]
            send({"id": "java-config", "method": "workspace/configuration", "params": {"items": [
                {"section": "java"}, {"section": "java.signatureHelp.enabled"}, {"section": "unknown"}
            ]}})
        refresh_supported = p["capabilities"]["workspace"].get("diagnostics", {}).get("refreshSupport", False)
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
                "workspaceSymbolProvider": {"resolveProvider": True}, "callHierarchyProvider": True,
                "codeActionProvider": {"resolveProvider": True},
                "executeCommandProvider": {"commands": ["fake.fix"]}}
        if args.scenario == "unsupported":
            caps = {"textDocumentSync": 2}
        if args.scenario.startswith("pull"):
            caps["diagnosticProvider"] = {"interFileDependencies": args.scenario == "pull-dependencies", "workspaceDiagnostics": False}
        if args.scenario == "encoding":
            caps["positionEncoding"] = "utf-8"
        send({"id": "config", "method": "workspace/configuration", "params": {"items": [{"section": "fake"}]}})
        send({"method": "$/progress", "params": {"token": 1, "value": {"kind": "begin", "title": "fake import"}}})
        send({"id": ident, "result": {"capabilities": caps, "serverInfo": {"name": "TEST DOUBLE"}}})
    elif method == "textDocument/didOpen":
        doc = p["textDocument"]
        documents[doc["uri"]] = {"text": doc["text"], "version": doc["version"]}
        if not args.scenario.startswith("pull"):
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
        if not args.scenario.startswith("pull"):
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
        diagnostic_pulls += 1
        if args.scenario == "pull-refresh-race" and diagnostic_pulls == 1:
            send({"id": "diagnostic-refresh", "method": "workspace/diagnostic/refresh"})
            later({"id": ident, "result": {"kind": "full", "items": [
                {"range": span(), "message": "OBSOLETE before refresh", "severity": 1}
            ]}})
            continue
        if args.scenario == "pull-refresh" and diagnostic_pulls == 1:
            send({"id": ident, "result": {"kind": "full", "items": []}})
            if refresh_supported:
                later({"id": "diagnostic-refresh", "method": "workspace/diagnostic/refresh"})
            continue
        dependent_fixed = args.scenario == "pull-dependencies" and any(d["text"].startswith("fixed") for d in documents.values())
        report = {"id": ident, "result": {"kind": "full", "items": [
            {"range": span(), "message": "pull diagnostic", "severity": 2}
        ] if documents[p["textDocument"]["uri"]]["text"].startswith("bad") and not dependent_fixed else []}}
        if dependent_fixed:
            later(report)
        else:
            send(report)
    elif method == "textDocument/completion":
        item = {"label": "good", "detail": "completion detail", "insertText": "good", "data": 42}
        if args.scenario.startswith("completion-command"):
            item["textEdit"] = {"range": span(0, 0), "newText": ""}
            item["data"] = {"title": "Apply completion", "command": "fake.fix" if args.scenario == "completion-command" else "fake.unadvertised", "arguments": [p["textDocument"]["uri"]]}
        send({"id": ident, "result": {"isIncomplete": False, "items": [
            item
        ]}})
    elif method == "completionItem/resolve":
        item = {**p, "documentation": "Resolved completion documentation"}
        if args.scenario.startswith("completion-command"):
            item["command"] = p["data"]
        send({"id": ident, "result": item})
    elif method == "textDocument/signatureHelp":
        send({"id": ident, "result": {"activeSignature": 0, "activeParameter": 1,
              "signatures": [{"label": "greet(name, count)", "documentation": "Signature documentation"}]}})
    elif method in ("textDocument/definition", "textDocument/references"):
        send({"id": ident, "result": [{"targetUri": root + "/Other.kt", "targetSelectionRange": span(0, 1),
                                      "targetRange": span()}]})
    elif method == "textDocument/documentSymbol":
        send({"id": ident, "result": [{"name": "outer", "kind": 5, "range": span(), "selectionRange": span(0, 1),
              "children": [{"name": "inner", "kind": 6, "range": span(), "selectionRange": span(1, 2)}]}]})
    elif method == "workspace/symbol":
        if p["query"] == "empty":
            comprehension_reply(ident, None)
            continue
        item = {"name": p["query"], "kind": 12, "containerName": "Project",
                "location": {"uri": root + "/Other.kt", "range": span(4, 5)}, "data": [42, "keep"]}
        if p["query"] == "lazy":
            del item["location"]["range"]
        if p["query"] == "virtual":
            item["location"]["uri"] = "jdt://decompiled/Library"
        if p["query"] == "malformed":
            item["location"]["range"] = span(9, 2)
        if p["query"] == "surrogate":
            item["location"]["range"] = {"start": {"line": 1, "character": 1}, "end": {"line": 1, "character": 2}}
        comprehension_reply(ident, [item] * (4100 if p["query"] == "many" else 1))
    elif method == "workspaceSymbol/resolve":
        assert p["data"] == [42, "keep"]
        comprehension_reply(ident, {**p, "location": {**p["location"], "range": span(4, 5)}})
    elif method == "textDocument/prepareCallHierarchy":
        comprehension_reply(ident, [call_item(p["textDocument"]["uri"], "start")])
    elif method in ("callHierarchy/incomingCalls", "callHierarchy/outgoingCalls"):
        item = p["item"]
        assert item["data"] == {"opaque": [item["name"], item["uri"], 42]}
        uri = root + ("/Main.kt" if item["uri"].endswith("/Other.kt") else "/Other.kt")
        incoming = method == "callHierarchy/incomingCalls"
        other = call_item(uri, "caller" if incoming else "callee")
        result = [{"from" if incoming else "to": other, "fromRanges": [span(4, 5), span(6, 7)]}]
        comprehension_reply(ident, None if args.scenario == "calls-empty" else result)
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
        if args.scenario == "exit-eof":
            assert sys.stdin.buffer.read() == b""
            record({"event": "stdin-closed"})
        break
