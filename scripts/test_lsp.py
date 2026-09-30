import io
from pathlib import Path
import queue
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from check_lsp import Client, prepare_dependencies, test_workspace


class RequestTests(unittest.TestCase):
    def client(self, messages):
        client = Client.__new__(Client)
        client.process = SimpleNamespace(stdin=io.BytesIO())
        client.messages = Mock()
        client.messages.get.side_effect = messages
        client.notifications = []
        client.sequence = 0
        return client

    def test_notifications_do_not_extend_deadline(self):
        client = self.client([
            {"method": "$/progress", "params": {}},
            {"id": 1, "result": "late"},
        ])
        with patch("check_lsp.time.monotonic", side_effect=[0, 1, 2, 61, 62]):
            with self.assertRaises(TimeoutError):
                client.request("test", {})
        self.assertEqual(client.messages.get.call_count, 1)

    def test_response_received_after_deadline_is_rejected(self):
        client = self.client([{"id": 1, "result": "late"}])
        with patch("check_lsp.time.monotonic", side_effect=[0, 59, 61]):
            with self.assertRaises(TimeoutError):
                client.request("test", {})

    def test_outer_deadline_is_not_reset(self):
        client = self.client([{"id": 1, "result": "ok"}])
        with patch("check_lsp.time.monotonic", return_value=10):
            self.assertEqual(client.request("test", {}, deadline=12), "ok")
        client.messages.get.assert_called_once_with(timeout=2)

    def test_expired_deadline_prevents_dispatch(self):
        client = self.client([])
        with patch("check_lsp.time.monotonic", return_value=12):
            with self.assertRaises(TimeoutError):
                client.request("test", {}, deadline=12)
        self.assertEqual(client.process.stdin.getvalue(), b"")
        client.messages.get.assert_not_called()

    def test_empty_queue_reports_timeout(self):
        client = self.client([queue.Empty()])
        with patch("check_lsp.time.monotonic", return_value=0):
            with self.assertRaisesRegex(TimeoutError, "test"):
                client.request("test", {})

    def test_successful_response_is_preserved(self):
        client = self.client([{"id": 1, "result": "ok"}])
        with patch("check_lsp.time.monotonic", return_value=0):
            self.assertEqual(client.request("test", {}), "ok")

    def test_indexing_invalidation_retries_with_new_id_and_same_deadline(self):
        for method in ["textDocument/definition", "textDocument/references", "textDocument/rename", "textDocument/documentSymbol"]:
            with self.subTest(method=method):
                client = self.client([
                    {"id": 1, "error": {"code": -32801, "message": "content modified"}},
                    {"id": 2, "result": "ok"},
                ])
                with patch("check_lsp.time.monotonic", return_value=10):
                    self.assertEqual(client.request_stable(method, {}, deadline=12), "ok")
                self.assertEqual(client.sequence, 2)
                self.assertEqual([call.kwargs["timeout"] for call in client.messages.get.call_args_list], [2, 2])

    def test_indexing_retries_stop_at_attempt_limit(self):
        client = self.client([
            {"id": identity, "error": {"code": -32801, "message": "content modified"}}
            for identity in range(1, 9)
        ])
        with patch("check_lsp.time.monotonic", return_value=0):
            with self.assertRaises(RuntimeError):
                client.request_stable("textDocument/references", {})
        self.assertEqual(client.sequence, 8)

    def test_indexing_retries_do_not_extend_outer_deadline(self):
        client = self.client([
            {"id": 1, "error": {"code": -32801, "message": "content modified"}},
        ])
        with patch("check_lsp.time.monotonic", side_effect=[0, 0, 0, 1, 2]):
            with self.assertRaises(TimeoutError):
                client.request_stable("textDocument/references", {}, deadline=2)
        self.assertEqual(client.sequence, 1)

    def test_other_errors_and_lifecycle_requests_are_not_retried(self):
        for method, code in [("textDocument/references", -32603), ("initialize", -32801), ("shutdown", -32801)]:
            with self.subTest(method=method, code=code):
                client = self.client([{"id": 1, "error": {"code": code, "message": "fixture"}}])
                with self.assertRaises(RuntimeError):
                    client.request_stable(method, {})
                self.assertEqual(client.sequence, 1)

    def test_server_errors_are_preserved(self):
        client = self.client([{"id": 1, "error": {"code": -32801, "message": "content modified"}}])
        with self.assertRaises(RuntimeError):
            client.request("test", {})


class WorkspaceTests(unittest.TestCase):
    def test_dependencies_are_prepared_offline_without_copying_user_config(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-lsp-dependencies-") as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            workspace.mkdir()
            configuration = '[source.crates-io]\nreplace-with = "vendored-sources"\n'
            with patch("check_lsp.subprocess.run", return_value=SimpleNamespace(stdout=configuration)) as run:
                prepare_dependencies(workspace, root)
            self.assertEqual((workspace / ".cargo/config.toml").read_text(), configuration)
            self.assertEqual(run.call_args.args[0], ["cargo", "vendor", "--locked", "--offline", str(root / "vendor")])
            self.assertEqual(run.call_args.kwargs["timeout"], 120)
            self.assertTrue(run.call_args.kwargs["check"])
            self.assertEqual(sorted(path.name for path in (workspace / ".cargo").iterdir()), ["config.toml"])

    def test_dependency_preparation_failure_does_not_fall_back_to_network(self):
        with patch("check_lsp.subprocess.run", side_effect=subprocess.CalledProcessError(1, "cargo")) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                prepare_dependencies(Path("fixture"), Path("root"))
            run.assert_called_once()

    def test_cargo_is_bounded_and_failures_are_not_hidden(self):
        failures = [
            subprocess.TimeoutExpired("cargo", 120, output=b"partial output"),
            subprocess.CalledProcessError(1, "cargo", stderr=b"assertion failed"),
        ]
        for failure in failures:
            with self.subTest(failure=failure):
                with patch("check_lsp.subprocess.run", side_effect=failure) as run:
                    with self.assertRaises(type(failure)) as caught:
                        test_workspace("fixture", {"CARGO_NET_OFFLINE": "true"})
                    self.assertIs(caught.exception, failure)
                    self.assertEqual(run.call_args.kwargs["timeout"], 120)
                    self.assertTrue(run.call_args.kwargs["capture_output"])
                    self.assertTrue(run.call_args.kwargs["check"])


class ReaderTests(unittest.TestCase):
    def test_eof_is_delivered_to_waiting_requests(self):
        for payload in [b"", b"Content-Length: 20\r\n\r\n{}"]:
            with self.subTest(payload=payload):
                client = Client(SimpleNamespace(stdout=io.BytesIO(payload), stdin=io.BytesIO()))
                client.reader.join(timeout=1)
                self.assertFalse(client.reader.is_alive())
                self.assertFalse(client.messages.empty(), "EOF must wake the receiver")
                self.assertIsInstance(client.messages.queue[0], EOFError)
                with self.assertRaises(EOFError):
                    client.request("test", {})


if __name__ == "__main__":
    unittest.main()
