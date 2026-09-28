import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Screen:
    def __init__(self):
        self.cells = [[" " for _ in range(200)] for _ in range(60)]
        self.row = 0
        self.column = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith("\x1b"):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                parameters, _, command = match.groups()
                if command in ("H", "f"):
                    values = [int(value or "1") for value in parameters.split(";")]
                    self.row, self.column = values[0] - 1, (values[1] if len(values) > 1 else 1) - 1
                elif command == "J" and parameters == "2":
                    self.cells = [[" " for _ in range(200)] for _ in range(60)]
                self.pending = self.pending[match.end():]
            else:
                character, self.pending = self.pending[0], self.pending[1:]
                if character == "\r":
                    self.column = 0
                elif character == "\n":
                    self.row += 1
                elif 0 <= self.row < 60 and 0 <= self.column < 200:
                    self.cells[self.row][self.column] = character
                    self.column += 1

    def text(self):
        return "\n".join("".join(row) for row in self.cells).encode()


class TerminalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(
            ["cargo", "build", "-p", "fluzo-cli", "--locked", "--offline", "--message-format=json"],
            cwd=ROOT, capture_output=True, text=True, check=True, timeout=120,
        )
        artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        cls.binary = next(item["executable"] for item in artifacts if item.get("executable") and item.get("target", {}).get("name") == "fluzo")

    def test_noninteractive_invocation_fails_without_escape_output(self):
        result = subprocess.run([self.binary, "demo", "--interactive"], env={}, capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"requires terminal", result.stderr)
        self.assertNotIn(b"\x1b", result.stderr)

    def exercise(self, interrupted):
        with tempfile.TemporaryDirectory(prefix="fluzo-tui-") as directory:
            root = Path(directory)
            (root / ".fluzo").write_text("invalid configuration must not be read")
            (root / "source.rs").write_text("unchanged fixture")
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            child = subprocess.Popen([self.binary, "demo", "--interactive"], stdin=slave, stdout=slave, stderr=slave,
                                     cwd=root, env={"HOME": directory, "XDG_CONFIG_HOME": directory, "TERM": "xterm-256color", "NO_COLOR": "1"})
            output = bytearray()
            screen = Screen()

            def wait_for(marker):
                deadline = time.monotonic() + 5
                while marker not in screen.text():
                    if time.monotonic() >= deadline:
                        self.fail(f"Missing terminal marker {marker!r}; recent synthetic output={bytes(output[-2000:])!r}")
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        self.assertTrue(data)
                        output.extend(data)
                        screen.feed(data)
                        self.assertLess(len(output), 1024 * 1024)

            try:
                wait_for(b"Composer")
                self.assertNotEqual(termios.tcgetattr(slave), before)
                os.write(master, b"\x1b[200~draft\n\x1b]52;c;hidden\x07text\x1b[201~")
                wait_for(b"draft")
                os.write(master, b"\x10")
                wait_for(b"Command palette")
                os.write(master, b"play\r")
                wait_for(b"Synthetic fragment")
                os.write(master, b"\x03")
                wait_for(b"Preview stopped")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
                child.send_signal(signal.SIGWINCH)
                if interrupted:
                    child.send_signal(signal.SIGTERM)
                else:
                    os.write(master, b"\x11")
                    wait_for(b"Discard unsaved")
                    os.write(master, b"\t\r")
                child.wait(timeout=5)
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                self.assertEqual(child.returncode, 0)
                self.assertEqual(termios.tcgetattr(slave), before)
                self.assertIn(b"\x1b[?2004l", output)
                self.assertIn(b"\x1b[?1049l", output)
                self.assertIn(b"\x1b[?25h", output)
                self.assertNotIn(b"]52;", output)
                self.assertNotIn(b"hidden", output)
                self.assertEqual((root / "source.rs").read_text(), "unchanged fixture")
                self.assertEqual((root / ".fluzo").read_text(), "invalid configuration must not be read")
                self.assertEqual(len(list(root.iterdir())), 2)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                os.close(master)
                os.close(slave)

    def test_paste_palette_stream_resize_and_normal_exit_restore_terminal(self):
        self.exercise(False)

    def test_external_termination_restores_terminal(self):
        self.exercise(True)


if __name__ == "__main__":
    unittest.main()
