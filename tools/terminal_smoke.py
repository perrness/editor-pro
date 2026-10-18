#!/usr/bin/env python3
"""Exercise the real binary via a PTY on Linux/macOS. No third-party modules."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def run_session(binary, path, commands, expected, environment=None, arguments=None, cwd=None):
    session_settings = tempfile.TemporaryDirectory(prefix="editor-pro-smoke-settings-")
    settings_root = Path(session_settings.name)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    pid = os.fork()
    if pid == 0:
        os.close(master)
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for descriptor in (0, 1, 2):
            os.dup2(slave, descriptor)
        if slave > 2:
            os.close(slave)
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLORTERM"] = "truecolor"
        os.environ.pop("NO_COLOR", None)
        # User settings must not affect acceptance fixtures.
        os.environ["HOME"] = str(settings_root / "home")
        os.environ["XDG_CONFIG_HOME"] = str(settings_root / "config")
        os.environ["XDG_DATA_HOME"] = str(settings_root / "data")
        for name in ("EDITOR_PRO_KOTLIN_LSP", "EDITOR_PRO_JAVA_LSP", "EDITOR_PRO_CSHARP_LSP"):
            os.environ.pop(name, None)
        settings = (Path(os.environ["HOME"]) / "Library/Application Support/editor-pro/config.json"
                    if sys.platform == "darwin" else Path(os.environ["XDG_CONFIG_HOME"]) / "editor-pro/config.json")
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text('{"lsp":{"auto_install":false}}\n')
        if environment:
            os.environ.update(environment)
        if cwd is not None:
            os.chdir(cwd)
        argv = [str(path)] if arguments is None else [str(arg) for arg in arguments]
        # Keep the session leader alive while checking restoration. macOS
        # revokes the slave terminal when its controlling session leader exits.
        termios.tcflush(0, termios.TCIFLUSH)
        original = termios.tcgetattr(0)
        editor_pid = os.fork()
        if editor_pid:
            _, status = os.waitpid(editor_pid, 0)
            # Canonical mode sets macOS PENDIN, a transient input-queue flag.
            # Clear pending input without changing the terminal configuration.
            termios.tcflush(0, termios.TCIFLUSH)
            restored = termios.tcgetattr(0)
            if restored != original:
                os.write(2, f"Terminal settings were not restored: expected {original!r}, got {restored!r}\n".encode())
                os._exit(1)
            code = os.waitstatus_to_exitcode(status)
            os._exit(code if code >= 0 else 128 - code)
        os.execv(str(binary), [str(binary), *argv])
    output = bytearray()
    reaped = False
    cursor_queries = 0

    def pump(duration):
        nonlocal cursor_queries
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            readable, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
            if readable:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        return
                    raise
                if not data:
                    return
                output.extend(data)
                # A PTY has no terminal emulator. Ratatui asks for the initial
                # cursor position, so implement the terminal's DSR response.
                queries = output.count(b"\x1b[6n")
                if queries > cursor_queries:
                    os.write(master, b"\x1b[1;1R" * (queries - cursor_queries))
                    cursor_queries = queries

    try:
        startup = time.monotonic() + 10
        while b"NORMAL" not in output and time.monotonic() < startup:
            pump(0.05)
        assert b"NORMAL" in output, f"Editor never rendered: {output!r}"
        command_output_start = len(output)
        for index, command in enumerate(commands):
            if not isinstance(command, tuple):
                command_output_start = len(output)
            if isinstance(command, tuple):
                # A three-item tuple retries an idempotent command until an
                # observable screen/durability signal, never a presumed delay.
                if len(command) == 3:
                    payload, marker, timeout = command
                    command_output_start = len(output)
                else:
                    marker, timeout = command
                    payload = None
                def complete():
                    return marker() if callable(marker) else marker in output[command_output_start:]
                deadline = time.monotonic() + timeout
                while not complete() and time.monotonic() < deadline:
                    if payload is not None:
                        if callable(payload):
                            payload(master, slave, pid)
                        else:
                            os.write(master, payload)
                    pump(0.05)
                assert complete(), f"Missing PTY completion {marker!r}: {output!r}"
            elif callable(command):
                command(master, slave, pid)
            else:
                os.write(master, command)
            # Separate Escape from subsequent bytes (terminal Escape ambiguity).
            pump(0.15)
            if index < len(commands) - 1:
                exited, _ = os.waitpid(pid, os.WNOHANG)
                if exited:
                    reaped = True
                assert not exited, f"Editor exited before the final command: {output!r}"
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            pump(0.05)
            exited, status = os.waitpid(pid, os.WNOHANG)
            if exited:
                reaped = True
                code = os.waitstatus_to_exitcode(status)
                assert code == 0, f"Editor exited with status {code}: {output.decode(errors='replace')}"
                break
        assert reaped, f"Editor did not exit: {output[-4000:]!r}"
        assert b"\x1b[?1049l" in output, "Alternate screen was not left"
        assert b"\x1b[?2004l" in output, "Bracketed paste was not disabled"
        assert path.read_bytes() == expected, path.read_bytes()
        return bytes(output)
    finally:
        if not reaped:
            os.killpg(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(master)
        os.close(slave)
        session_settings.cleanup()


def run_binary_snapshot_session(binary, directory, resize, capture_dir=None):
    repo = Path(directory) / "binary-snapshot-project"
    repo.mkdir()

    def git(*args):
        return subprocess.check_output(["git", "--no-optional-locks", *args], cwd=repo)

    git("init", "-b", "main")
    git("config", "user.name", "PTY Snapshot Test")
    git("config", "user.email", "snapshot@example.invalid")
    git("config", "commit.gpgsign", "false")
    git("config", "core.autocrlf", "false")
    source = repo / "source.txt"
    source.write_bytes(b"base revision\n")
    asset = repo / "asset.png"
    original_asset = b"\x89PNG\r\n\x1a\n\0original"
    current_asset = b"\x89PNG\r\n\x1a\n\0revision"
    asset.write_bytes(original_asset)
    git("add", "--all")
    git("commit", "-m", "mixed snapshot base")
    source.write_bytes(b"next revision\n")
    asset.write_bytes(current_asset)
    encoded = repo / "z-encoded"
    encoded.write_bytes(b"\xff\xfe\x80")
    index = (repo / ".git/index").read_bytes()
    head = git("rev-parse", "HEAD")
    store = repo / ".git/editor-pro-review.json"

    def captured():
        return store.exists() and len(json.loads(store.read_text()).get("history", {}).get("checkpoints", [])) == 1

    output = run_session(binary, source, [
        (b":review snapshots\r", b"No snapshots", 10),
        b":review snapshot\r", (captured, 30),
        (b":review full\r", b"review full HEAD", 10),
        resize(24, 130), (b"base revision", 10),
        b":changes\r", resize(24, 129), (b"asset.png", 10),
        (b"d", b"binary or non-UTF-8 content", 10),
        b"\x1b", b":review full\r", resize(24, 130), (b"next revision", 10),
        b":review since\r", resize(24, 129), (b"No textual changes", 10),
        b"\x1b", b":q\r",
    ], b"next revision\n")
    history = json.loads(store.read_text())["history"]
    baseline = {bytes(f["path"]).decode(): f for f in history["baseline"]["files"]}
    checkpoint = {bytes(f["path"]).decode(): f for f in history["checkpoints"][0]["files"]}
    assert bytes.fromhex(baseline["asset.png"]["binary"]) == original_asset
    assert bytes.fromhex(checkpoint["asset.png"]["binary"]) == current_asset
    assert bytes.fromhex(checkpoint["z-encoded"]["binary"]) == encoded.read_bytes()
    assert baseline["source.txt"]["text"] == "base revision\n"
    assert checkpoint["source.txt"]["text"] == "next revision\n"
    assert asset.read_bytes() == current_asset
    assert (repo / ".git/index").read_bytes() == index
    assert git("rev-parse", "HEAD") == head
    if capture_dir:
        (Path(capture_dir) / "binary-review-snapshot.ansi").write_bytes(output)


def run_splash_sessions(binary, directory, resize, capture_dir=None):
    root = Path(directory) / "splash-project"
    root.mkdir()
    source = root / "existing.txt"
    original = "existing 界\r\n".encode()
    source.write_bytes(original)
    banner = b"Your terminal. Your flow."

    def capture(name, output):
        assert banner in output, "Pathless startup did not show the splash"
        if capture_dir:
            (Path(capture_dir) / f"splash-{name}.ansi").write_bytes(output)

    destination = root / "new.txt"
    output = run_session(binary, destination, [
        resize(10, 24), (b"Esc to begin", 10), resize(1, 1),
        b"\x1b[I", resize(24, 80), (banner, 10),
        # The very first key enters Insert; the artwork never enters the buffer.
        b"ifirst ", b"\x1b", b"u", b"\x12", b"A",
        "\x1b[200~界é\r\nnext\x1b[201~".encode(), b"\x1b", b":wq new.txt\r",
    ], "first 界é\nnext".encode(), arguments=[], cwd=root)
    assert b"\x1b[?25l" in output.split(b"first", 1)[0], "Splash cursor was not hidden"
    capture("editing-resize", output)

    output = run_session(binary, source, [
        b"\x1bOP", (b"Help | All", 10), b"\x1b",
        b":enew\r", resize(24, 81), b":q\r",
    ], original, arguments=[], cwd=root)
    assert output.count(banner) == 1, "Splash reappeared after help or :enew"
    capture("help", output)

    output = run_session(binary, source, [
        b"\x10", b"existing", (b"\r", b"existing", 10), b":q\r",
    ], original, arguments=[], cwd=root)
    capture("picker", output)

    # A paste in Normal is rejected as usual, including command-looking text.
    output = run_session(binary, source, [
        b"\x1b[200~:q!\r\x1b[201~", resize(24, 81),
        (b"Enter Insert mode before pasting", 10),
        b":q\r",
    ], original, arguments=[], cwd=root)
    capture("paste", output)

    output = run_session(binary, source, [b":q\r"], original, arguments=[], cwd=root)
    capture("quit", output)

    # Existing files, new paths and directories should still open immediately.
    for arguments in [[source], [root / "not-created.txt"], [root], ["--", source]]:
        output = run_session(binary, source, [b"\x1b", b":q\r"], original,
                             arguments=arguments, cwd=root)
        assert banner not in output, f"Unexpected splash for {arguments!r}"
    assert not (root / "not-created.txt").exists()


def run_ai_explain_sessions(binary, directory, resize, capture_dir=None):
    root = Path(directory) / "explain project"
    root.mkdir()
    source = root / "source.txt"
    original = "first\r\nold 界👩‍💻\r\nlast\r\n".encode()
    source.write_bytes(original)
    (root / "Cargo.toml").write_text("[package]\n")
    environment = {"EDITOR_PRO_CODEX": str(Path("tools/ai/fake_codex.py").resolve()),
                   "EDITOR_PRO_AI_MODEL": "normal"}
    output = run_session(binary, source, [
        b"jV\x07", resize(28, 160), (b"Question: Explain what this code does", 10),
        (b"Repository / project:", 10), (b":2:1", 10),
        resize(2, 1), resize(28, 160), b"\x1b",
        # Insertion at the retained cursor proves that dismissing the preview
        # did not move the source. Undo must leave the exact CRLF/Unicode bytes.
        b"ihere ", b"\x1b", resize(28, 161), (b"here old", 10), b"u",
        b"V\x07", b":ai ask What is known about this code?\r", b":ai send\r",
        (b"Completed", 10), resize(28, 162), (b"Inferred intent / uncertainty:", 10),
        b"\x1b", b":q!\r",
    ], original, environment)
    if capture_dir:
        (Path(capture_dir) / "ai-explain-selection.ansi").write_bytes(output)

    for args in (["init", "-b", "main"], ["add", "."],
                 ["-c", "user.name=AI Fixture", "-c", "user.email=ai@example.invalid",
                  "-c", "commit.gpgsign=false", "commit", "-m", "base"]):
        subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)
    updated = original.replace(b"old", b"changed")
    source.write_bytes(updated)
    index = (root / ".git/index").read_bytes()
    output = run_session(binary, source, [
        b"j", b":diff\r", resize(32, 160), (resize(32, 161), b"@@", 10),
        # Loading review metadata invalidates the first Git snapshot. Retry
        # this read-only shortcut until a fresh comparison opens the preview;
        # Ctrl-g is inert inside AI and cannot send the request.
        (b"\x07", b"Question: Explain the behavior changed", 10), resize(32, 160),
        (b"Base commit:", 10), (b"Hunk source (matches captured new side)", 10),
        b":ai send\r", (b"Completed", 10),
        b"\x1b", resize(32, 161), (b"@@", 10), b"\x1b",
        b"ihere ", b"\x1b", resize(32, 162), (b"here changed", 10), b"u", b":q!\r",
    ], updated, environment)
    assert (root / ".git/index").read_bytes() == index
    if capture_dir:
        (Path(capture_dir) / "ai-explain-hunk.ansi").write_bytes(output)


def run_disk_refresh_sessions(binary, directory, resize, capture_dir=None):
    source = Path(directory) / "disk-refresh.txt"
    original = "old 界\r\n".encode()
    first = "agent é\r\n".encode()
    final = "latest 👩‍💻\r\n".encode()
    source.write_bytes(original)
    columns = 100
    def repaint(master, slave, pid):
        nonlocal columns
        # Incremental ANSI writes can split even a completed message. Repaint
        # while waiting, without sending any additional editing/approval keys.
        columns = 101 if columns == 100 else 100
        resize(24, columns)(master, slave, pid)
    def replace(master, slave, pid):
        temporary = source.with_suffix(".temporary")
        temporary.write_bytes(first)
        temporary.replace(source)
    output = run_session(binary, source, [
        b"ilocal ", b"\x1b", replace,
        (b"!DISK", 10), b":refresh\r", (repaint, b"Comparison ready", 10),
        (repaint, b"UNSAVED conflict", 10), (repaint, b"- local old", 10),
        b"r", (repaint, b"Discard unsaved", 10), b"n", b"\x1b",
        b":w\r", (repaint, b"File changed on disk", 10),
        b":refresh\r", (repaint, b"Comparison ready", 10),
        lambda master, slave, pid: source.write_bytes(final),
        b"r", b"y", (repaint, b"Disk changed", 10),
        b"c", (repaint, b"Comparison ready", 10), resize(5, 20), resize(1, 1),
        resize(24, 100), b"r", b"y", (repaint, b"Reloaded from disk", 10),
        b":q\r",
    ], final)
    if capture_dir:
        (Path(capture_dir) / "disk-refresh-conflict.ansi").write_bytes(output)

    recovered = Path(directory) / "disk-recovered.txt"
    output = run_session(binary, recovered, [
        lambda master, slave, pid: source.unlink(),
        b"\x1b[I", (b"!DISK", 10), b":refresh\r", (repaint, b"File deleted or renamed", 10),
        b"r", b"y", b"\x1b", f":w {recovered}\r".encode(),
        (lambda: recovered.exists() and recovered.read_bytes() == final, 10), b":q\r",
    ], final, arguments=[source])
    assert not source.exists()
    if capture_dir:
        (Path(capture_dir) / "disk-refresh-deletion.ansi").write_bytes(output)


def run_help_session(binary, directory, resize, capture_dir=None):
    source = Path(directory) / "help-source.txt"
    original = "é界\r\n" + "".join(f"source line {line}\r\n" for line in range(2, 41))
    source.write_bytes(original.encode())
    # Open help during Insert, search the three discovery workflows, resize,
    # then continue the same edit at the original cursor with one undo group.
    edited = original.replace("source line 30\r\n", "source line 30beforeafter\r\n")
    output = run_session(binary, source, [
        b"30GAbefore", b"\x1bOP", (b"Help | All", 10),
        b"definition", resize(24, 81), (b"gd", 10),
        b"\x15git diff", resize(24, 80), (b"Space g d", 10),
        b"\x15", b"\x1b[Z", (b"AI / Codex", 10),
        b"ask", resize(24, 81), (b":ai ask QUESTION", 10),
        resize(7, 20), b"\x1b[6~", b"\x1b[5~", resize(1, 1),
        resize(24, 80), b"\x1b", (b"INSERT", 10), b"after", b"\x1b",
        b":w\r", (lambda: source.read_bytes() == edited.encode(), 10),
        b"u", b":w\r", (lambda: source.read_bytes() == original.encode(), 10),
        b":help Codex\r", resize(24, 81), (b":ai context file", 10),
        b"\x1b", b":q\r",
    ], original.encode())
    if capture_dir:
        (Path(capture_dir) / "keyboard-help.ansi").write_bytes(output)


def run_completion_acceptance_session(binary, directory, resize, capture_dir=None):
    project = Path(directory) / "completion-acceptance"
    project.mkdir()
    (project / "pom.xml").write_text("<project/>")
    source = project / "Main.kt"
    original = "😀 value.\r\nnext\r\n".encode()
    partial = "😀 value.le\r\nnext\r\n".encode()
    source.write_bytes(original)
    log = project / "protocol.jsonl"
    fake = Path(__file__).parent / "lsp" / "fake_server.py"
    environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
        sys.executable, str(fake.resolve()), "--log", str(log),
        "--scenario", "auto-completion",
    ])}
    def ready():
        return log.exists() and '"method": "textDocument/didOpen"' in log.read_text()
    def saved(text):
        return lambda: source.read_bytes() == text
    output = run_session(binary, source, [
        (ready, 10), b"Ale", (b"Tab/Enter accept", 10),
        b"\x1b[B", b"\x1b[A", b"\x1b[B", b"\r", (b"inserted", 10),
        b"\x1b", b":w\r", (saved("😀 value.lengthSquared \r\nnext\r\n".encode()), 10),
        b"u", b":w\r", (saved(partial), 10),
        b"A\x0e", (b"completion (", 10), b"d", (b"Resolved", 10),
        resize(3, 1), resize(24, 100), b"\t", resize(24, 101), (b"Completion inserted", 10),
        b"\x1b", b":w\r", (saved("😀 value.length \r\nnext\r\n".encode()), 10),
        b"u", b":w\r", (saved(partial), 10),
        b"A\x7fe", (b"Tab/Enter accept", 10), b"\x1b", b"\t\r",
        b"\x1b", b":w\r", (saved("😀 value.le\t\r\n\r\nnext\r\n".encode()), 10),
        b"u", b":w\r", (saved(partial), 10),
        b"A\x0e", (b"completion (", 10), b"\x1b", b"\x1b", b":q\r",
    ], partial, environment)
    assert b"Enter applies" not in output, "Plain completions opened an edit preview"
    messages = [json.loads(line) for line in log.read_text().splitlines()]
    assert not any(m.get("method") == "workspace/executeCommand" for m in messages)
    assert sum(m.get("method") == "completionItem/resolve" for m in messages) == 2
    if capture_dir:
        (Path(capture_dir) / "completion-acceptance.ansi").write_bytes(output)


def run_completion_edits_sessions(binary, directory, resize, capture_dir=None):
    for scenario, completed in [
        ("auto-completion-extra", "import sample\r\n😀 value.length \r\nnext\r\n"),
        ("auto-completion-multiline", "😀 value.length\r\nnext \r\nnext\r\n"),
        ("auto-completion-command", "😀 value.length() \r\nnext\r\n"),
    ]:
        project = Path(directory) / scenario
        project.mkdir()
        (project / "pom.xml").write_text("<project/>")
        source = project / "Main.kt"
        source.write_bytes("😀 value.\r\nnext\r\n".encode())
        partial = "😀 value.le\r\nnext\r\n".encode()
        log = project / "protocol.jsonl"
        fake = Path(__file__).parent / "lsp" / "fake_server.py"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
            sys.executable, str(fake.resolve()), "--log", str(log), "--scenario", scenario,
        ])}
        def ready():
            return log.exists() and '"method": "textDocument/didOpen"' in log.read_text()
        def saved(text):
            return lambda: source.read_bytes() == text
        output = run_session(binary, source, [
            (ready, 10), b"Ale", (b"Tab/Enter accept", 10),
            b"\r", resize(24, 101), (b"Completion inserted", 10), b"\x1b", b":w\r",
            (saved(completed.encode()), 10), b"u", b":w\r", (saved(partial), 10),
            b"A\x0e", (b"completion (", 10), resize(3, 1), resize(24, 100),
            b"\t", resize(24, 101), (b"Completion inserted", 10), b"\x1b", b":w\r",
            (saved(completed.encode()), 10), b"u", b":w\r", (saved(partial), 10), b":q\r",
        ], partial, environment)
        assert b"Enter applies" not in output, f"{scenario} opened a completion preview"
        if scenario == "auto-completion-command":
            messages = [json.loads(line) for line in log.read_text().splitlines()]
            assert sum(m.get("method") == "workspace/executeCommand" for m in messages) == 2
            assert sum(m.get("id") == 777 and m.get("result", {}).get("applied", False) for m in messages) == 2
        if capture_dir:
            (Path(capture_dir) / f"{scenario}.ansi").write_bytes(output)


def run_file_tree_session(binary, directory, resize, capture_dir=None):
    # File-tree binding, prompts, safe mutations, folding and return to source.
    tree_root = Path(directory) / "tree-project"
    tree_root.mkdir()
    tree_source = tree_root / "source.txt"
    tree_source.write_bytes("界\r\nsource".encode())
    output = run_session(binary, tree_source, [
        b" pv", (b"Tree", 10),
        b"A", b"folder", b"\r", (lambda: (tree_root / "folder").is_dir(), 10),
        b"R", resize(24, 101), (b"folder", 10),
        b"l", b"a", b"created.txt", b"\r",
        (lambda: (tree_root / "folder/created.txt").exists(), 10),
        b"R", resize(24, 100), (b"folder", 10), b"h", b"l",
        resize(24, 99), (b"created.txt", 10), b"j",
        b"r", b"renamed.txt", b"\r",
        (lambda: (tree_root / "folder/renamed.txt").exists(), 10),
        b"R", resize(24, 101), (b"folder", 10), b"h", b"l",
        resize(24, 99), (b"renamed.txt", 10), b"j",
        b"d", b"no", b"\r", (b"cancelled", 10),
        b"d", b"yes", b"\r",
        (lambda: not (tree_root / "folder/renamed.txt").exists(), 10),
        resize(3, 1), resize(24, 100),
        b" pv", (b"NORMAL", 10),
        b"\x10", (b"source.txt", 10), b"\x1b", b":q\r",
    ], tree_source.read_bytes(), cwd=tree_root)
    if capture_dir:
        (Path(capture_dir) / "file-tree.ansi").write_bytes(output)


def run_colorful_ui_sessions(binary, directory, resize, capture_dir=None):
    root = Path(directory) / "colorful-project"
    (root / "src").mkdir(parents=True)
    source = root / "src/main.rs"
    original = 'fn main() { println!("Hello, 界!"); }\r\n'.encode()
    source.write_bytes(original)
    for name in ("Cargo.toml", "README.md", "logo.svg"):
        (root / name).write_text("")
    for theme, panel in (("tokyo-night", b"48;2;22;22;30"),
                         ("catppuccin", b"48;2;24;24;37"),
                         ("gruvbox", b"48;2;29;32;33")):
        output = run_session(binary, source, [
            f":theme {theme}\r".encode(), b" pv", (b"README.md", 10),
            b"l", (b"[rs]", 10), b"j", resize(24, 100),
            (b"[rs]", 10), (b"[{}]", 10), (b"[md]", 10),
            resize(4, 20), resize(2, 2), resize(1, 1), resize(24, 101),
            (b"main.rs", 10), b"\x1b", b":ls\r", (b"Buffers", 10),
            b"\x1b", b"\x10", (b"Files", 10), b"main", b"\r",
            b"i// temporary", b"\x1b", b"u", b":q\r",
        ], original, cwd=root)
        assert panel in output, f"Missing {theme} panel color"
        assert "▎".encode() in output, "Missing selection rail"
        assert "│".encode() in output, "Missing tree guide"
        if capture_dir:
            (Path(capture_dir) / f"colorful-ui-{theme}.ansi").write_bytes(output)


def run_lsp_warmup_sessions(binary, directory, resize, capture_dir=None):
    for name, arguments in [("directory", ["."]), ("cwd", [])]:
        root = Path(directory) / f"warmup-{name}"
        root.mkdir()
        (root / ".git").mkdir()
        (root / "pom.xml").write_text("<artifactId>kotlin-maven-plugin</artifactId>")
        source = root / "Main.kt"
        source.write_bytes("bad value\r\n😀 é界\r\n".encode())
        log = root / "protocol.jsonl"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
            "python3", str(Path("tools/lsp/fake_server.py").absolute()),
            "--log", str(log), "--scenario", "initialize-held",
        ])}

        def messages():
            return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

        def release(master, slave, pid):
            assert not any(m.get("method") == "textDocument/didOpen" for m in messages())
            Path(str(log) + ".initialize").write_text("ready")

        output = run_session(binary, source, [
            (lambda: any(m.get("method") == "initialize" for m in messages()), 10),
            b"\x1b", b":LspInfo\r", (b"Prepared kotlin", 10), b"\x1b",
            b":e Main.kt\r", b"iunsaved ", b"\x1b", resize(3, 1), resize(24, 101),
            release, (lambda: any(m.get("method") == "textDocument/didOpen" for m in messages()), 10),
            b":LspInfo\r", (b"Ready", 10), b"\x1b", b":q!\r",
        ], source.read_bytes(), environment, arguments=arguments, cwd=root)
        assert sum(m.get("method") == "initialize" for m in messages()) == 1
        assert any(m.get("method") == "textDocument/didOpen" and
                   m["params"]["textDocument"]["text"].startswith("unsaved ") for m in messages())
        assert any(m.get("method") == "shutdown" for m in messages())
        if capture_dir:
            (Path(capture_dir) / f"lsp-warmup-{name}.ansi").write_bytes(output)


def run_lsp_startup_session(binary, directory, resize, capture_dir=None):
    root = Path(directory) / "lsp-startup"
    root.mkdir()
    (root / "pom.xml").write_text("<project/>")
    source = root / "Main.kt"
    original = "bad value\r\n😀 é界\r\n".encode()
    source.write_bytes(original)
    log = root / "protocol.jsonl"
    environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
        sys.executable, str(Path("tools/lsp/fake_server.py").absolute()),
        "--log", str(log), "--scenario", "lifecycle-held",
    ])}

    def messages():
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def opened(count):
        return lambda: sum(m.get("method") == "textDocument/didOpen" for m in messages()) >= count

    def notifications(*frames):
        def send(master, slave, pid):
            with Path(str(log) + ".notifications").open("a") as output:
                for frame in frames:
                    output.write(json.dumps(frame) + "\n")
        return send

    def progress(token, kind, **fields):
        return {"method": "$/progress", "params": {"token": token, "value": {"kind": kind, **fields}}}

    def release(master, slave, pid):
        Path(str(log) + ".initialize").write_text("ready")

    columns = 100
    def repaint(master, slave, pid):
        nonlocal columns
        columns = 101 if columns == 100 else 100
        resize(24, columns)(master, slave, pid)

    def doctor_ready(master, slave, pid):
        os.write(master, b"G")
        repaint(master, slave, pid)

    output = run_session(binary, source, [
        (lambda: any(m.get("method") == "initialize" for m in messages()), 10),
        b":LspInfo\r", (b"Status: Starting", 10), b"\x1b",
        b"iunsaved ", b"\x1b",
        notifications(progress(1, "begin", title="Gradle import"),
                      progress("1", "begin", title="Resolve dependencies"),
                      progress(1, "end")),
        b":LspInfo\r", (repaint, b"Status: Indexing", 10),
        resize(3, 1), resize(24, 100), release, (opened(1), 10),
        notifications({"method": "window/showMessage", "params": {
            "type": 1, "message": "Gradle import failed: project JDK 21 missing"}}, progress("1", "end")),
        (repaint, b"Status: Degraded", 10), b"\x1b",
        b":LspLog\r", (repaint, b"Gradle import failed", 10), b"\x1b",
        b":Doctor\r", (doctor_ready, b"Runtime supplied by explicit command override", 10), b"\x1b",
        b":help Doctor\r", (b":LspInfo / :LspLog / :Doctor", 10), b"\x1b",
        b":LspRestart\r", (opened(2), 10), b":LspInfo\r", (repaint, b"Status: Ready", 10),
        b"\x1b", b"u", b":q\r",
    ], original, environment)
    assert sum(m.get("method") == "initialize" for m in messages()) == 2
    assert all(m["params"]["textDocument"]["text"].startswith("unsaved ")
               for m in messages() if m.get("method") == "textDocument/didOpen")
    if capture_dir:
        (Path(capture_dir) / "lsp-startup-recovery.ansi").write_bytes(output)

    output = run_session(binary, source, [
        b":LspInfo\r", (repaint, b"Status: Failed", 10), b"\x1b",
        b":Doctor\r", (b":LspRestart", 10), b"\x1b",
        b"istill editable", b"\x1b", b"u", b":q\r",
    ], original, {"EDITOR_PRO_KOTLIN_LSP": '["/missing/editor-pro-lsp"]'})
    if capture_dir:
        (Path(capture_dir) / "lsp-startup-missing.ansi").write_bytes(output)


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/editor-pro").absolute()
    if not binary.is_file():
        print(f"BLOCKED: build the editor first; binary missing: {binary}", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="editor-pro-pty-") as directory:
        project = Path(directory) / "project with spaces"
        project.mkdir()
        project_file = project / "startup.txt"
        project_file.write_bytes("before 界\r\n".encode())
        ep = Path(directory) / "ep"
        ep.symlink_to(binary)
        output = run_session(ep, project_file, [
            # An unchanged picker result may not be redrawn after filtering;
            # wait for the opened file instead of another copy of its name.
            b"startup", (b"\r", b"before", 10), b"i// ", b"\x1b", b":wq\r",
        ], "// before 界\r\n".encode(), arguments=["."], cwd=project)
        assert b"Files" in output
        # A directory argument also sets the root for relative :e and :w paths.
        run_session(ep, project_file, [
            b"\x1b", b":e startup.txt\r", b":wq\r",
        ], "// before 界\r\n".encode(), arguments=[project])

        config_home = Path(directory) / "settings home"
        config_env = {"HOME": str(config_home), "XDG_CONFIG_HOME": str(config_home / "xdg")}
        config_path = Path(subprocess.check_output(
            [str(ep), "--config-path"], env={**os.environ, **config_env}, text=True,
        ).strip())
        config_path.parent.mkdir(parents=True)
        config_bytes = b'{ "theme": "gruvbox" }\n'
        config_path.write_bytes(config_bytes)
        output = run_session(ep, project_file, [b":q\r"], "// before 界\r\n".encode(), config_env)
        assert b"48;2;40;40;40" in output, "Configured Gruvbox background was not rendered"
        assert config_path.read_bytes() == config_bytes
        capture_dir = os.environ.get("EDITOR_PRO_SMOKE_CAPTURE_DIR")
        if capture_dir:
            Path(capture_dir).mkdir(parents=True, exist_ok=True)
            (Path(capture_dir) / "installation-config.ansi").write_bytes(output)

        path = Path(directory) / "source.rs"
        path.write_bytes(b"fn main() {}\n")
        run_session(binary, path, [b"i// ", "界e\u0301".encode(), b"\r", b"\x1b",
                                  b"u", b"\x12", b":wq\r"],
                    "// 界e\u0301\nfn main() {}\n".encode())
        run_session(binary, path, [b"iunsaved", b"\x1b", b":q\r", b":q!\r"],
                    "// 界e\u0301\nfn main() {}\n".encode())
        path.write_bytes("one two three\r\n界e\u0301\r\nlast".encode())
        run_session(binary, path, [b"cwnew", b"\x1b", b"j0vly", b"G$p", b"u",
                                  b"\x12", b":wq\r"],
                    "new two three\r\n界e\u0301\r\nlast界e\u0301".encode())
        path.write_bytes(b"one x one\r\none")
        run_session(binary, path, [b"/one\r", b"nNcwT", b"\x1b", b":wq\r"],
                    b"one x T\r\none")
        second = Path(directory) / "second.kt"
        path.write_bytes(b"alpha\r\n")
        second.write_bytes(b"beta\n")
        run_session(binary, path, [b"iA", b"\x1b", (b"NORMAL", 10), f":e {second}\r".encode(),
                                  b"iB", b"\x1b", (b"NORMAL", 10), b":q\r", b":w\r", b":q\r",
                                  b":ls\r", (b"Buffers", 10), b"\x1b", (b"beta", 10),
                                  b":bp\r", b":wq\r"],
                    b"Aalpha\r\n")
        assert second.read_bytes() == b"Bbeta\n"
        run_session(binary, path, [f":files {directory}\r".encode(), b"secondkt", b"\r",
                                  b"iC", b"\x1b", (b"NORMAL", 10), b":w\r", b":b 1\r", b":q\r"],
                    b"Aalpha\r\n")
        assert second.read_bytes() == b"CBbeta\n"
        output = run_session(binary, path, [b":theme catppuccin\r", b":theme gruvbox\r",
                                            b":theme tokyo-night\r", b":q\r"], b"Aalpha\r\n")
        for background in [b"48;2;30;30;46", b"48;2;40;40;40", b"48;2;26;27;38"]:
            assert background in output, f"Theme background was not rendered: {background!r}"
        def resize(rows, columns):
            def apply_resize(master, slave, pid):
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
                os.killpg(pid, signal.SIGWINCH)
            return apply_resize

        wide = (("\t界👩‍💻e\u0301\r\n" * 50) + "\t界" * 200).encode()
        path.write_bytes(wide)
        run_session(binary, path, [b"G$", resize(7, 19), b"gg", resize(3, 1),
                                  b"G", resize(24, 80), b":q\r"], wide)
        path.write_bytes(b"base\r\n")
        run_session(binary, path, [b"i", b"\x1b[200~:q!\r\n" + "界👩‍💻".encode() + b"\x1b[201~",
                                  b"\x1b", b":wq\r"], ":q!\r\n界👩‍💻base\r\n".encode())
        path.write_bytes(b"original\r\n")
        recovery = Path(directory) / "recovered.rs"
        run_session(binary, path, [b"ilocal ", b"\x1b",
                                  lambda master, slave, pid: path.write_bytes(b"external\r\n"),
                                  b"\x1b[I", b":w\r", f":w {recovery}\r".encode(), b":q\r"],
                    b"external\r\n")
        assert recovery.read_bytes() == b"local original\r\n"
        repo = Path(directory) / "git project 界"
        repo.mkdir()
        def git(*args):
            environment = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
                               GIT_AUTHOR_DATE="2001-01-01T00:00:00Z", GIT_COMMITTER_DATE="2001-01-01T00:00:00Z")
            subprocess.run(["git", "-C", str(repo), *args], env=environment, check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        git("init", "-b", "main")
        git("config", "user.name", "PTY Test")
        git("config", "user.email", "pty@example.invalid")
        git("config", "commit.gpgsign", "false")
        git("config", "core.autocrlf", "false")
        source = repo / "source.rs"
        second = repo / "second.rs"
        gone = repo / "gone.rs"
        rows = [f"row{index}\n" for index in range(60)]
        source.write_text("".join(rows))
        second.write_text("second\n")
        gone.write_text("historical deletion\n")
        status_source = repo / "src" / "界 status.txt"
        status_source.parent.mkdir()
        status_source.write_bytes(b"status fixture\r\n")
        short_status_path = "Gjensidige.Alarm.Bff.Notifications/Functions/NotificationOutboxFunction.cs"
        long_status_source = repo / "Gjensidige.Alarm.Bff" / short_status_path
        long_status_source.parent.mkdir(parents=True)
        long_status_source.write_bytes(b"// status fixture\r\n")
        git("add", "--all")
        git("commit", "-m", "base")
        # Ratatui can emit a cursor move after a wide glyph. Check the relative
        # prefix and filename separately instead of requiring contiguous bytes.
        relative_status = " src/界".encode()
        output = run_session(binary, status_source, [
            b":gitrefresh\r", resize(24, 200), (b"main", 10),
            resize(24, 201), (relative_status, 10), (b" status.txt", 10), b"iLOCAL ", b"\x1b",
            resize(7, 45), resize(3, 1), resize(24, 200), (relative_status, 10),
            (b" status.txt", 10), b"u", b":q\r",
        ], b"status fixture\r\n", cwd=status_source.parent)
        if capture_dir:
            (Path(capture_dir) / "repository-relative-status.ansi").write_bytes(output)
        # Keep the extra Git status label identical locally and on hosted runners.
        git("config", "filter.status-fixture.clean", "cat")
        shortened_status = f" {short_status_path}".encode()
        output = run_session(binary, long_status_source, [
            b":gitrefresh\r", resize(24, 260), (b"main", 10), (b"Failed", 10),
            resize(24, 200), (shortened_status, 10), b"i// LOCAL ", b"\x1b",
            resize(7, 45), resize(3, 1), resize(24, 200), (shortened_status, 10),
            b"u", resize(24, 260), (f" Gjensidige.Alarm.Bff/{short_status_path}".encode(), 10),
            b":q\r",
        ], b"// status fixture\r\n", cwd=long_status_source.parent,
            environment={"EDITOR_PRO_CSHARP_LSP": json.dumps([str(repo / "missing-status-lsp")])})
        git("config", "--remove-section", "filter.status-fixture")
        if capture_dir:
            (Path(capture_dir) / "shortened-status-path.ansi").write_bytes(output)
        rows[2] = "STAGED change\n"
        source.write_text("".join(rows))
        git("add", "source.rs")
        rows[50] = "WORKING change\n"
        source.write_text("".join(rows))
        second.write_text("changed second\n")
        gone.unlink()
        (repo / "untracked.rs").write_text("untracked addition\n")
        index_before = (repo / ".git" / "index").read_bytes()
        shortcut_rows = rows.copy()
        shortcut_rows[2] = "KEYS" + shortcut_rows[2]
        output = run_session(binary, source, [
            (b"gg2]c", b"Git hunk 2/2", 10), b"[c", b" gd", (b"OLD ", 10),
            b"\t", resize(24, 130), (b"paired", 10),
            (b"n", b"untracked addition", 10), resize(8, 40), resize(3, 1),
            resize(24, 130), b"\x1b", b"iKEYS", b"\x1b", b":wq\r",
        ], "".join(shortcut_rows).encode())
        assert (repo / ".git" / "index").read_bytes() == index_before
        if capture_dir:
            (Path(capture_dir) / "git-keyboard-navigation.ansi").write_bytes(output)
        source.write_text("".join(rows))
        expected_rows = rows.copy()
        expected_rows[0] = "LOCAL" + expected_rows[0]
        expected_rows[29] = "RETURN" + expected_rows[29]
        output = run_session(binary, source, [b"iLOCAL", b"\x1b", b"30G0", b" gd",
            (b"UNSAVED", 10), b"\t", resize(24, 130), (b"paired", 10), b"]", b"[",
            resize(10, 45), b"\x06", b"r", resize(3, 1), resize(24, 130), b"\x1b",
            b"iRETURN", b"\x1b", b":wq\r"], "".join(expected_rows).encode())
        assert b"OLD " in output and b"NEW " in output
        assert (repo / ".git" / "index").read_bytes() == index_before
        if capture_dir:
            (Path(capture_dir) / "git-shortcut-dirty-origin.ansi").write_bytes(output)
        output = run_session(binary, source, [b" gc", (b"gone.rs", 10),
            # Review loading can briefly invalidate the list after it renders.
            # Retry navigation until the destination appears instead of losing
            # a single Enter during that refresh window.
            (b"\r", b"historical deletion", 10), b"\r", b"\x1b", b" gc", (b"second.rs", 10), b"j",
            (b"\r", b"changed second", 10),
            b"iSECOND ", b"\x1b", b":w\r", b"]f", b"[f", b":q\r"],
            "".join(expected_rows).encode())
        assert second.read_bytes() == b"SECOND changed second\n", (second.read_bytes(), output[-12000:])
        assert not gone.exists()
        assert (repo / ".git" / "index").read_bytes() == index_before
        if capture_dir:
            (Path(capture_dir) / "git-shortcut-changed-files.ansi").write_bytes(output)
        output = run_session(binary, source, [b":diff staged\r", (b"index@", 10), b":diff index\r",
            b":diff HEAD\r", b":diff missing-revision\r", (b"Unavailable revision", 10),
            b":diff HEAD\r", b"n", b"r", b"\x1b", b":q\r"], "".join(expected_rows).encode())
        assert (repo / ".git" / "index").read_bytes() == index_before
        # M4: inspect, mark and annotate without source writes; restart, detect
        # changed content, export, and recover a failed metadata write.
        store = repo / ".git" / "editor-pro-review.json"
        feedback = Path(directory) / "review feedback 界.md"
        reviewed_bytes = source.read_bytes()
        def saved(predicate):
            def check():
                if not store.exists():
                    return False
                return predicate(json.loads(store.read_text())["records"])
            return check
        has_file = saved(lambda rs: any(r["state"] == "Reviewed" and r["attachment"] == "File" for r in rs))
        has_hunk = saved(lambda rs: any(r["state"] == "Attention" for r in rs))
        note_text = "界👩‍💻 review\nCheck `edge` case <tag>."
        has_note = saved(lambda rs: any(r["note"] == note_text for r in rs))
        output = run_session(binary, source, [b"30G0", b":changes\r", (b"source.rs", 10), b"\x1b", b":diff HEAD\r",
            (b":review file reviewed\r", has_file, 10),
            (b":review hunk attention\r", has_hunk, 10),
            resize(24, 130), b"\t", resize(10, 45), b"\t", resize(24, 130),
            (b":note hunk\r", b"NOTE ", 10), b"i", b"\x1b[200~" + note_text.encode() + b"\x1b[201~", b"\x1b",
            b":note save\r", (has_note, 10), (b":notes\r", b"REVIEW records", 10),
            (b"fresh", 10), b"\x1b", b"\x1b", b"iRETURN", b"\x1b", b"u", b":q\r"], reviewed_bytes)
        assert (repo / ".git" / "index").read_bytes() == index_before
        output = run_session(binary, source, [b":diff HEAD\r", (b":notes\r", b"REVIEW records", 10), (b"fresh", 10),
            b"\x1b", b"\x1b", lambda master, slave, pid: source.write_bytes(reviewed_bytes.replace(b"WORKING", b"ALTERED")),
            b":e!\r", b":diff HEAD\r", b":review list stale\r", (b"STALE", 10), b"\x1b",
            (f":review export {feedback}\r".encode(), feedback.exists, 10), b":q\r"], reviewed_bytes.replace(b"WORKING", b"ALTERED"))
        report = feedback.read_text()
        assert "STALE" in report and "needs-attention" in report and "界👩‍💻" in report, report
        assert "&#96;edge&#96;" in report and "&#60;tag&#62;" in report, report
        assert (repo / ".git" / "index").read_bytes() == index_before
        lock = store.with_suffix(".json.lock")
        lock.write_text("test writer")
        recovered = saved(lambda rs: any(r["note"] == "recovered note" for r in rs))
        output = run_session(binary, source, [b":diff HEAD\r", (b":note file\r", b"NOTE ", 10),
            b"irecovered note", b"\x1b", b":note save\r", (b"lock unavailable", 10),
            lambda master, slave, pid: lock.unlink(), b":note save\r", (recovered, 10),
            f":review export {feedback}\r".encode(), (b"already exists", 10), b":q\r"], source.read_bytes())
        assert feedback.read_text() == report
        assert (repo / ".git" / "index").read_bytes() == index_before
        # M5 uses a labelled test double: protocol/UI validation, not real-server acceptance.
        lsp_root = Path(directory) / "lsp project"
        lsp_root.mkdir()
        (lsp_root / "pom.xml").write_text("<project/>")
        language_source = lsp_root / "Main.kt"
        language_source.write_text("bad value\n")
        log = lsp_root / "protocol.jsonl"
        fake = Path(__file__).parent / "lsp" / "fake_server.py"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps(
            [sys.executable, str(fake.resolve()), "--log", str(log)])}
        def server_ready():
            return log.exists() and '"method": "textDocument/didOpen"' in log.read_text()
        output = run_session(binary, language_source, [
            (server_ready, 10), b":Hover\r", (b"Documentation", 10), b"\x1b",
            b":Format\r", (b"Enter applies", 10), resize(3, 1), resize(24, 100),
            b"\r", (b"unsaved", 10), b"u", b":Format\r", (b"Enter applies", 10),
            b"\r", b":wq\r",
        ], b"good value\n", environment)
        assert '"method": "shutdown"' in log.read_text()
        assert '"method": "exit"' in log.read_text()
        language_source.write_text("bad value\n")
        log = lsp_root / "installation-controls.jsonl"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps(
            [sys.executable, str(fake.resolve()), "--log", str(log)])}
        output = run_session(binary, language_source, [
            (server_ready, 10), b":LspInfo\r", (b"Automatic installation: false", 10), b"\x1b",
            b":LspInstall rust\r", (b"Supported installers", 10),
            b":LspInstallCancel\r", b":LspInfo\r", (b"No installation running", 10),
            resize(3, 1), resize(24, 100), b"\x1b", b":q\r",
        ], b"bad value\n", environment)
        capture_dir = os.environ.get("EDITOR_PRO_SMOKE_CAPTURE_DIR")
        if capture_dir:
            Path(capture_dir).mkdir(parents=True, exist_ok=True)
            (Path(capture_dir) / "lsp-installation-controls.ansi").write_bytes(output)
        language_source.write_text("bad value\n")
        log = lsp_root / "completion.jsonl"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
            sys.executable, str(fake.resolve()), "--log", str(log),
            "--scenario", "completion-command",
        ])}
        output = run_session(binary, language_source, [
            (server_ready, 10), b":Complete\r", (b"detail", 10),
            b"\x1b", b":Complete\r", (b"detail", 10), b"\r",
            resize(24, 101), (b"Completion inserted", 10), b"u", b":Format\r", (b"Enter applies", 10),
            b"\r", b":wq\r",
        ], b"good value\n", environment)
        messages = [json.loads(line) for line in log.read_text().splitlines()]
        assert sum(m.get("method") == "workspace/executeCommand" for m in messages) == 1
        assert any(m.get("id") == 777 and m.get("result", {}).get("applied") for m in messages)
        completion_source = ("value.\r\n" + (
            "// Text beneath the menu: 界e\u0301 " + "value.length + " * 5 + "\r\n"
        ) * 3).encode()
        language_source.write_bytes(completion_source)
        log = lsp_root / "automatic-completion.jsonl"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
            sys.executable, str(fake.resolve()), "--log", str(log),
            "--scenario", "auto-completion",
        ])}
        output = run_session(binary, language_source, [
            (server_ready, 10), b"Ale", (b"Tab/Enter accept", 10),
            resize(3, 1), resize(24, 100), b"\t", (b"inserted", 10),
            b"\x1b", b"u", b"A\x7fe", (b"Tab/Enter accept", 10),
            b"\x05", b"\x1b", b":q!\r",
        ], completion_source, environment)
        messages = [json.loads(line) for line in log.read_text().splitlines()]
        assert sum(m.get("method") == "textDocument/completion" for m in messages) >= 2
        assert any(m.get("method") == "completionItem/resolve" for m in messages)
        if capture_dir:
            (Path(capture_dir) / "automatic-completion.ansi").write_bytes(output)
        run_completion_acceptance_session(binary, directory, resize, capture_dir)
        run_completion_edits_sessions(binary, directory, resize, capture_dir)
        # M6: disk search and a multi-file path with preserved location/undo.
        navigation_root = Path(directory) / "navigation"
        navigation_root.mkdir()
        navigation_source = navigation_root / "source.txt"
        navigation_target = navigation_root / "target.txt"
        navigation_bytes = b"source row\n" * 80
        navigation_source.write_bytes(navigation_bytes)
        navigation_target.write_bytes("😀 needle target\r\n".encode())
        output = run_session(binary, navigation_source, [
            b"40G0", f":grepdir {navigation_root}\r".encode(), b":grep needle\r",
            (b"target.txt:1:6", 10), resize(2, 1), resize(24, 100), b"\r",
            resize(24, 101), (b"Project search location", 10), b"\x0f",
            resize(24, 100), (b"Earlier location", 10),
            b"iRETURN", b"\x1b", (b"NORMAL", 10), b"\t",
            resize(24, 101), (b"Later location", 10),
            b":back\r", b"u", b":recent\r", resize(24, 100), (b"Recent files", 10), b"j\r",
            b":back\r", b":q\r",
        ], navigation_bytes)
        assert navigation_target.read_bytes() == "😀 needle target\r\n".encode()
        capture_dir = os.environ.get("EDITOR_PRO_SMOKE_CAPTURE_DIR")
        if capture_dir:
            Path(capture_dir).mkdir(parents=True, exist_ok=True)
            (Path(capture_dir) / "m6-search-history.ansi").write_bytes(output)

        # The comprehension UI still uses a protocol double, separately from lsp_real.rs.
        language_source.write_text("bad value\n")
        (lsp_root / "Other.kt").write_text("bad other\n")
        log = lsp_root / "comprehension.jsonl"
        environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
            sys.executable, str(fake.resolve()), "--log", str(log),
        ])}
        output = run_session(binary, language_source, [
            (server_ready, 10), b":Outline\r", resize(24, 100), (b"Symbol outline", 10), b"\x1b",
            b":WorkspaceSymbols lazy\r", resize(24, 101), (b"lazy Project", 10), b"\r",
            resize(24, 100), (b"Language-server location", 10), b"\x0f", b":Calls\r",
            resize(24, 101), (b"Call hierarchy", 10), b"i", resize(24, 100), (b"caller", 10),
            b"o", resize(24, 101), (b"callee", 10), b"\x7f", b"\r",
            resize(24, 100), (b"Language-server location", 10), b"\x0f", b":q\r",
        ], b"bad value\n", environment)
        assert (lsp_root / "Other.kt").read_bytes() == b"bad other\n"
        if capture_dir:
            (Path(capture_dir) / "m6-symbols-calls.ansi").write_bytes(output)
        # Normal-mode navigation shortcuts: immediate single jumps and a chooser.
        for scenario in ("normal", "navigation-multiple"):
            log = lsp_root / f"navigation-{scenario}.jsonl"
            environment = {"EDITOR_PRO_KOTLIN_LSP": json.dumps([
                sys.executable, str(fake.resolve()), "--log", str(log),
                "--scenario", scenario,
            ])}
            commands = [(server_ready, 10), b"w"]
            for shortcut, title in ((b"gd", b"definition"), (b"gr", b"references")):
                commands.append(shortcut)
                if scenario == "navigation-multiple":
                    commands.extend([
                        resize(24, 101), (title, 10), b"\x1b", shortcut,
                        resize(24, 100), (title, 10), resize(3, 1), resize(24, 100),
                        b"j\r",
                    ])
                commands.extend([
                    resize(24, 101), (b"Language-server location", 10),
                    b"\x0f", resize(24, 100), (b"Earlier location", 10),
                ])
            commands.extend([b"\t", resize(24, 101), (b"Later location", 10), b":q\r"])
            output = run_session(binary, language_source, commands, b"bad value\n", environment)
            assert (lsp_root / "Other.kt").read_bytes() == b"bad other\n"
            messages = [json.loads(line) for line in log.read_text().splitlines()]
            for method in ("textDocument/definition", "textDocument/references"):
                requests = [m for m in messages if m.get("method") == method]
                assert len(requests) == (2 if scenario == "navigation-multiple" else 1)
                assert all(m["params"]["position"] == {"line": 0, "character": 4} for m in requests)
            if capture_dir:
                (Path(capture_dir) / f"navigation-{scenario}.ansi").write_bytes(output)
        # C# generated/decompiled navigation uses read-only virtual buffers.
        metadata_source = lsp_root / "Main.cs"
        metadata_source.write_text("class Main { Library value; }\n")
        (lsp_root / "Probe.csproj").write_text("<Project/>")
        log = lsp_root / "csharp-metadata.jsonl"
        environment = {"EDITOR_PRO_CSHARP_LSP": json.dumps([
            sys.executable, str(fake.resolve()), "--log", str(log),
            "--scenario", "metadata-normal",
        ])}
        output = run_session(binary, metadata_source, [
            (server_ready, 10), b"gd", resize(24, 100), (b"Read-only generated/decompiled", 10),
            resize(24, 101), (b"[RO]", 10), b"i", resize(24, 100), (b"source is read only", 10),
            b"gd", resize(24, 101), (b"GeneratedThing", 10),
            b"\x0f", resize(24, 100), (b"Library", 10),
            b"\x0f", resize(24, 101), (b"Earlier location", 10), b":q\r",
        ], b"class Main { Library value; }\n", environment)
        if capture_dir:
            (Path(capture_dir) / "csharp-metadata.ansi").write_bytes(output)
        # M7: immutable checkpoints, live external revisions, and retained source.
        repo = Path(directory) / "snapshot project"
        repo.mkdir()
        git("init", "-b", "main")
        git("config", "user.name", "PTY Snapshot Test")
        git("config", "user.email", "snapshot@example.invalid")
        git("config", "commit.gpgsign", "false")
        git("config", "core.autocrlf", "false")
        source = repo / "source.txt"
        source.write_bytes(b"base revision\n")
        git("add", "--all")
        git("commit", "-m", "snapshot base")
        source.write_bytes(b"first revision\n")
        store = repo / ".git" / "editor-pro-review.json"
        index_before = (repo / ".git" / "index").read_bytes()
        def checkpoint_count(count):
            return lambda: store.exists() and len(json.loads(store.read_text()).get("history", {}).get("checkpoints", [])) == count
        output = run_session(binary, source, [
            (b":review snapshots\r", b"No snapshots", 10),
            b":review snapshot\r", (checkpoint_count(1), 30),
            (b":review full\r", b"review full HEAD", 10),
            (b":review file reviewed\r", saved(lambda rs: any(r["state"] == "Reviewed" for r in rs)), 10),
            b":review since\r", (b"No textual changes", 10),
            lambda master, slave, pid: source.write_bytes(b"later revision\n"),
            b"\x1b[I", resize(24, 130), (b"later revision", 10),
            b":review full\r", resize(24, 129), (b"base revision", 10),
            b":review list stale\r", resize(24, 130), (b"STALE", 10),
            b"\x1b", b"\x1b", resize(24, 129), (b"first revision", 10),
            b"iLOCAL ", b"\x1b", b":review since\r", resize(24, 130), (b"UNSAVED", 10),
            b"\t", resize(3, 1), resize(24, 130), b"\x1b", b"u", b":q\r",
        ], b"later revision\n")
        assert (repo / ".git" / "index").read_bytes() == index_before
        if capture_dir:
            (Path(capture_dir) / "m7-live-review.ansi").write_bytes(output)
        output = run_session(binary, source, [
            (b":review since\r", b"review checkpoint #1", 10),
            resize(24, 130), (b"first revision", 10),
            b":review snapshot\r", (checkpoint_count(2), 30),
            b":review since\r", (b"No textual changes", 10),
            b":review full\r", resize(24, 129), (b"base revision", 10),
            b":changes\r", resize(24, 130), (b"snapshot delta", 10), b"d", b"\x1b", b":q\r",
        ], b"later revision\n")
        assert (repo / ".git" / "index").read_bytes() == index_before
        history = json.loads(store.read_text())["history"]
        assert history["baseline"]["files"][0]["text"] == "base revision\n"
        assert [c["files"][0]["text"] for c in history["checkpoints"]] == ["first revision\n", "later revision\n"]
        if capture_dir:
            (Path(capture_dir) / "m7-checkpoint-resume.ansi").write_bytes(output)
        # M8: protocol-double conversation, explicit revisions, undo and resume.
        # Actual Codex acceptance is separately opt-in in tests/ai_real.rs.
        ai_root = Path(directory) / "ai project"
        ai_root.mkdir()
        (ai_root / "Cargo.toml").write_text("[package]\n")
        source = ai_root / "source.txt"
        original = "old 界👩‍💻\r\n".encode()
        source.write_bytes(original)
        conversation = Path(directory) / "conversation.json"
        ai_environment = {"EDITOR_PRO_CODEX": str(Path("tools/ai/fake_codex.py").resolve()),
                          "EDITOR_PRO_AI_MODEL": "edit"}
        def ai_turns(count):
            return lambda: conversation.exists() and len(json.loads(conversation.read_text())["turns"]) == count
        output = run_session(binary, source, [
            b"ilocal ", b"\x1b", b":ai context file\r", resize(24, 100), (b"Context preview", 10),
            b":ai ask Revise old to new\r", b":ai send\r",
            # The headline can be split into ANSI diff writes; the turn status
            # changes entirely from Pending/Receiving to Completed.
            (b"Completed", 10), b":ai edits\r", resize(24, 101), (b"- old", 10),
            b":ai apply\r", b"\x1b", resize(24, 100), (b"local new", 10),
            b"u", b"u", b":q\r", resize(24, 101), (b"AI conversation pending or unsaved", 10),
            f":ai save {conversation}\r".encode(), (ai_turns(1), 10), b":q\r",
        ], original, ai_environment)
        if capture_dir:
            (Path(capture_dir) / "m8-ai-revisions.ansi").write_bytes(output)
        ai_environment["EDITOR_PRO_AI_MODEL"] = "followup"
        output = run_session(binary, source, [
            # The status headline can be split into ANSI diff writes. The
            # restored turn is new content and confirms that loading finished.
            f":ai load {conversation}\r".encode(), resize(24, 100), (b"Turn 1", 10),
            b":ai edits\r", resize(24, 101), (b"historical", 10),
            b":ai context file\r", b":ai ask Recall the previous discussion\r", b":ai send\r",
            (b"Completed", 10), b":ai save\r", (ai_turns(2), 10),
            resize(3, 1), resize(24, 101), b"\x1b", b":q\r",
        ], original, ai_environment)
        if capture_dir:
            (Path(capture_dir) / "m8-ai-resume.ansi").write_bytes(output)
        # Repeated real keypresses must walk distinct sessions in both directions.
        history_path = Path(directory) / "undo-history.txt"
        history_states = ["界\r\nlast"]
        history_path.write_bytes(history_states[0].encode())
        history_commands = []
        for word in ["one", "two", "three"]:
            history_commands.extend([f"A{word}".encode(), b"\x1b"])
            history_states.append(history_states[-1].replace("\r\n", word + "\r\n", 1))
        for key, expected_states in [(b"u", list(reversed(history_states[:3]))),
                                     (b"\x12", history_states[1:])]:
            for expected_state in expected_states:
                expected_bytes = expected_state.encode()
                history_commands.extend([
                    key, b":w\r",
                    (lambda expected=expected_bytes: history_path.read_bytes() == expected, 10),
                ])
        history_commands.extend([resize(3, 1), resize(24, 101), b":q\r"])
        output = run_session(binary, history_path, history_commands, history_states[-1].encode())
        if capture_dir:
            (Path(capture_dir) / "multi-step-undo-redo.ansi").write_bytes(output)
        run_splash_sessions(binary, directory, resize, capture_dir)
        run_file_tree_session(binary, directory, resize, capture_dir)
        run_colorful_ui_sessions(binary, directory, resize, capture_dir)
        run_lsp_warmup_sessions(binary, directory, resize, capture_dir)
        run_lsp_startup_session(binary, directory, resize, capture_dir)
        run_help_session(binary, directory, resize, capture_dir)
        run_disk_refresh_sessions(binary, directory, resize, capture_dir)
        run_ai_explain_sessions(binary, directory, resize, capture_dir)
        run_binary_snapshot_session(binary, directory, resize, capture_dir)
    print("PASS: 63 PTY sessions; colorful tree/picker/buffers in all themes, ASCII splash/startup shortcuts, JVM startup/indexing/failure/recovery, contextual Codex explanation shortcuts, disk comparison/refresh/conflicts/deletion, searchable keyboard help, file tree, ep directory/file startup, user configuration, editing, multi-step undo/redo, search, buffers, picker, themes, resize, paste, repository-relative and shortened status paths, Git keyboard navigation, review/resume/export, LSP warmup/reuse, gd/gr navigation including C# generated/decompiled sources, one-step completion with imports/multiline/commands/trailing spaces and installation controls, M6 project search/history/recent files/symbols/call hierarchy, M7 snapshots/live refresh/resume including binary assets, M8 AI preview/revisions/undo/persistence, and restoration")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
