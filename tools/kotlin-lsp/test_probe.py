import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

from probe import encode_message, position, read_message


HERE = Path(__file__).resolve().parent


class FramingTests(unittest.TestCase):
    def test_content_length_counts_utf8_bytes(self):
        message = {"jsonrpc": "2.0", "id": 1, "result": "界👩‍💻"}
        encoded = encode_message(message)
        header, data = encoded.split(b"\r\n\r\n", 1)
        self.assertEqual(int(header.split(b":")[1]), len(data))
        self.assertEqual(read_message(io.BytesIO(encoded)), message)

    def test_extra_headers_and_back_to_back_frames(self):
        data = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"
        data += encode_message({"id": 1}) + encode_message({"id": 2})
        stream = io.BytesIO(data)
        self.assertEqual(read_message(stream)["id"], 1)
        self.assertEqual(read_message(stream)["id"], 2)

    def test_truncated_body_and_stdout_noise_are_failures(self):
        with self.assertRaises(EOFError):
            read_message(io.BytesIO(b"Content-Length: 20\r\n\r\n{}"))
        with self.assertRaises(ValueError):
            read_message(io.BytesIO(b"Starting server...\n"))

    def test_positions_count_utf16_units(self):
        self.assertEqual(position("// marker\n👩 token // marker2", "marker2", "token"),
                         {"line": 1, "character": 3})


class ProcessTests(unittest.TestCase):
    def run_probe(self, command, timeout=5):
        with tempfile.TemporaryDirectory() as directory:
            result = Path(directory) / "result.json"
            process = subprocess.run([
                sys.executable, str(HERE / "probe.py"), "--server", command,
                "--timeout", str(timeout), "--output", str(result),
                "--stderr", str(Path(directory) / "stderr.log"),
            ], capture_output=True, text=True, timeout=15)
            self.assertTrue(result.exists(), process.stderr)
            return process, json.loads(result.read_text())

    def test_protocol_round_trip_with_test_double(self):
        command = shlex.join([sys.executable, str(HERE / "fake_server.py")])
        process, report = self.run_probe(command)
        self.assertEqual(process.returncode, 0, report)
        self.assertEqual(report["status"], "passed")
        self.assertTrue(report["diagnostic_version_verified"])
        self.assertIn("workspace/configuration", report["server_requests"])
        self.assertIn("TEST DOUBLE", report["initialize"]["serverInfo"]["name"])

    def test_missing_server_is_blocked_not_passed(self):
        process, report = self.run_probe("/does-not-exist/editor-pro-kotlin-lsp")
        self.assertEqual(process.returncode, 2)
        self.assertEqual(report["status"], "blocked")
        self.assertNotIn("initialize", report)

    def test_exit_closes_stdin_for_servers_waiting_for_transport_eof(self):
        command = shlex.join([sys.executable, str(HERE / "fake_server.py"), "--exit-on-eof"])
        process, report = self.run_probe(command)
        self.assertEqual(process.returncode, 0, report)
        self.assertEqual(report["status"], "passed")

    def test_hung_server_times_out_and_is_reaped(self):
        command = shlex.join([sys.executable, str(HERE / "fake_server.py"), "--hang"])
        process, report = self.run_probe(command, timeout=0.2)
        self.assertEqual(process.returncode, 1)
        self.assertEqual(report["status"], "failed")
        self.assertIn("Timed out", report["reason"])


if __name__ == "__main__":
    unittest.main()
