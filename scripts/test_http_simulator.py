import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def simulator_binary():
    result = subprocess.run(
        ["cargo", "test", "-p", "fluzo-runtime", "--test", "http_simulator",
         "--locked", "--offline", "--no-run", "--message-format=json"],
        cwd=ROOT, check=True, capture_output=True, text=True, timeout=120,
    )
    artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    binaries = [item["executable"] for item in artifacts
                if item.get("reason") == "compiler-artifact"
                and item.get("target", {}).get("name") == "http_simulator"
                and item.get("executable")]
    if len(binaries) != 1:
        raise RuntimeError("Expected exactly one HTTP simulator test executable")
    return Path(binaries[0]).resolve(strict=True)


def isolated_environment(root):
    environment = {"PATH": os.defpath}
    for key, directory in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                           ("XDG_DATA_HOME", "data"), ("TMPDIR", "tmp")]:
        path = root / directory
        path.mkdir()
        environment[key] = str(path)
    return environment


def run_isolated(binary):
    with tempfile.TemporaryDirectory(prefix="fluzo-http-profile-") as temporary:
        root = Path(temporary)
        environment = isolated_environment(root)
        subprocess.run(
            ["unshare", "--user", "--map-root-user", "--net", sys.executable,
             str(Path(__file__).resolve()), "--isolated", str(binary)],
            cwd=root, env=environment, check=True, timeout=60,
        )


def inside_namespace(binary):
    import fcntl

    if socket.if_nameindex() != [(1, "lo")]:
        raise RuntimeError("Deterministic profile requires a private loopback-only namespace")
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as control:
        request = struct.pack("16sH14s", b"lo", 0, b"")
        flags = struct.unpack("16sH14s", fcntl.ioctl(control, 0x8913, request))[1]
        fcntl.ioctl(control, 0x8914, struct.pack("16sH14s", b"lo", flags | 1, b""))
    subprocess.run([str(binary)], check=True, timeout=45)
    print("HTTP simulator: passed; live inference: not_run", flush=True)


class HttpProfileTests(unittest.TestCase):
    def test_environment_excludes_inherited_provider_proxy_and_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.dict(os.environ, {"HTTP_PROXY": "synthetic-proxy", "OPENAI_API_KEY": "synthetic-secret"}):
                environment = isolated_environment(Path(temporary))
            self.assertEqual(set(environment), {"PATH", "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "TMPDIR"})
            self.assertNotIn("synthetic-secret", repr(environment))
            self.assertTrue(all(Path(value).is_dir() for key, value in environment.items() if key != "PATH"))

    def test_namespace_failure_never_falls_back(self):
        failure = subprocess.CalledProcessError(1, "unshare")
        with patch("subprocess.run", side_effect=failure) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                run_isolated(Path("/synthetic-test"))
            self.assertEqual(run.call_count, 1)
            self.assertEqual(run.call_args.args[0][:4], ["unshare", "--user", "--map-root-user", "--net"])
            self.assertEqual(run.call_args.kwargs["timeout"], 60)
            self.assertTrue(run.call_args.kwargs["check"])
            self.assertFalse(Path(run.call_args.kwargs["cwd"]).exists())

    def test_compile_failure_does_not_fetch_or_execute(self):
        with patch("subprocess.run", side_effect=subprocess.CalledProcessError(1, "cargo")) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                simulator_binary()
            self.assertEqual(run.call_count, 1)
            self.assertIn("--offline", run.call_args.args[0])
            self.assertIn("--locked", run.call_args.args[0])

    def test_nonisolated_network_is_rejected_before_execution(self):
        with patch("socket.if_nameindex", return_value=[(1, "lo"), (2, "eth0")]):
            with patch("subprocess.run") as run:
                with self.assertRaisesRegex(RuntimeError, "loopback-only"):
                    inside_namespace(Path("/synthetic-test"))
                run.assert_not_called()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--isolated":
        inside_namespace(Path(sys.argv[2]))
    elif len(sys.argv) == 1:
        run_isolated(simulator_binary())
    else:
        raise SystemExit("Usage: python3 scripts/test_http_simulator.py")
