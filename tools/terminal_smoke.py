#!/usr/bin/env python3
"""Exercise the real binary via a PTY on Linux/macOS. No third-party modules."""
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import sys
import tempfile
import termios
import time


def run_session(binary, path, commands, expected):
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
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
        os.execv(str(binary), [str(binary), str(path)])
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
        for index, command in enumerate(commands):
            if callable(command):
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
                assert os.waitstatus_to_exitcode(status) == 0, output.decode(errors="replace")
                break
        assert reaped, "Editor did not exit"
        assert termios.tcgetattr(slave) == original, "Terminal settings were not restored"
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


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/editor-pro").resolve()
    if not binary.is_file():
        print(f"BLOCKED: build the editor first; binary missing: {binary}", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="editor-pro-pty-") as directory:
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
        run_session(binary, path, [b"iA", b"\x1b", f":e {second}\r".encode(),
                                  b"iB", b"\x1b", b":q\r", b":w\r", b":q\r",
                                  b":ls\r", b"\x1b", b":bp\r", b":wq\r"],
                    b"Aalpha\r\n")
        assert second.read_bytes() == b"Bbeta\n"
        run_session(binary, path, [f":files {directory}\r".encode(), b"secondkt", b"\r",
                                  b"iC", b"\x1b", b":w\r", b":b 1\r", b":q\r"],
                    b"Aalpha\r\n")
        assert second.read_bytes() == b"CBbeta\n"
        output = run_session(binary, path, [b":theme catppuccin\r", b":theme gruvbox\r",
                                            b":theme tokyo-night\r", b":q\r"], b"Aalpha\r\n")
        for background in [b"48;2;30;30;46", b"48;2;40;40;40", b"48;2;26;27;38"]:
            assert background in output, f"Theme background was not rendered: {background!r}"
        def resize(rows, columns):
            def apply_resize(master, slave, pid):
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
                os.kill(pid, signal.SIGWINCH)
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
    print("PASS: 10 PTY sessions; modal editing, search, buffers, picker, themes, resize, paste, disk conflicts, and restoration")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
