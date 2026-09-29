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
        self.cells = [[" " for _ in range(260)] for _ in range(90)]
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
                    self.cells = [[" " for _ in range(260)] for _ in range(90)]
                self.pending = self.pending[match.end():]
            else:
                character, self.pending = self.pending[0], self.pending[1:]
                if character == "\r":
                    self.column = 0
                elif character == "\n":
                    self.row += 1
                elif 0 <= self.row < 90 and 0 <= self.column < 260:
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

    def test_invalid_visual_options_fail_before_terminal_setup(self):
        for arguments in (["--animation-fps", "61"], ["--animation-fps", "-1"],
                          ["--animation-fps"], ["--theme", "unknown"], ["--yolo"]):
            result = subprocess.run([self.binary, "demo", "--interactive", *arguments],
                                    env={}, capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Configuration error", result.stderr)
            self.assertNotIn(b"\x1b", result.stdout + result.stderr)

    def exercise(self, interrupted, visual=False, fps=0):
        with tempfile.TemporaryDirectory(prefix="fluzo-tui-") as directory:
            root = Path(directory)
            (root / ".fluzo").write_text("invalid configuration must not be read")
            (root / "source.rs").write_text("unchanged fixture")
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            arguments = [self.binary, "demo", "--interactive"]
            if visual:
                arguments += ["--dev-menu", "--animation-fps", str(fps), "--ascii"]
            environment = {"HOME": directory, "XDG_CONFIG_HOME": directory, "TERM": "xterm-256color", "NO_COLOR": "1"}
            if visual and fps == 60:
                arguments.remove("--ascii")
                environment.pop("NO_COLOR")
                environment.update(TERM="xterm-ghostty", COLORTERM="truecolor")
            child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave,
                                     cwd=root, env=environment)
            output = bytearray()
            screen = Screen()

            def wait_for(marker, row=None):
                deadline = time.monotonic() + 5
                while marker not in (screen.text() if row is None else "".join(screen.cells[row]).encode()):
                    if time.monotonic() >= deadline:
                        self.fail(f"Missing terminal marker {marker!r}; recent synthetic output={bytes(output[-2000:])!r}")
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        self.assertTrue(data)
                        output.extend(data)
                        screen.feed(data)
                        self.assertLess(len(output), 1024 * 1024)

            try:
                wait_for(b"Ask anything")
                self.assertNotEqual(termios.tcgetattr(slave), before)
                wait_for(b"75%", row=1)
                os.write(master, b"\x04")
                wait_for(b"Reasoning X-High")
                wait_for(b"GTP-6 Astra via Github Copilot")
                wait_for(b"Example data / offline")
                os.write(master, b"\x04")
                wait_for(b"Ask anything")
                os.write(master, b"\x13")
                wait_for(b"Sessions | offline demo")
                os.write(master, b"\x1b")
                wait_for(b"Ask anything")
                os.write(master, b"\x0c")
                wait_for(b"Models | offline demo")
                os.write(master, b"\x1b")
                wait_for(b"Ask anything")
                os.write(master, b"\x07")
                wait_for(b"Help 1/")
                os.write(master, b"\x07")
                wait_for(b"Ask anything")
                os.write(master, b"\x1b[200~draft\n\x1b]52;c;hidden\x07text\x1b[201~")
                wait_for(b"draft")
                os.write(master, b"\x0ajoined")
                wait_for(b"joined", row=20)
                if visual and fps == 60:
                    os.write(master, b"\x1b[13;2u")
                    os.write(master, b"shifted")
                    wait_for(b"shifted", row=20)
                if visual:
                    os.write(master, b"\x10theme\r")
                    wait_for(b"Theme preview")
                    os.write(master, b"\x1b[C")
                    wait_for(b"high-contrast")
                    os.write(master, b"\x1b")
                    wait_for(b"draft", row=17 if fps == 60 else 18)
                    os.write(master, b"\x10developer\r")
                    wait_for(b"Developer menu")
                    os.write(master, b"render_diagnostics\x1b[C")
                    wait_for(b"UI v0 | target", row=23)
                    os.write(master, b"\x1b")
                    wait_for(b"draft", row=17 if fps == 60 else 18)
                    os.write(master, b"\x10developer\r")
                    wait_for(b"Developer menu")
                    os.write(master, b"animation_fps")
                    wait_for(b"CommandLine")
                    os.write(master, b"\x1b[C")
                    wait_for(b"Locked by command line")
                    os.write(master, b"\x0e")
                    wait_for(b"Demo state: Running")
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 16, 60, 0, 0))
                    child.send_signal(signal.SIGWINCH)
                    os.write(master, b"\x7f")
                    wait_for(b"> animation_fp ", row=4)
                    os.write(master, b"\x0e")
                    wait_for(b"Demo state: Waiting")
                    os.write(master, b"\x1b")
                    wait_for(b"Demo state: Pending")
                    wait_for(b"draft", row=9 if fps == 60 else 10)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                    wait_for(b"draft", row=17 if fps == 60 else 18)
                os.write(master, b"\x10")
                wait_for(b"Search commands...")
                wait_for(b"Enter run")
                os.write(master, b"play\r")
                wait_for(b"Synthetic fragment")
                wait_for(b"Working (demo) 0s")
                wait_for(b"esc cancel", row=22)
                if not visual:
                    title_row = next(index for index, row in enumerate(screen.cells) if "Inspect project structure" in "".join(row))
                    os.write(master, f"\x1b[<0;6;{title_row + 1}M".encode())
                    wait_for("▌".encode())
                    wait_for("◇ GTP-6 Astra via Github Copilot in 1m21s".encode())
                    footer_row = next(row for row in screen.cells if "GTP-6 Astra via" in "".join(row))
                    self.assertEqual(footer_row[1:4], [" ", " ", "◇"])
                    self.assertIn("─", footer_row)
                    self.assertNotIn(b"2,480 tokens", screen.text())
                os.write(master, b"\x1b")
                wait_for(b"Preview stopped")
                if not visual:
                    wait_for(b"tab focus editor", row=22)
                    os.write(master, b"\t")
                wait_for(b"tab focus chat", row=22)
                os.write(master, b"\x1b[200~" + b"\nscroll fixture" * 30 + b"\x1b[201~")
                wait_for(b"scroll fixture")
                os.write(master, b"\r")
                wait_for(b"Draft previewed only")
                os.write(master, b"\x1b[<64;6;4M")
                wait_for(b"paused")
                os.write(master, b"\x1b[<65;6;4M")
                wait_for(b"following")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 8, 30, 0, 0))
                wait_for(b"Minimum: 60x16")
                for height, width in [(40, 120), (50, 160), (80, 240), (24, 80)]:
                    screen.cells = [[" " for _ in range(260)] for _ in range(90)]
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
                    wait_for(b"tab focus chat", row=height - 2)
                    wait_for(b"scroll fixture", row=height - 4)
                    self.assertEqual("".join(screen.cells[height - 4][1:5]), "::: ")
                    if width >= 120:
                        if visual and fps == 0:
                            wait_for(b"FLUZO", row=1)
                            self.assertEqual("".join(screen.cells[1][width - 32:width - 27]), "FLUZO")
                        else:
                            self.assertTrue(any(cell in "▀▄█" for row in screen.cells[1:4] for cell in row[width - 32:width - 2]))
                        wait_for(b"Demo session", row=5)
                    else:
                        wait_for(b"FLUZO", row=1)
                if interrupted:
                    child.send_signal(signal.SIGTERM)
                else:
                    os.write(master, b"\x03")
                    wait_for(b"Discard unsaved")
                    wait_for(b"Keep editing")
                    wait_for(b"Enter confirms")
                    os.write(master, b"\r")
                    wait_for(b"scroll fixture", row=20)
                    self.assertIsNone(child.poll())
                    os.write(master, b"\x03")
                    wait_for(b"Discard unsaved")
                    os.write(master, b"\t\r")
                child.wait(timeout=5)
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                self.assertEqual(child.returncode, 0)
                self.assertEqual(termios.tcgetattr(slave), before)
                self.assertIn(b"\x1b[?1006h", output)
                self.assertIn(b"\x1b[?1006l", output)
                self.assertIn(b"\x1b[?1000l", output)
                self.assertIn(b"\x1b[?2004l", output)
                self.assertIn(b"\x1b[?1049l", output)
                self.assertIn(b"\x1b[?25h", output)
                if visual and fps == 60:
                    self.assertIn(b"\x1b[?2026h", output)
                    self.assertIn(b"\x1b[?2026l", output)
                    self.assertIn(b"38;2;", output)
                    self.assertIn(b"\x1b[>1u", output)
                    self.assertIn(b"\x1b[<1u", output)
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

    def test_notification_focus_policy_and_wire_output_without_desktop_access(self):
        for enabled in (False, True):
            with self.subTest(enabled=enabled), tempfile.TemporaryDirectory(prefix="fluzo-notify-") as directory:
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
                before = termios.tcgetattr(slave)
                arguments = [self.binary, "demo", "--interactive", "--dev-menu", "--animation-fps", "0"]
                if enabled:
                    arguments.append("--desktop-notifications")
                child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave, cwd=directory,
                                         env={"HOME": directory, "TERM": "xterm-ghostty", "NO_COLOR": "1"})
                output = bytearray()
                screen = Screen()
                def receive(marker, start=0):
                    deadline = time.monotonic() + 7
                    while marker not in (output[start:] if marker.startswith(b"\x1b") else screen.text()):
                        self.assertLess(time.monotonic(), deadline, repr(output[-1000:]))
                        if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                            data = os.read(master, 65536)
                            self.assertTrue(data)
                            output.extend(data)
                            screen.feed(data)
                            self.assertLess(len(output), 1024 * 1024)
                try:
                    receive(b"Ask anything")
                    os.write(master, b"\x10developer\r")
                    receive(b"Notification test")
                    if not enabled:
                        os.write(master, b"\x1b[O\x14")
                        receive(b"Notifications disabled")
                    else:
                        os.write(master, b"\x1b[I\x14")
                        receive(b"Notification test in 3 seconds")
                        receive(b"suppressed")
                        self.assertNotIn(b"]777;notify;", output)
                        start = len(output)
                        os.write(master, b"\x1b[O\x14")
                        notification = b"\x1b]777;notify;Fluzo notification test;Synthetic completion test. No agent work was executed.\x1b\\"
                        receive(notification, start)
                        self.assertEqual(output.count(notification), 1)
                    os.write(master, b"\x03")
                    receive(b"\x1b[?1004l")
                    child.wait(timeout=5)
                    while select.select([master], [], [], 0)[0]:
                        output.extend(os.read(master, 65536))
                    self.assertEqual(child.returncode, 0)
                    self.assertEqual(termios.tcgetattr(slave), before)
                    self.assertIn(b"\x1b[?1004h", output)
                    self.assertIn(b"\x1b[?1004l", output)
                    self.assertEqual(output.count(b"]777;notify;"), int(enabled))
                    self.assertEqual(list(Path(directory).iterdir()), [])
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=5)
                    os.close(master)
                    os.close(slave)

    def test_natural_playback_completion_notifies_once_but_cancel_does_not(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-completion-") as directory:
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            child = subprocess.Popen([self.binary, "demo", "--interactive", "--desktop-notifications", "--animation-fps", "0"],
                                     stdin=slave, stdout=slave, stderr=slave, cwd=directory,
                                     env={"HOME": directory, "TERM": "xterm-ghostty", "NO_COLOR": "1"})
            output = bytearray()
            screen = Screen()
            def receive(marker, timeout=5):
                deadline = time.monotonic() + timeout
                while marker not in (output if marker.startswith(b"\x1b") else screen.text()):
                    self.assertLess(time.monotonic(), deadline, repr(output[-1000:]))
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        self.assertTrue(data)
                        output.extend(data)
                        screen.feed(data)
                        self.assertLess(len(output), 1024 * 1024)
            try:
                receive(b"Ask anything")
                os.write(master, b"\x1b[O\x10play\r")
                receive(b"Synthetic fragment")
                os.write(master, b"\x1b")
                receive(b"Preview stopped")
                self.assertNotIn(b"]777;notify;", output)
                os.write(master, b"\x10play\r")
                receive(b"\x1b]777;notify;Fluzo is waiting...;Synthetic playback completed. No agent work was executed.\x1b\\", 65)
                os.write(master, b"\x03")
                receive(b"\x1b[?1004l")
                child.wait(timeout=5)
                self.assertEqual(output.count(b"]777;notify;"), 1)
                self.assertEqual(child.returncode, 0)
                self.assertEqual(list(Path(directory).iterdir()), [])
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                os.close(master)
                os.close(slave)

    def test_paste_palette_stream_resize_and_normal_exit_restore_terminal(self):
        self.exercise(False)

    def test_zero_fps_developer_previews_remain_interactive_and_read_only(self):
        self.exercise(False, visual=True)

    def test_active_animation_resize_and_controls_remain_safe(self):
        self.exercise(False, visual=True, fps=60)

    def test_external_termination_restores_terminal(self):
        self.exercise(True)


if __name__ == "__main__":
    unittest.main()
