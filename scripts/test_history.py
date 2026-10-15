"""CLI tests for `fluzo history list` and `fluzo history purge`.

These drive the real binary. Headless cases use pipes so the process sees
no terminal, which is exactly the path that must return a structured
`confirmation_required` instead of waiting for input. The interactive
confirmation path uses a PTY.

The drive and schema are created by Fluzo itself (a `purge --all --yes`
on an empty workspace opens the store), and rows are then inserted with
the stdlib sqlite3 driver. That keeps the test from restating the schema:
it seeds whatever the binary actually created.
"""

import hashlib
import json
import os
from pathlib import Path
import pty
import select
import sqlite3
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


class HistoryCliTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(
            ["cargo", "build", "-p", "fluzo-cli", "--locked", "--offline",
             "--message-format=json"],
            cwd=ROOT, capture_output=True, text=True, check=True, timeout=300,
        )
        artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        cls.binary = next(
            item["executable"]
            for item in artifacts
            if item.get("executable") and item.get("target", {}).get("name") == "fluzo"
        )

    # ---- helpers -------------------------------------------------------

    def run_cli(self, root, *arguments, stdin=subprocess.DEVNULL, input_data=None):
        return subprocess.run(
            [self.binary, *arguments],
            cwd=root,
            env={"HOME": str(root), "TERM": "xterm-256color"},
            capture_output=True,
            timeout=20,
            stdin=stdin,
            input=input_data,
        )

    def make_workspace(self, directory):
        root = Path(directory)
        (root / ".fluzo").write_text("schema_version = 1\n")
        (root / "keep-me.txt").write_text("user work\n")
        return root

    def create_drive(self, root):
        """Opens the store through the CLI so Fluzo builds its own schema."""
        result = self.run_cli(root, "history", "purge", "--all", "--yes")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((root / ".fluzodrive" / "state.sqlite3").exists())

    def seed(self, root, sessions):
        """sessions: list of (session_id, artifact_id, content_bytes)."""
        database = root / ".fluzodrive" / "state.sqlite3"
        artifacts = root / ".fluzodrive" / "artifacts"
        artifacts.mkdir(exist_ok=True)
        connection = sqlite3.connect(database)
        for session_id, artifact_id, content in sessions:
            connection.execute(
                "INSERT INTO state (id, kind, payload, revision, updated_at) VALUES (?, 'session', 'created', 1, 0)",
                (session_id,),
            )
            connection.execute(
                "INSERT INTO event (state_id, kind, payload) VALUES (?, 'turn', 'first event')",
                (session_id,),
            )
            if artifact_id is not None:
                connection.execute(
                    "INSERT INTO artifact (id, state_id, kind, provenance, stored_bytes, content_hash, capture_flags)"
                    " VALUES (?, ?, 'diff', 'fixture', ?, ?, '')",
                    (artifact_id, session_id, len(content), hashlib.sha256(content).hexdigest()),
                )
                (artifacts / artifact_id).write_bytes(content)
        connection.commit()
        connection.close()

    def attach(self, root, session_id):
        connection = sqlite3.connect(root / ".fluzodrive" / "state.sqlite3")
        connection.execute(
            "INSERT INTO session_attachment (state_id, attached_at) VALUES (?, 1)", (session_id,)
        )
        connection.commit()
        connection.close()

    def code(self, result):
        return json.loads(result.stderr.decode())["code"]

    # ---- listing -------------------------------------------------------

    def test_list_on_empty_workspace_creates_nothing(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-empty-") as directory:
            root = self.make_workspace(directory)
            result = self.run_cli(root, "history", "list")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(b"No sessions to purge.", result.stdout)
            self.assertFalse((root / ".fluzodrive").exists())
            self.assertEqual(result.stderr, b"")

    def test_list_reports_seeded_sessions_with_sizes(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-list-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"x" * 4096), ("beta", None, b"")])
            result = self.run_cli(root, "history", "list")
            self.assertEqual(result.returncode, 0, result.stderr)
            output = result.stdout.decode()
            self.assertIn("alpha", output)
            self.assertIn("beta", output)
            self.assertIn("4.0 KiB", output)
            self.assertIn("Total: 2 sessions", output)

    # ---- argument handling --------------------------------------------

    def test_argument_validation_is_structured(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-args-") as directory:
            root = self.make_workspace(directory)
            cases = [
                (("history",), "invalid_arguments"),
                (("history", "bogus"), "invalid_arguments"),
                (("history", "purge"), "invalid_arguments"),
                (("history", "purge", "--session"), "invalid_arguments"),
                (("history", "purge", "--session", ""), "invalid_arguments"),
                (("history", "purge", "--all", "--session", "alpha"), "invalid_arguments"),
                (("history", "purge", "--nope"), "invalid_arguments"),
                (("history", "list", "--all"), "invalid_arguments"),
                (("history", "list", "--yes"), "invalid_arguments"),
            ]
            for arguments, expected in cases:
                result = self.run_cli(root, *arguments)
                self.assertNotEqual(result.returncode, 0, arguments)
                self.assertEqual(self.code(result), expected, (arguments, result.stderr))
            # Nothing was created by any rejected invocation.
            self.assertFalse((root / ".fluzodrive").exists())

    # ---- confirmation --------------------------------------------------

    def test_headless_purge_without_yes_returns_immediately(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-headless-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            start = time.monotonic()
            result = self.run_cli(root, "history", "purge", "--session", "alpha")
            # Returns promptly rather than blocking on input.
            self.assertLess(time.monotonic() - start, 5)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.code(result), "confirmation_required")
            # Nothing deleted.
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())
            self.assertIn("alpha", self.run_cli(root, "history", "list").stdout.decode())

    def test_all_requires_yes(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-all-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            result = self.run_cli(root, "history", "purge", "--all")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.code(result), "confirmation_required")
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    def test_dry_run_previews_and_deletes_nothing(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-dry-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"y" * 2048)])
            result = self.run_cli(root, "history", "purge", "--session", "alpha", "--dry-run")
            self.assertEqual(result.returncode, 0, result.stderr)
            output = result.stdout.decode()
            self.assertIn("Dry run: nothing was deleted.", output)
            self.assertIn("alpha", output)
            self.assertIn("remove   alpha-art", output)
            self.assertIn("Total release: 2.0 KiB", output)
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    def test_confirmed_purge_removes_exclusive_and_preserves_shared(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-purge-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            shared = b"content two sessions share"
            self.seed(root, [
                ("alpha", "alpha-art", shared),
                ("beta", "beta-art", shared),
                ("gamma", "gamma-art", b"unique to gamma"),
            ])
            result = self.run_cli(root, "history", "purge", "--session", "alpha", "--yes")
            self.assertEqual(result.returncode, 0, result.stderr)
            output = result.stdout.decode()
            self.assertIn("preserve alpha-art", output)
            self.assertIn("still held by beta", output)
            artifacts = root / ".fluzodrive" / "artifacts"
            # alpha's shared artifact survives; gamma-style exclusives would go.
            self.assertTrue((artifacts / "alpha-art").exists())
            self.assertTrue((artifacts / "beta-art").exists())
            listing = self.run_cli(root, "history", "list").stdout.decode()
            self.assertNotIn("alpha", listing)
            self.assertIn("beta", listing)

            # Purging gamma removes its exclusive file.
            result = self.run_cli(root, "history", "purge", "--session", "gamma", "--yes")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("remove   gamma-art", result.stdout.decode())
            self.assertFalse((artifacts / "gamma-art").exists())

    def test_attached_session_is_refused_and_named(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-attached-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            self.attach(root, "alpha")
            result = self.run_cli(root, "history", "purge", "--session", "alpha", "--yes")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.code(result), "session_attached")
            # The refusal names the blocking session.
            self.assertIn("alpha", result.stderr.decode())
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())
            self.assertIn("(attached)", self.run_cli(root, "history", "list").stdout.decode())

    def test_unknown_session_is_refused(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-unknown-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            result = self.run_cli(root, "history", "purge", "--session", "ghost", "--yes")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.code(result), "no_such_session")
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    def test_purge_never_touches_configuration_or_user_files(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-safe-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            configuration = (root / ".fluzo").read_bytes()
            result = self.run_cli(root, "history", "purge", "--all", "--yes")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / ".fluzo").read_bytes(), configuration)
            self.assertEqual((root / "keep-me.txt").read_bytes(), b"user work\n")
            self.assertFalse((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    # ---- interactive confirmation (PTY) --------------------------------

    def run_pty(self, root, answers, arguments):
        master, slave = pty.openpty()
        child = subprocess.Popen(
            [self.binary, *arguments],
            cwd=root,
            env={"HOME": str(root), "TERM": "xterm-256color"},
            stdin=slave,
            stdout=slave,
            stderr=slave,
        )
        os.close(slave)
        output = bytearray()

        def wait_for(marker, timeout=15):
            deadline = time.monotonic() + timeout
            while marker not in output:
                if time.monotonic() >= deadline:
                    self.fail(f"Missing {marker!r}: {bytes(output)!r}")
                if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                    chunk = os.read(master, 65536)
                    if not chunk:
                        break
                    output.extend(chunk)
            self.assertIn(marker, bytes(output))

        try:
            wait_for(b"[y/N]")
            for answer in answers:
                os.write(master, answer)
                time.sleep(0.2)
            # Keep draining until the child closes its end, so the text it
            # prints after answering is captured rather than lost.
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.2)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output.extend(chunk)
                elif child.poll() is not None:
                    break
            return child, bytes(output)
        finally:
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                child.kill()
                self.fail("history prompt did not exit after answering")
            os.close(master)

    def test_interactive_decline_deletes_nothing(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-pty-no-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            child, output = self.run_pty(
                root, [b"n\n"], ("history", "purge", "--session", "alpha")
            )
            self.assertEqual(child.returncode, 0)
            self.assertIn(b"Purge cancelled; nothing deleted.", output)
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    def test_interactive_accept_purges(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-pty-yes-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"z" * 1024)])
            child, output = self.run_pty(
                root, [b"yes\n"], ("history", "purge", "--session", "alpha")
            )
            self.assertEqual(child.returncode, 0)
            self.assertIn(b"Purged.", output)
            self.assertIn(b"Released 1.0 KiB", output)
            self.assertFalse((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())

    def test_interactive_empty_answer_declines(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-history-pty-blank-") as directory:
            root = self.make_workspace(directory)
            self.create_drive(root)
            self.seed(root, [("alpha", "alpha-art", b"payload")])
            child, output = self.run_pty(
                root, [b"\n"], ("history", "purge", "--session", "alpha")
            )
            self.assertEqual(child.returncode, 0)
            self.assertIn(b"Purge cancelled; nothing deleted.", output)
            self.assertTrue((root / ".fluzodrive" / "artifacts" / "alpha-art").exists())


if __name__ == "__main__":
    unittest.main()
