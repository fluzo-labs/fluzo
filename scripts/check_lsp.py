import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[1]


class Client:
    def __init__(self, process):
        self.process = process
        self.messages = queue.Queue()
        self.notifications = []
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        raise EOFError("LSP server closed stdout")
                    if line == b"\r\n":
                        break
                    key, value = line.decode().split(":", 1)
                    headers[key.lower()] = value.strip()
                length = int(headers["content-length"])
                payload = self.process.stdout.read(length)
                if len(payload) != length:
                    raise EOFError("LSP server closed stdout during a message")
                self.messages.put(json.loads(payload))
        except Exception as error:
            self.messages.put(error)

    def send(self, message):
        payload = json.dumps({"jsonrpc": "2.0", **message}).encode()
        self.process.stdin.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
        self.process.stdin.flush()

    def notify(self, method, params):
        self.send({"method": method, "params": params})

    def request(self, method, params, *, deadline=None):
        now = time.monotonic()
        deadline = min(now + 60, deadline) if deadline is not None else now + 60
        if now >= deadline:
            raise TimeoutError(f"LSP request timed out: {method}")
        self.sequence += 1
        identity = self.sequence
        self.send({"id": identity, "method": method, "params": params})
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"LSP request timed out: {method}")
            try:
                message = self.messages.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError(f"LSP request timed out: {method}") from error
            if time.monotonic() >= deadline:
                raise TimeoutError(f"LSP request timed out: {method}")
            if isinstance(message, Exception):
                raise message
            if message.get("id") == identity and "method" not in message:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message.get("result")
            if "id" in message and "method" in message:
                self.send({"id": message["id"], "result": None})
            else:
                self.notifications.append(message)


def position(text, needle):
    offset = text.index(needle)
    prefix = text[:offset]
    return {"line": prefix.count("\n"), "character": len(prefix.rsplit("\n", 1)[-1])}


def offset(text, location):
    lines = text.splitlines(keepends=True)
    return sum(len(line) for line in lines[:location["line"]]) + location["character"]


def test_workspace(workspace, environment):
    subprocess.run(["cargo", "test", "--workspace", "--locked", "--offline"],
                   cwd=workspace, env=environment, check=True, capture_output=True, timeout=120)


def main():
    rustup_home = subprocess.check_output(["rustup", "show", "home"], text=True, timeout=30).strip()
    with tempfile.TemporaryDirectory(prefix="fluzo-lsp-") as temporary:
        root = Path(temporary)
        workspace = root / "workspace"
        workspace.mkdir()
        for name in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]:
            shutil.copy2(ROOT / name, workspace / name)
        shutil.copytree(ROOT / "crates", workspace / "crates")
        home = root / "home"
        home.mkdir()
        environment = {
            "PATH": os.environ["PATH"],
            "HOME": str(home),
            "RUSTUP_HOME": rustup_home,
            "CARGO_HOME": str(home / ".cargo"),
            "CARGO_NET_OFFLINE": "true",
        }
        with (root / "server.log").open("wb") as errors:
            process = subprocess.Popen(
                ["rustup", "run", "1.98.0", "rust-analyzer"],
                cwd=workspace, env=environment, stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=errors,
            )
            client = Client(process)
            try:
                initialized = client.request("initialize", {
                    "processId": os.getpid(),
                    "rootUri": workspace.as_uri(),
                    "capabilities": {},
                    "workspaceFolders": [{"uri": workspace.as_uri(), "name": "fixture"}],
                    "initializationOptions": {
                        "cargo": {"buildScripts": {"enable": False}},
                        "procMacro": {"enable": False},
                    },
                })
                assert initialized["capabilities"]["definitionProvider"]
                assert initialized["capabilities"]["referencesProvider"]
                assert initialized["capabilities"]["renameProvider"]
                client.notify("initialized", {})
                source = workspace / "crates/fluzo-cli/src/main.rs"
                text = source.read_text()
                client.notify("textDocument/didOpen", {"textDocument": {
                    "uri": source.as_uri(), "languageId": "rust", "version": 1, "text": text,
                }})
                params = {"textDocument": {"uri": source.as_uri()},
                          "position": position(text, "availability_text")}
                deadline = time.monotonic() + 60
                while True:
                    try:
                        definitions = client.request("textDocument/definition", params, deadline=deadline)
                    except RuntimeError as error:
                        if error.args[0].get("code") != -32801:
                            raise
                        definitions = None
                    if definitions:
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Workspace indexing did not yield a definition")
                    time.sleep(0.2)
                locations = definitions if isinstance(definitions, list) else [definitions]
                assert any("fluzo-tui/src/lib.rs" in item.get("uri", item.get("targetUri", "")) for item in locations)
                references = client.request("textDocument/references", {**params, "context": {"includeDeclaration": True}})
                assert len({item["uri"] for item in references}) >= 2
                edit = client.request("textDocument/rename", {**params, "newName": "status_label"})
                changes = dict(edit.get("changes", {}))
                for change in edit.get("documentChanges", []):
                    changes[change["textDocument"]["uri"]] = change["edits"]
                assert len(changes) >= 2
                for uri, edits in changes.items():
                    path = Path(unquote(urlparse(uri).path)).resolve()
                    assert path.is_relative_to(workspace)
                    content = path.read_text()
                    for change in sorted(edits, key=lambda item: offset(content, item["range"]["start"]), reverse=True):
                        start = offset(content, change["range"]["start"])
                        end = offset(content, change["range"]["end"])
                        content = content[:start] + change["newText"] + content[end:]
                    path.write_text(content)
                test_workspace(workspace, environment)
                broken = text + '\nfn diagnostic_fixture() { let value: u32 = "invalid"; }\n'
                client.notify("textDocument/didChange", {
                    "textDocument": {"uri": source.as_uri(), "version": 2},
                    "contentChanges": [{"text": broken}],
                })
                source.write_text(broken)
                client.notify("textDocument/didSave", {"textDocument": {"uri": source.as_uri()}})
                deadline = time.monotonic() + 60
                while True:
                    client.request("textDocument/documentSymbol", {"textDocument": {"uri": source.as_uri()}}, deadline=deadline)
                    if any(message.get("method") == "textDocument/publishDiagnostics" and
                           any(str(item.get("code")) == "E0308" for item in message["params"].get("diagnostics", []))
                           for message in client.notifications):
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Expected E0308 diagnostic was not published")
                    time.sleep(0.2)
                client.request("shutdown", None)
                client.notify("exit", None)
                process.wait(timeout=10)
                assert process.returncode == 0
                print("LSP definition, cross-crate references/rename, renamed tests and E0308 diagnostics passed in an isolated offline fixture.")
            except Exception as error:
                if isinstance(error, (subprocess.CalledProcessError, subprocess.TimeoutExpired)):
                    for output in (error.stdout, error.stderr):
                        if output:
                            print(output[-16384:].decode(errors="replace") if isinstance(output, bytes) else output[-16384:], file=sys.stderr)
                errors.flush()
                with (root / "server.log").open("rb") as log:
                    log.seek(0, os.SEEK_END)
                    log.seek(max(0, log.tell() - 16384))
                    print(log.read().decode(errors="replace"), file=sys.stderr)
                raise
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
                process.stdin.close()
                process.stdout.close()


if __name__ == "__main__":
    main()
