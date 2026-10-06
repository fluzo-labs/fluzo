import fcntl
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

import test_setup
from test_tui import Screen


class ModelWizardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        test_setup.SetupTests.setUpClass()
        cls.binary = test_setup.SetupTests.binary

    def exercise(self, existing=False, authorization=False, multiple=False):
        with tempfile.TemporaryDirectory(prefix="fluzo-model-wizard-") as directory, socket.socket() as listener:
            root = Path(directory)
            baseline = b"schema_version = 1\n"
            if existing:
                (root / ".fluzo").write_bytes(baseline)
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            listener.settimeout(5)
            endpoint = f"http://127.0.0.1:{listener.getsockname()[1]}"
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
            before = termios.tcgetattr(slave)
            environment = {"HOME": directory, "XDG_CONFIG_HOME": directory, "TERM": "xterm-256color", "NO_COLOR": "1"}
            if authorization:
                environment["FIXTURE_AUTH"] = "Bearer synthetic-fixture-token"
            child = subprocess.Popen([self.binary], cwd=root,
                env=environment,
                stdin=slave, stdout=slave, stderr=slave)
            screen = Screen()
            output = bytearray()

            def wait(marker):
                deadline = time.monotonic() + 8
                while marker not in screen.text():
                    self.assertLess(time.monotonic(), deadline, screen.text())
                    if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                        data = os.read(master, 65536)
                        output.extend(data)
                        self.assertLess(len(output), 2 * 1024 * 1024)
                        screen.feed(data)

            try:
                wait(b"Configuration workspace" if existing else b"Welcome to Fluzo")
                self.assertEqual(select.select([listener], [], [], 0)[0], [])
                os.write(master, b"\x1bOR")
                wait(b"1 / Model location")
                os.write(master, b"\r")
                wait(b"2 / Local provider")
                os.write(master, b"\r")
                wait(b"3 / Authorization")
                if authorization:
                    os.write(master, b"\x1b[B\r")
                    wait(b"environment variable")
                    os.write(master, b"FIXTURE_AUTH\t")
                else:
                    os.write(master, b"\r")
                wait(b"Server URL")
                os.write(master, b"\x1b[200~" + endpoint.encode() + b"\x1b[201~")
                self.assertEqual(select.select([listener], [], [], 0)[0], [])
                self.assertEqual((root / ".fluzo").exists(), existing)
                os.write(master, b"\t")
                connection, _ = listener.accept()
                with connection:
                    connection.settimeout(5)
                    request = bytearray()
                    while not request.endswith(b"\r\n\r\n"):
                        data = connection.recv(4096)
                        self.assertTrue(data)
                        request.extend(data)
                        self.assertLess(len(request), 8192)
                    self.assertTrue(request.startswith(b"GET /v1/models HTTP/1.1\r\n"))
                    if authorization:
                        self.assertIn(b"authorization: Bearer synthetic-fixture-token", request)
                    else:
                        self.assertNotIn(b"authorization", request.lower())
                    body = b'{"data":[{"id":"fixture-coder","max_context_length":65536,"max_output_tokens":8192,"slots":2},{"id":"fixture-second"}]}'
                    connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: " + str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body)
                wait(b"fixture-coder")
                wait(b"65536")
                wait(b"8192")
                if multiple:
                    os.write(master, b" \x1b[B ")
                os.write(master, b"\r")
                wait(b"Alias prefix:")
                os.write(master, b"reviewmodel\r")
                wait(b"Reported limits")
                self.assertEqual((root / ".fluzo").exists(), existing)
                os.write(master, b"\r")
                wait(b"Discovered model staged" if existing else b"Models ready to add:")
                if existing:
                    wait(b"Save: available")
                    os.write(master, b"\x16")
                    wait(b"Draft validated")
                    os.write(master, b"\x13")
                    wait(b"Completed: Saved")
                    os.write(master, b"\x03")
                    child.wait(timeout=8)
                    self.assertEqual(child.returncode, 0)
                    self.assertEqual(termios.tcgetattr(slave), before)
                    document = tomllib.loads((root / ".fluzo").read_text())
                    models = document.get("models", {})
                    pools = document.get("capacity_pools", {})
                    expected = (
                        ["reviewmodel"]
                        if not multiple
                        else ["reviewmodel_1", "reviewmodel_2"]
                    )
                    for alias in expected:
                        self.assertIn(alias, models)
                        self.assertIn(alias, pools)
                        self.assertEqual(models[alias]["base_url"], endpoint + "/v1")
                    self.assertEqual(select.select([listener], [], [], 0)[0], [])
                    return
                os.write(master, b"\r")
                wait(b"Create configuration?")
                self.assertEqual((root / ".fluzo").exists(), existing)
                os.write(master, b"\r")
                wait(b"Your repository configuration is ready.")
                os.write(master, b"\x03")
                child.wait(timeout=8)
                self.assertEqual(child.returncode, 0)
                self.assertEqual(termios.tcgetattr(slave), before)
                settings = tomllib.loads((root / ".fluzo").read_text())
                alias = "reviewmodel_1" if multiple else "reviewmodel"
                self.assertEqual(settings["models"][alias]["model"], "fixture-coder")
                self.assertEqual(settings["models"][alias]["base_url"], endpoint + "/v1")
                self.assertEqual(settings["agent"]["model"], alias)
                self.assertEqual(settings["models"][alias]["pool"], alias)
                if multiple:
                    self.assertEqual(settings["models"]["reviewmodel_2"]["model"], "fixture-second")
                if authorization:
                    self.assertEqual(settings["models"][alias]["api_key_env"], "FIXTURE_AUTH")
                    self.assertNotIn(b"synthetic-fixture-token", (root / ".fluzo").read_bytes())
                    self.assertNotIn(b"synthetic-fixture-token", output)
                self.assertEqual(select.select([listener], [], [], 0)[0], [])
                self.assertNotIn(b"\x1b]52;", output)
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=8)
                os.close(master)
                os.close(slave)

    def test_explicit_discovery_selection_and_setup_creation(self):
        self.exercise()

    def test_existing_configuration_stages_and_preserves_file(self):
        self.exercise(existing=True, multiple=True)

    def test_authorized_multi_model_selection_saves_references_only(self):
        self.exercise(authorization=True, multiple=True)
