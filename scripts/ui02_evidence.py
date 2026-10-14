#!/usr/bin/env python3
"""Generate durable sanitized UI-02 evidence screen dumps.

UI-02 C4 (#39) requires reviewed synthetic artifacts tied to an exact source
revision. Temporary local captures are not durable evidence, and raw terminal
byte streams are a terminal-control hazard for a repository. This harness drives
the real binary in a private PTY at each required viewport and color mode, then
commits only the rendered cell grid: escape sequences are interpreted by the
shared Screen renderer from test_tui.py, never written out.

Usage:
    python3 scripts/ui02_evidence.py            # write evidence/ui02/
    python3 scripts/ui02_evidence.py --verify   # regenerate and compare

Each dump is deterministic only for the static presentation: cases pin
--animation-fps 0 so no frame timing enters the artifact. Point-in-time values
such as elapsed counters are reported in the header instead of being asserted.
"""

import argparse
import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_tui import Screen  # reuse the existing PTY screen renderer, no new framework

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "evidence" / "ui02"

# (id, rows, cols, color mode, extra binary arguments, key to reach the view, view marker)
SESSION = b"Ask anything"
CASES = [
    ("session-80x24-truecolor", 24, 80, "truecolor", [], b"", SESSION),
    ("session-80x24-ansi", 24, 80, "ansi", [], b"", SESSION),
    ("session-80x24-nocolor", 24, 80, "nocolor", [], b"", SESSION),
    ("session-120x40-truecolor", 40, 120, "truecolor", [], b"", SESSION),
    ("session-120x40-ansi", 40, 120, "ansi", [], b"", SESSION),
    ("session-120x40-nocolor", 40, 120, "nocolor", [], b"", SESSION),
    ("session-160x50-truecolor", 50, 160, "truecolor", [], b"", SESSION),
    ("session-160x50-ansi", 50, 160, "ansi", [], b"", SESSION),
    ("session-160x50-nocolor", 50, 160, "nocolor", [], b"", SESSION),
    ("session-80x24-high-contrast", 24, 80, "truecolor", ["--theme", "high-contrast"], b"", SESSION),
    ("session-120x40-high-contrast", 40, 120, "truecolor", ["--theme", "high-contrast"], b"", SESSION),
    ("session-160x50-high-contrast", 50, 160, "truecolor", ["--theme", "high-contrast"], b"", SESSION),
    ("help-120x40-truecolor", 40, 120, "truecolor", [], b"\x07", b"Help 1/"),
    ("sessions-120x40-truecolor", 40, 120, "truecolor", [], b"\x13", b"Sessions | offline demo"),
    ("models-120x40-truecolor", 40, 120, "truecolor", [], b"\x0c", b"Models | offline demo"),
]

COLOR_ENVIRONMENT = {
    "truecolor": {"TERM": "xterm-ghostty", "COLORTERM": "truecolor"},
    "ansi": {"TERM": "xterm-256color"},
    "nocolor": {"TERM": "xterm-ghostty", "COLORTERM": "truecolor", "NO_COLOR": "1"},
}

def build_binary():
    result = subprocess.run(
        ["cargo", "build", "-p", "fluzo-cli", "--locked", "--offline", "--message-format=json"],
        cwd=ROOT, capture_output=True, text=True, check=True, timeout=600,
    )
    for line in result.stdout.splitlines():
        if not line.startswith("{"):
            continue
        import json
        artifact = json.loads(line)
        if artifact.get("executable") and artifact.get("target", {}).get("name") == "fluzo":
            return artifact["executable"]
    raise SystemExit("fluzo binary not found in cargo output")


def render(binary, case):
    identifier, rows, columns, color_mode, extra, view_key, marker = case
    with tempfile.TemporaryDirectory(prefix="fluzo-evidence-") as directory:
        root = Path(directory)
        (root / "source.rs").write_text("unchanged fixture")
        master, slave = os.openpty()
        import fcntl
        import select
        import termios
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        environment = {"HOME": directory, "XDG_CONFIG_HOME": directory, "PATH": os.environ.get("PATH", "")}
        environment.update(COLOR_ENVIRONMENT[color_mode])
        arguments = [binary, "demo", "--interactive", "--animation-fps", "0", "--dev-menu", *extra]
        child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave, cwd=root, env=environment)
        os.close(slave)
        screen = Screen()
        deadline = time.monotonic() + 15
        try:
            while SESSION not in screen.text() and time.monotonic() < deadline:
                if select.select([master], [], [], 0.2)[0]:
                    data = os.read(master, 65536)
                    if not data:
                        break
                    screen.feed(data)
            if SESSION not in screen.text():
                raise SystemExit(f"{identifier}: composer marker never appeared")
            if view_key:
                os.write(master, view_key)
                view_deadline = time.monotonic() + 10
                while marker not in screen.text() and time.monotonic() < view_deadline:
                    if select.select([master], [], [], 0.2)[0]:
                        screen.feed(os.read(master, 65536))
                if marker not in screen.text():
                    raise SystemExit(f"{identifier}: view marker {marker!r} never appeared")
            # Settle: only dump a screen that has stopped changing. At --animation-fps 0
            # the presentation must become static, so a screen that never settles is a
            # finding rather than a flaky capture.
            settle_deadline = time.monotonic() + 12
            previous = None
            while time.monotonic() < settle_deadline:
                if select.select([master], [], [], 0.5)[0]:
                    screen.feed(os.read(master, 65536))
                current = screen.text()
                if previous is not None and current == previous:
                    break
                previous = current
            else:
                raise SystemExit(f"{identifier}: screen never became static at fps 0")
            body = screen.text().decode("utf-8", "replace")
        finally:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
            os.close(master)
    body = "\n".join(line.rstrip() for line in body.split("\n")[:rows])
    if "\x1b" in body:
        raise SystemExit(f"{identifier}: escape byte survived rendering, refusing to commit")
    return body


def header(case, revision, body):
    identifier, rows, columns, color_mode, extra, _, marker = case
    digest = hashlib.sha256(body.encode()).hexdigest()
    return (
        f"ui02-evidence: {identifier}\n"
        f"source-revision: {revision}\n"
        f"viewport: {columns}x{rows} cells\n"
        f"color-mode: {color_mode} ({', '.join(f'{k}={v}' for k, v in sorted(COLOR_ENVIRONMENT[color_mode].items()))})\n"
        f"command: fluzo demo --interactive --animation-fps 0 --dev-menu{' ' + ' '.join(extra) if extra else ''}\n"
        f"generator: scripts/ui02_evidence.py\n"
        f"sha256-body: {digest}\n"
        "--- sanitized cell grid below; no escape sequences ---\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--verify", action="store_true", help="regenerate and compare against committed dumps")
    arguments = parser.parse_args()
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    binary = build_binary()
    if not arguments.verify:
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        for existing in EVIDENCE.glob("*.txt"):
            existing.unlink()
    drift = []
    for case in CASES:
        body = render(binary, case)
        document = header(case, revision, body) + body + "\n"
        target = EVIDENCE / f"{case[0]}.txt"
        if arguments.verify:
            committed = target.read_text() if target.exists() else ""
            committed_body = committed.split("--- sanitized cell grid below; no escape sequences ---\n", 1)
            if len(committed_body) != 2 or committed_body[1] != body + "\n":
                drift.append(case[0])
            print(f"verify {case[0]}: {'DRIFT' if case[0] in drift else 'match'}")
        else:
            target.write_text(document)
            print(f"wrote {target.relative_to(ROOT)} ({len(body)} rendered bytes)")
    if arguments.verify and drift:
        raise SystemExit(f"non-deterministic artifacts: {', '.join(drift)}")
    if not arguments.verify:
        print(f"evidence written under {EVIDENCE.relative_to(ROOT)} for revision {revision}")


if __name__ == "__main__":
    main()
