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
        git("add", "--all")
        git("commit", "-m", "base")
        rows[2] = "STAGED change\n"
        source.write_text("".join(rows))
        git("add", "source.rs")
        rows[50] = "WORKING change\n"
        source.write_text("".join(rows))
        second.write_text("changed second\n")
        gone.unlink()
        (repo / "untracked.rs").write_text("untracked addition\n")
        index_before = (repo / ".git" / "index").read_bytes()
        expected_rows = rows.copy()
        expected_rows[0] = "LOCAL" + expected_rows[0]
        expected_rows[29] = "RETURN" + expected_rows[29]
        output = run_session(binary, source, [b"iLOCAL", b"\x1b", b"30G0", b":diff HEAD\r",
            (b"UNSAVED", 10), b"\t", resize(24, 130), (b"paired", 10), b"]", b"[",
            resize(10, 45), b"\x06", b"r", resize(3, 1), resize(24, 130), b"\x1b",
            b"iRETURN", b"\x1b", b":wq\r"], "".join(expected_rows).encode())
        assert b"OLD " in output and b"NEW " in output
        assert (repo / ".git" / "index").read_bytes() == index_before
        output = run_session(binary, source, [b":changes\r", (b"gone.rs", 10),
            # Review loading can briefly invalidate the list after it renders.
            # Retry navigation until the destination appears instead of losing
            # a single Enter during that refresh window.
            (b"\r", b"historical deletion", 10), b"\r", b"\x1b", b":changes\r", (b"second.rs", 10), b"j",
            (b"\r", b"changed second", 10),
            b"iSECOND ", b"\x1b", b":w\r", b":gnext\r", b":gprev\r", b":q\r"],
            "".join(expected_rows).encode())
        assert second.read_bytes() == b"SECOND changed second\n", (second.read_bytes(), output[-12000:])
        assert not gone.exists()
        assert (repo / ".git" / "index").read_bytes() == index_before
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
            b"\r", (b"fake.fix", 10), b"\x1b",
            b":Complete\r", (b"detail", 10), b"\r",
            (b"fake.fix", 10), b"\r", (b"good", 10),
            b"\r", (b"unsaved", 10), b"u", b":Format\r", (b"Enter applies", 10),
            b"\r", b":wq\r",
        ], b"good value\n", environment)
        messages = [json.loads(line) for line in log.read_text().splitlines()]
        assert sum(m.get("method") == "workspace/executeCommand" for m in messages) == 1
        assert any(m.get("id") == 777 and m.get("result", {}).get("applied") for m in messages)
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
    print("PASS: 28 PTY sessions; ep directory/file startup, user configuration, editing, search, buffers, picker, themes, resize, paste, disk conflicts, Git, review/resume/export, LSP edits and installation controls, M6 project search/history/recent files/symbols/call hierarchy, M7 snapshots/live refresh/resume, M8 AI preview/revisions/undo/persistence, and restoration")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
