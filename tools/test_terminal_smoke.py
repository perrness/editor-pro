"""Check that the PTY harness still rejects broken terminal cleanup and exits."""
from pathlib import Path
import sys
import tempfile
import unittest

from terminal_smoke import run_session


class TerminalSmokeTests(unittest.TestCase):
    def run_fixture(self, restore=True, exit_code=0, mouse_restore=True):
        with tempfile.TemporaryDirectory(prefix="editor-pro-smoke-test-") as directory:
            root = Path(directory)
            source = root / "source.txt"
            source.write_bytes(b"unchanged\n")
            binary = root / "fake-editor"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import os, termios, tty\n"
                "original = termios.tcgetattr(0)\n"
                "tty.setraw(0)\n"
                "os.write(1, b'\\x1b[?1003hNORMAL')\n"
                "os.read(0, 1)\n"
                + ("termios.tcsetattr(0, termios.TCSANOW, original)\n" if restore else "")
                + ("os.write(1, b'\\x1b[?1003l')\n" if mouse_restore else "")
                + "os.write(1, b'\\x1b[?1049l\\x1b[?2004l')\n"
                + f"os._exit({exit_code})\n"
            )
            binary.chmod(0o755)
            return run_session(binary, source, [b"q"], b"unchanged\n")

    def test_restored_terminal_and_successful_exit_pass(self):
        self.assertIn(b"NORMAL", self.run_fixture())

    def test_unrestored_terminal_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "Terminal settings were not restored"):
            self.run_fixture(restore=False)

    def test_nonzero_editor_exit_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "exited with status 7"):
            self.run_fixture(exit_code=7)

    def test_enabled_mouse_capture_must_be_disabled_on_exit(self):
        with self.assertRaisesRegex(AssertionError, "Mouse mode 1003 was not disabled"):
            self.run_fixture(mouse_restore=False)


if __name__ == "__main__":
    unittest.main()
