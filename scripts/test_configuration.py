import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unittest

import test_setup
from test_tui import Screen


class ConfigurationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        test_setup.SetupTests.setUpClass()
        cls.binary = test_setup.SetupTests.binary

    def exercise(self, missing=False, terminate=False):
        with tempfile.TemporaryDirectory(prefix="fluzo-s3-pty-") as directory, socket.socket() as listener:
            root = Path(directory)
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            listener.setblocking(False)
            endpoint = listener.getsockname()[1]
            original = f"schema_version = 1\n[agent]\nmodel = 'coder'\n[capacity_pools.local]\n[models.coder]\nbase_url = 'http://127.0.0.1:{endpoint}/v1'\nmodel = 'fixture'\ncapacity_id = 'fixture'\npool = 'local'\nauth = 'env'\napi_key_env = 'ABSENT_FIXTURE'\n"
            if not missing:
                (root / ".fluzo").write_text(original)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
            before = termios.tcgetattr(slave)
            child = subprocess.Popen([self.binary, "--animation-fps", "0", "--no-dev-menu"], cwd=root,
                env={"HOME": directory, "XDG_CONFIG_HOME": directory, "TERM": "xterm-256color", "NO_COLOR": "1"},
                stdin=slave, stdout=slave, stderr=slave)
            screen = Screen()
            output = bytearray()

            def wait(marker):
                deadline = time.monotonic() + 8
                while marker not in screen.text():
                    self.assertLess(time.monotonic(), deadline, f"Missing {marker!r}: {screen.text()!r}")
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        output.extend(data)
                        self.assertLess(len(output), 2 * 1024 * 1024)
                        screen.feed(data)

            try:
                if missing:
                    wait(b"Welcome to Fluzo")
                    os.write(master, b"\r\x13")
                    wait(b"Review every value")
                    os.write(master, b"\x13")
                    wait(b"Configuration saved offline")
                    original = (root / ".fluzo").read_text()
                    os.write(master, b"\x1bOQ")
                wait(b"Configuration workspace")
                os.write(master, b"\x1b[200~preserved composer\x1b[201~\x10")
                wait(b"Commands / configuration only")
                self.assertNotIn(b"Play synthetic", screen.text())
                os.write(master, b"Theme\r")
                wait(b"tui.theme = default")
                wait(b"ReadOnly")
                os.write(master, b"\x1b[C\x01")
                wait(b"Completed: Applied")
                wait(b"Effective high-contrast")
                self.assertEqual((root / ".fluzo").read_text(), original)
                os.write(master, b"\x13")
                wait(b"ReadOnly")
                os.write(master, b"\x1b")
                wait(b"preserved composer")
                os.write(master, b"\x10Notifications\r")
                wait(b"tui.notifications.desktop_enabled")
                os.write(master, b"\x15tui.notifications.duration_seconds\r\x1512\r\x01")
                wait(b"Completed: Applied")
                wait(b"Effective 12")
                os.write(master, b"\x15tui.animation_fps\r\x1515\r\x01")
                wait(b"CliLocked")
                wait(b"Effective 0")
                os.write(master, b"\x15tui.notifications.duration_seconds\r\x1520\r\x01")
                wait(b"Effective 20")
                wait(b"Completed: Applied")
                os.write(master, b"\x15tui.animation_fps")
                wait(b"draft 15")
                wait(b"Effective 0")
                os.write(master, b"\x10review_pool\r")
                wait(b"Search: capacity_pools.review_pool")
                os.write(master, b"\x16")
                wait(b"Draft validated")
                os.write(master, b"\x1b[3~")
                wait(b"Collection removal staged only")
                os.write(master, b"\x16")
                wait(b"Draft validated")
                os.write(master, b"\x10")
                wait(b"Add pool name")
                os.write(master, b"review_pool\r")
                wait(b"Local draft changed")
                os.write(master, b"\x16")
                wait(b"Draft validated")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 10, 40, 0, 0))
                wait(b"resize required")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                wait(b"FLUZO / Configuration")
                if terminate:
                    child.send_signal(signal.SIGTERM)
                else:
                    os.write(master, b"\x03")
                child.wait(timeout=8)
                self.assertEqual(child.returncode, 0)
                self.assertEqual(termios.tcgetattr(slave), before)
                self.assertEqual((root / ".fluzo").read_text(), original)
                self.assertEqual(sorted(path.name for path in root.iterdir()), [".fluzo"])
                self.assertNotIn(b"\x1b]52;", output)
                self.assertNotIn(b"\x1b]777;", output)
                with self.assertRaises(BlockingIOError):
                    listener.accept()
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=8)
                os.close(master)
                os.close(slave)

    def test_normal_settings_apply_cli_locks_resize_and_preserve_files(self):
        self.exercise()

    def test_setup_to_settings_keeps_create_only_and_request_identity(self):
        self.exercise(missing=True)

    def test_settings_signal_restores_terminal(self):
        self.exercise(terminate=True)
