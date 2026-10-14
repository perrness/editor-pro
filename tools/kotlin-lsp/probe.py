#!/usr/bin/env python3
"""Isolated stdio feasibility experiment, never imported by the Rust editor.

Runs against a disposable copy of the Maven fixture. A result of 'passed' means
observed behavior, not just advertised capabilities. Exit 2 means unavailable.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shlex
import shutil
import signal
import subprocess
import tempfile
import threading
import time


def read_message(stream):
    headers = {}
    header_bytes = 0
    while True:
        line = stream.readline(8193)
        if not line:
            raise EOFError("Language server closed stdout")
        header_bytes += len(line)
        if header_bytes > 8192:
            raise ValueError("LSP header exceeds 8 KiB")
        if line == b"\r\n":
            break
        name, separator, value = line.partition(b":")
        if not separator or not line.endswith(b"\r\n"):
            raise ValueError(f"Non-LSP data on stdout: {line[:100]!r}")
        headers[name.strip().lower()] = value.strip()
    size = int(headers[b"content-length"])
    if not 0 <= size <= 16 * 1024 * 1024:
        raise ValueError("Invalid LSP Content-Length")
    data = stream.read(size)
    if len(data) != size:
        raise EOFError("Truncated LSP body")
    return json.loads(data)


def encode_message(message):
    data = json.dumps(message, ensure_ascii=False).encode("utf-8")
    return f"Content-Length: {len(data)}\r\n\r\n".encode("ascii") + data


class Client:
    def __init__(self, command, root, stderr):
        self.root = root
        self.process = subprocess.Popen(
            command, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=stderr, start_new_session=True,
        )
        self.messages = queue.Queue()
        self.sequence = 0
        self.diagnostics = []
        self.notifications = []
        self.server_requests = []
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        try:
            while True:
                self.messages.put(read_message(self.process.stdout))
        except (EOFError, ValueError, KeyError, OSError) as error:
            self.messages.put(error)

    def send(self, message):
        self.process.stdin.write(encode_message({"jsonrpc": "2.0", **message}))
        self.process.stdin.flush()

    def notify(self, method, params=None):
        self.send({"method": method, "params": params})

    def receive(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Timed out waiting for the language server")
        try:
            message = self.messages.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError("Timed out waiting for the language server") from error
        if isinstance(message, Exception):
            raise message
        if "method" in message and "id" in message:
            self._answer(message)
        elif "method" in message:
            self.notifications.append(message["method"])
            if message["method"] == "textDocument/publishDiagnostics":
                self.diagnostics.append(message.get("params", {}))
        return message

    def _answer(self, message):
        method = message["method"]
        self.server_requests.append(method)
        if method == "workspace/configuration":
            result = [None for _ in message["params"]["items"]]
        elif method == "workspace/workspaceFolders":
            result = [{"uri": self.root.as_uri(), "name": self.root.name}]
        elif method == "workspace/applyEdit":
            result = {"applied": False, "failureReason": "Read-only feasibility client"}
        elif method in {
            "window/workDoneProgress/create", "client/registerCapability",
            "client/unregisterCapability", "window/showMessageRequest",
        }:
            result = None
        else:
            self.send({"id": message["id"], "error": {
                "code": -32601, "message": f"Probe does not implement {method}",
            }})
            return
        self.send({"id": message["id"], "result": result})

    def request(self, method, params, deadline):
        self.sequence += 1
        request_id = self.sequence
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self.receive(deadline)
            if message.get("id") == request_id and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result")

    def wait_diagnostics(self, uri, line, has_error, since, deadline, min_version=1):
        while True:
            for report in self.diagnostics[since:]:
                if report.get("uri") != uri:
                    continue
                version = report.get("version")
                if version is not None and version < min_version:
                    continue
                errors = [d for d in report.get("diagnostics", [])
                          if d.get("severity") == 1
                          and d["range"]["start"]["line"] <= line
                          <= d["range"]["end"]["line"]]
                if bool(errors) == has_error:
                    return report
            self.receive(deadline)

    def close(self):
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        if self.process.poll() is None:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        self.process.stdin.close()
        self.reader.join(timeout=2)
        self.process.stdout.close()


def position(text, marker, token, offset=0):
    for line_number, line in enumerate(text.splitlines()):
        if marker in line:
            prefix = line[:line.index(token) + offset]
            return {"line": line_number, "character": len(prefix.encode("utf-16-le")) // 2}
    raise ValueError(f"Fixture is missing {marker}")


def probe(client, root, timeout, report):
    started = time.monotonic()
    deadline = started + timeout
    file = root / "src/main/kotlin/Main.kt"
    text = file.read_text(encoding="utf-8")
    uri = file.as_uri()
    initialize = client.request("initialize", {
        "processId": os.getpid(),
        "clientInfo": {"name": "editor-pro-feasibility", "version": "0.1"},
        "rootUri": root.as_uri(),
        "workspaceFolders": [{"uri": root.as_uri(), "name": "kotlin-lsp-probe"}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "workspace": {"configuration": True, "workspaceFolders": True},
            "window": {"workDoneProgress": True},
            "textDocument": {
                "synchronization": {"didSave": False},
                "completion": {"completionItem": {"snippetSupport": False}},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "publishDiagnostics": {"versionSupport": True},
            },
        },
    }, deadline)
    report["initialize"] = initialize
    capabilities = initialize["capabilities"]
    if capabilities.get("positionEncoding", "utf-16") != "utf-16":
        raise RuntimeError("Server did not negotiate the advertised UTF-16 encoding")
    client.notify("initialized", {})
    client.notify("textDocument/didOpen", {"textDocument": {
        "uri": uri, "languageId": "kotlin", "version": 1, "text": text,
    }})
    diagnostic_line = position(text, "PROBE_DIAGNOSTIC", "broken")["line"]
    # Observed diagnostics are the readiness barrier, not a fixed indexing sleep.
    report["initial_diagnostics"] = client.wait_diagnostics(
        uri, diagnostic_line, True, 0, deadline,
    )
    symbol = position(text, "PROBE_DEFINITION", "greet", 1)
    params = {"textDocument": {"uri": uri}, "position": symbol}
    report["definition"] = client.request("textDocument/definition", params, deadline)
    definitions = report["definition"]
    definitions = definitions if isinstance(definitions, list) else [definitions]
    if not any(isinstance(item, dict) and
               item.get("uri", item.get("targetUri", "")).endswith("/Greeter.kt")
               for item in definitions):
        raise RuntimeError("Definition did not resolve to Greeter.kt")
    report["hover"] = client.request("textDocument/hover", params, deadline)
    if not report["hover"] or not report["hover"].get("contents"):
        raise RuntimeError("Hover returned no documentation")
    report["completion"] = client.request("textDocument/completion", {
        "textDocument": {"uri": uri},
        "position": position(text, "PROBE_COMPLETION", "message.", len("message.")),
        "context": {"triggerKind": 1},
    }, deadline)
    completion = report["completion"]
    items = completion.get("items", []) if isinstance(completion, dict) else completion or []
    if not any("length" in item.get("label", "") for item in items):
        raise RuntimeError("Completion did not include String.length")
    fixed = text.replace('"wrong"', "1")
    sync = capabilities.get("textDocumentSync", 0)
    sync = sync.get("change", 0) if isinstance(sync, dict) else sync
    change = {"text": fixed}
    if sync == 2:
        change["range"] = {"start": {"line": 0, "character": 0}, "end": {
            "line": text.count("\n"),
            "character": len(text.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2,
        }}
    elif sync != 1:
        raise RuntimeError(f"Unsupported document synchronization kind: {sync}")
    since = len(client.diagnostics)
    client.notify("textDocument/didChange", {
        "textDocument": {"uri": uri, "version": 2}, "contentChanges": [change],
    })
    report["updated_diagnostics"] = client.wait_diagnostics(
        uri, diagnostic_line, False, since, deadline, min_version=2,
    )
    report["diagnostic_version_verified"] = report["updated_diagnostics"].get("version") == 2
    client.notify("textDocument/didClose", {"textDocument": {"uri": uri}})
    client.request("shutdown", None, min(deadline, time.monotonic() + 10))
    client.notify("exit")
    client.process.stdin.close()
    return_code = client.process.wait(timeout=5)
    if return_code != 0:
        raise RuntimeError(f"Server exited with status {return_code}")
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", default="intellij-server --stdio",
                        help="Command and arguments, shell quoting accepted; no shell is run")
    parser.add_argument("--timeout", type=float, default=180,
                        help="Total protocol deadline including indexing, in seconds")
    parser.add_argument("--output", type=Path, default=Path(__file__).with_name("result.json"))
    parser.add_argument("--stderr", type=Path, default=Path(__file__).with_name("server.stderr.log"))
    args = parser.parse_args()
    command = shlex.split(args.server)
    report = {"status": "blocked", "command": command, "checks": [
        "initialize", "diagnostics", "cross-file definition", "hover",
        "stdlib completion", "didChange diagnostics", "shutdown/exit",
    ]}
    executable = shutil.which(command[0]) if command else None
    client = None
    if executable is None:
        report["reason"] = "Kotlin LSP executable is not installed or not on PATH"
    else:
        command[0] = str(Path(executable).resolve())
        with tempfile.TemporaryDirectory(prefix="editor-pro-kotlin-") as directory:
            root = Path(directory) / "fixture"
            fixture = Path(__file__).with_name("fixture")
            shutil.copytree(fixture, root)
            report["fixture_sha256"] = hashlib.sha256(b"".join(
                path.read_bytes() for path in sorted(fixture.rglob("*")) if path.is_file()
            )).hexdigest()
            with args.stderr.open("wb") as stderr:
                try:
                    client = Client(command, root, stderr)
                    probe(client, root, args.timeout, report)
                    report["status"] = "passed"
                except (OSError, ValueError, KeyError, EOFError, RuntimeError,
                        TimeoutError, subprocess.TimeoutExpired) as error:
                    report["status"] = "failed"
                    report["reason"] = str(error)
                finally:
                    if client:
                        report["server_requests"] = client.server_requests
                        report["notifications"] = client.notifications
                        client.close()
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": report["status"], "reason": report.get("reason"),
                      "report": str(args.output)}, indent=2))
    return {"passed": 0, "failed": 1, "blocked": 2}[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
