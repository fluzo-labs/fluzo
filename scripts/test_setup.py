import fcntl
import json
import os
from pathlib import Path
import pty
import select
import socket
import struct
import subprocess
import tempfile
import termios
import time
import tomllib
import unittest

from test_tui import Screen

ROOT = Path(__file__).resolve().parents[1]


class SetupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(
            ["cargo", "build", "-p", "fluzo-cli", "--locked", "--offline", "--message-format=json"],
            cwd=ROOT, capture_output=True, text=True, check=True, timeout=120,
        )
        artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        cls.binary = next(item["executable"] for item in artifacts if item.get("executable") and item.get("target", {}).get("name") == "fluzo")

    def test_headless_discovery_and_explicit_selection_preserve_files(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-setup-") as directory:
            root = Path(directory)
            for content, expected in [(None, "configuration_required"), (b"schema_version = 1\n", "runtime_unavailable"), (b"broken = [", "configuration_invalid"), (b"\xff", "configuration_invalid")]:
                target = root / "selected.toml"
                if content is not None:
                    target.write_bytes(content)
                result = subprocess.run([self.binary, "--config", "selected.toml"], cwd=root, env={}, capture_output=True, timeout=8)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, b"")
                self.assertEqual(json.loads(result.stderr)["code"], expected)
                self.assertNotIn(b"\x1b", result.stderr)
                self.assertFalse((root / ".fluzo").exists())
                if content is not None:
                    self.assertEqual(target.read_bytes(), content)
            result = subprocess.run([self.binary, "--config", "../outside"], cwd=root, env={}, capture_output=True, timeout=8)
            self.assertEqual(json.loads(result.stderr)["code"], "configuration_unavailable")

    def test_offline_provider_is_not_contacted_or_resolved(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-setup-offline-") as directory, socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            listener.setblocking(False)
            port = listener.getsockname()[1]
            content = f"schema_version = 1\n[agent]\nmodel = 'coder'\n[capacity_pools.local]\n[models.coder]\nbase_url = 'http://127.0.0.1:{port}/v1'\nmodel = 'fixture'\ncapacity_id = 'fixture'\npool = 'local'\nauth = 'env'\napi_key_env = 'ABSENT_FIXTURE_CREDENTIAL'\n"
            target = Path(directory) / ".fluzo"
            target.write_text(content)
            result = subprocess.run([self.binary], cwd=directory, env={}, capture_output=True, timeout=8)
            self.assertEqual(json.loads(result.stderr)["code"], "runtime_unavailable")
            with self.assertRaises(BlockingIOError):
                listener.accept()
            self.assertEqual(target.read_text(), content)
            result = subprocess.run([self.binary, "init"], cwd=directory, env={}, capture_output=True, timeout=8)
            self.assertEqual(json.loads(result.stderr)["code"], "replacement_unavailable")
            self.assertEqual(target.read_text(), content)

    def test_inaccessible_and_oversized_files_remain_untouched(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-setup-invalid-") as directory:
            target = Path(directory) / ".fluzo"
            target.mkdir()
            result = subprocess.run([self.binary], cwd=directory, env={}, capture_output=True, timeout=8)
            self.assertEqual(json.loads(result.stderr)["code"], "configuration_inaccessible")
            self.assertTrue(target.is_dir())
            target.rmdir()
            original = b"x" * (1024 * 1024 + 1)
            target.write_bytes(original)
            result = subprocess.run([self.binary, "init"], cwd=directory, env={}, capture_output=True, timeout=8)
            self.assertEqual(json.loads(result.stderr)["code"], "configuration_invalid")
            self.assertEqual(target.read_bytes(), original)

    def exercise(self, action):
        with tempfile.TemporaryDirectory(prefix="fluzo-setup-pty-") as directory:
            root = Path(directory)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            original = b"schema_version = 1\n# external\n"
            if action == "valid":
                (root / ".fluzo").write_bytes(original)
            child = subprocess.Popen([self.binary, "--animation-fps", "0"], cwd=root,
                env={"HOME": directory, "XDG_CONFIG_HOME": directory, "TERM": "xterm-256color", "NO_COLOR": "1"},
                stdin=slave, stdout=slave, stderr=slave)
            screen = Screen()
            output = bytearray()

            def wait_for(marker):
                deadline = time.monotonic() + 8
                while marker not in screen.text():
                    if time.monotonic() >= deadline:
                        self.fail(f"Missing {marker!r}: {screen.text()!r}")
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        output.extend(data)
                        self.assertLess(len(output), 1024 * 1024)
                        screen.feed(data)

            try:
                if action == "valid":
                    wait_for(b"Setup skipped")
                    os.write(master, b"\x1b")
                else:
                    wait_for(b"Welcome to Fluzo")
                    self.assertFalse((root / ".fluzo").exists())
                    os.write(master, b"\r")
                    wait_for(b"models.coder.base_url")
                    if action == "resize":
                        os.write(master, b"\r")
                        wait_for(b"Enter a value")
                        os.write(master, b"\x1b[200~synthetic\x1b[201~")
                        wait_for(b"private value entered")
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 10, 40, 0, 0))
                        wait_for(b"Resize or")
                        wait_for(b"Esc to cancel.")
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                        wait_for(b"private value entered")
                        os.write(master, b"\x03")
                    elif action == "invalid":
                        os.write(master, b"\r\x1b[200~not-an-endpoint\x1b[201~\r\x13")
                        wait_for(b"Validation")
                        self.assertFalse((root / ".fluzo").exists())
                        os.write(master, b"\x03")
                    elif action == "cancel":
                        os.write(master, b"\r\x1b[200~synthetic\ntext\x1b[201~\x1b")
                        wait_for(b"private value entered")
                        os.write(master, b"\x03")
                    elif action == "signal":
                        child.terminate()
                    else:
                        if action == "review":
                            os.write(master, b"\t" * 8 + b"\r\x153\r")
                            wait_for(b"> capacity_pools.local.max_in_flight = 3")
                        os.write(master, b"\x13")
                        wait_for(b"Review every value")
                        if action == "review":
                            os.write(master, b"\x1b[6~")
                            wait_for(b"context.compaction_threshold = 0.8")
                            wait_for(b"context.safety_reserve_fraction = 0.05")
                            os.write(master, b"\x1b")
                            wait_for(b"Welcome to Fluzo")
                            self.assertIn(b"Target:", screen.text())
                            self.assertIn(b"> capacity_pools.local.max_in_flight = 3", screen.text())
                            self.assertFalse((root / ".fluzo").exists())
                            os.write(master, b"\x13")
                            wait_for(b"Review every value")
                        self.assertFalse((root / ".fluzo").exists())
                        if action == "conflict":
                            (root / ".fluzo").write_bytes(original)
                        os.write(master, b"\x13")
                        wait_for(b"Conflict" if action == "conflict" else b"Configuration saved offline")
                        os.write(master, b"\x1b")
                child.wait(timeout=8)
                self.assertEqual(termios.tcgetattr(slave), before)
                self.assertNotIn(b"\x1b]777;", output)
                self.assertNotIn(b"\x1b]52;", output)
                if action in ("save", "review"):
                    settings = tomllib.loads((root / ".fluzo").read_text())
                    if action == "review":
                        self.assertEqual(settings["capacity_pools"]["local"]["max_in_flight"], 3)
                        self.assertEqual(settings["capacity_pools"]["local"]["foreground_reserved_slots"], 1)
                        self.assertEqual(settings["models"], {})
                        self.assertEqual(settings["context"]["compaction_threshold"], 0.8)
                        self.assertEqual(settings["context"]["safety_reserve_fraction"], 0.05)
                    self.assertEqual(settings["tui"]["animation_fps"], 60)
                    self.assertFalse(settings["telemetry"]["export_enabled"])
                    self.assertEqual(sorted(path.name for path in root.iterdir()), [".fluzo"])
                    self.assertEqual(child.returncode, 0)
                elif action in ("conflict", "valid"):
                    self.assertEqual((root / ".fluzo").read_bytes(), original)
                else:
                    self.assertEqual(list(root.iterdir()), [])
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                os.close(master)
                os.close(slave)

    def test_review_regressions_preserve_pool_summary_and_return_navigation(self):
        self.exercise("review")

    def test_setup_save_requires_two_explicit_steps(self):
        self.exercise("save")

    def test_setup_conflict_after_confirmation_does_not_clobber(self):
        self.exercise("conflict")

    def test_setup_paste_and_cancel_write_nothing(self):
        self.exercise("cancel")

    def test_valid_configuration_skips_setup(self):
        self.exercise("valid")

    def test_setup_signal_restores_terminal(self):
        self.exercise("signal")

    def test_setup_resize_preserves_draft(self):
        self.exercise("resize")

    def test_setup_invalid_model_does_not_save(self):
        self.exercise("invalid")
