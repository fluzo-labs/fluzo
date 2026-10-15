#!/usr/bin/env python3
"""Generate the UI-05 single-session review package for every merged UI surface.

UI-05 (#10) asks for one review session covering the surfaces merged by
UI-02 (#7), UI-03 (#8) and UI-04 (#9) at the three required viewports.
Raw terminal byte streams are a terminal-control hazard for a repository and
temporary local captures are not durable evidence, so this harness drives
the real binary in a private PTY and commits only the rendered cell grid:
escape sequences are interpreted by the shared Screen renderer from
test_tui.py and never written out.

Evidence class: `pty`. A dump proves what the binary rendered into a real
pseudo-terminal at a pinned size. It does not prove visual quality, colour
accuracy, motion quality, screen-reader behaviour or human acceptance.
Those are separate classes and stay human-only; see UI05_REVIEW_CHECKLIST.md.

Usage:
    python3 scripts/ui05_review_package.py             # write evidence/ui05/
    python3 scripts/ui05_review_package.py --verify    # regenerate and compare
    python3 scripts/ui05_review_package.py --list      # print the case matrix

Every case pins --animation-fps 0 so no frame timing enters an artifact.
A case whose marker never appears, or whose screen never becomes static,
aborts instead of writing a partial or wrong-screen dump: that is a
finding, not a retry.
"""

import argparse
import hashlib
import json
import os
import select
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_tui import Screen  # reuse the existing PTY screen renderer, no new framework

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "evidence" / "ui05"
VIEWPORTS = [(24, 80), (40, 120), (50, 160)]
BANNER = "--- sanitized cell grid below; no escape sequences ---"
# Some surfaces legitimately render the workspace path. The private PTY root is
# a fresh temporary directory on every run, so the literal path is the only
# non-deterministic content in those dumps. It is replaced by this placeholder
# and the substitution is declared in each artifact header rather than hidden.
ROOT_PLACEHOLDER = "<WORKSPACE_ROOT>"

COLOR_ENVIRONMENT = {
    "truecolor": {"TERM": "xterm-ghostty", "COLORTERM": "truecolor"},
    "nocolor": {"TERM": "xterm-ghostty", "COLORTERM": "truecolor", "NO_COLOR": "1"},
    "high-contrast": {"TERM": "xterm-ghostty", "COLORTERM": "truecolor"},
}

# A configuration fixture with no reachable endpoint and no credential. The
# port is bound and closed by the caller so nothing can be contacted even by
# accident; the harness never issues a request.
CONFIG_FIXTURE = (
    "schema_version = 1\n"
    "[agent]\n"
    "model = 'coder'\n"
    "[capacity_pools.local]\n"
    "[models.coder]\n"
    "base_url = 'http://127.0.0.1:1/v1'\n"
    "model = 'fixture'\n"
    "capacity_id = 'fixture'\n"
    "pool = 'local'\n"
    "auth = 'env'\n"
    "api_key_env = 'ABSENT_FIXTURE'\n"
)

DEMO_BASE = ["demo", "--interactive", "--animation-fps", "0", "--dev-menu"]
CONFIG_BASE = ["--animation-fps", "0"]

# id, owning story, program arguments, root fixture, key sequence, marker, color modes
SURFACES = [
    (
        "welcome",
        "UI-03",
        CONFIG_BASE,
        "empty",
        [],
        b"Welcome to Fluzo",
        ("truecolor", "nocolor", "high-contrast"),
    ),
    (
        "config-workspace",
        "UI-03",
        CONFIG_BASE,
        "config",
        [],
        b"Configuration workspace",
        ("truecolor", "nocolor", "high-contrast"),
    ),
    (
        "config-palette",
        "UI-03",
        CONFIG_BASE,
        "config",
        [b"\x10"],
        b"Providers",
        ("truecolor",),
    ),
    (
        "session",
        "UI-02",
        DEMO_BASE,
        "empty",
        [],
        b"Ask anything",
        ("truecolor", "nocolor", "high-contrast"),
    ),
    (
        "devmenu",
        "UI-02",
        DEMO_BASE,
        "empty",
        [b"\x10", b"Developer", b"\r"],
        b"Developer menu",
        ("truecolor",),
    ),
    (
        "tasks",
        "UI-04",
        DEMO_BASE,
        "empty",
        [b"\x14"],
        b"Tasks | offline demo",
        ("truecolor",),
    ),
    (
        "task-detail",
        "UI-04",
        DEMO_BASE,
        "empty",
        [b"\x14", b"\r"],
        b"Task detail | offline demo",
        ("truecolor",),
    ),
    (
        "permission",
        "UI-04",
        DEMO_BASE,
        "empty",
        [b"\x14", b"\r", b"p"],
        b"Permission | synthetic request",
        ("truecolor",),
    ),
    (
        "diff",
        "UI-04",
        DEMO_BASE,
        "empty",
        [b"\x14", b"\r", b"d"],
        b"Diff & verification",
        ("truecolor",),
    ),
    (
        "notices",
        "UI-04",
        [*DEMO_BASE, "--desktop-notifications"],
        "empty",
        [
            b"\x10",
            b"Developer",
            b"\r",
            b"Notification preview",
            b"\r",
            # Alt+N twice, kitty-protocol encoding; the shell reads these as
            # the demo "add notice" binding.
            b"\x1b[110;3u",
            b"\x1b[110;3u",
        ],
        b"[DEMO INFO]",
        ("truecolor",),
    ),
]


def cases():
    """Yield (case_id, surface_row, rows, cols, color_mode) for the matrix."""
    for surface in SURFACES:
        identifier, story, arguments, fixture, keys, marker, modes = surface
        for rows, columns in VIEWPORTS:
            for mode in modes:
                yield (
                    f"{identifier}-{rows}x{columns}-{mode}",
                    surface,
                    rows,
                    columns,
                    mode,
                )


def build_binary():
    result = subprocess.run(
        ["cargo", "build", "-p", "fluzo-cli", "--locked", "--offline",
         "--message-format=json"],
        cwd=ROOT, capture_output=True, text=True, check=True, timeout=900,
    )
    for line in result.stdout.splitlines():
        if not line.startswith("{"):
            continue
        artifact = json.loads(line)
        if artifact.get("executable") and artifact.get("target", {}).get("name") == "fluzo":
            return artifact["executable"]
    raise SystemExit("fluzo binary not found in cargo output")


def drain(master, screen, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.2)[0]:
            screen.feed(os.read(master, 65536))


def wait_for(master, screen, marker, seconds, identifier, stage):
    deadline = time.monotonic() + seconds
    while marker not in screen.text():
        if time.monotonic() >= deadline:
            raise SystemExit(f"{identifier}: {stage} marker {marker!r} never appeared")
        if select.select([master], [], [], 0.2)[0]:
            screen.feed(os.read(master, 65536))


def render(binary, case_id, surface, rows, columns, color_mode):
    _, _, arguments, fixture, keys, marker, _ = surface
    with tempfile.TemporaryDirectory(prefix="fluzo-ui05-") as directory:
        root = Path(directory)
        if fixture == "config":
            (root / ".fluzo").write_text(CONFIG_FIXTURE)
        master, slave = os.openpty()
        import fcntl
        import termios
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        environment = {
            "HOME": directory,
            "XDG_CONFIG_HOME": directory,
            "PATH": os.environ.get("PATH", ""),
        }
        environment.update(COLOR_ENVIRONMENT[color_mode])
        child = subprocess.Popen(
            [binary, *arguments], stdin=slave, stdout=slave, stderr=slave,
            cwd=root, env=environment,
        )
        os.close(slave)
        screen = Screen()
        try:
            # The base session/composer must exist before any surface is reached.
            wait_for(master, screen, b"Ask anything" if "demo" in arguments
                     else b"Welcome to Fluzo" if fixture == "empty"
                     else b"Configuration workspace", 25, case_id, "base")
            if fixture == "empty" and "demo" not in arguments:
                # The welcome surface is the base state; nothing to drive.
                pass
            for key in keys:
                os.write(master, key)
                drain(master, screen, 0.6)
            wait_for(master, screen, marker, 15, case_id, "surface")
            # Settle: only a screen that stopped changing may be dumped. At
            # --animation-fps 0 the presentation must become static, so a
            # screen that never settles is a finding rather than a flaky capture.
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
                raise SystemExit(f"{case_id}: screen never became static at fps 0")
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
        raise SystemExit(f"{case_id}: escape byte survived rendering, refusing to commit")
    if directory not in body:
        return body
    normalized = body.replace(directory, ROOT_PLACEHOLDER)
    if directory in normalized:
        raise SystemExit(f"{case_id}: temp root still present after normalization")
    return normalized


def header(case_id, surface, rows, columns, color_mode, revision, body):
    identifier, story, arguments, _, keys, marker, _ = surface
    digest = hashlib.sha256(body.encode()).hexdigest()
    return (
        f"ui05-review-package: {case_id}\n"
        f"surface: {identifier} ({story})\n"
        f"source-revision: {revision}\n"
        f"viewport: {columns}x{rows} cells\n"
        f"color-mode: {color_mode} "
        f"({', '.join(f'{k}={v}' for k, v in sorted(COLOR_ENVIRONMENT[color_mode].items()))})\n"
        f"command: fluzo {' '.join(arguments)}\n"
        f"keys: {json.dumps([k.decode('unicode_escape') for k in keys])}\n"
        f"surface-marker: {marker.decode()}\n"
        f"normalization: private temp workspace root rendered as {ROOT_PLACEHOLDER}\n"
        f"evidence-class: pty (rendered cell grid; not visual acceptance)\n"
        f"generator: scripts/ui05_review_package.py\n"
        f"sha256-body: {digest}\n"
        f"{BANNER}\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--verify", action="store_true",
                      help="regenerate and compare against committed dumps")
    parser.add_argument("--list", action="store_true", help="print the case matrix")
    arguments = parser.parse_args()

    if arguments.list:
        for case_id, surface, rows, columns, mode in cases():
            print(f"{case_id}\t{surface[1]}\t{columns}x{rows}\t{mode}")
        return

    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                             capture_output=True, text=True, check=True).stdout.strip()
    binary = build_binary()
    if not arguments.verify:
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        for existing in EVIDENCE.glob("*.txt"):
            existing.unlink()
    drift = []
    failures = []
    for case_id, surface, rows, columns, mode in cases():
        try:
            body = render(binary, case_id, surface, rows, columns, mode)
        except SystemExit as error:
            failures.append(f"{case_id}: {error}")
            print(f"FAIL {case_id}: {error}")
            continue
        document = header(case_id, surface, rows, columns, mode, revision, body) + body + "\n"
        target = EVIDENCE / f"{case_id}.txt"
        if arguments.verify:
            committed = target.read_text() if target.exists() else ""
            parts = committed.split(f"{BANNER}\n", 1)
            if len(parts) != 2 or parts[1] != body + "\n":
                drift.append(case_id)
            print(f"verify {case_id}: {'DRIFT' if case_id in drift else 'match'}")
        else:
            target.write_text(document)
            print(f"wrote {target.relative_to(ROOT)} ({len(body)} rendered bytes)")
    if failures:
        raise SystemExit(f"{len(failures)} case(s) could not produce evidence:\n  "
                        + "\n  ".join(failures))
    if arguments.verify and drift:
        raise SystemExit(f"non-deterministic artifacts: {', '.join(drift)}")
    if not arguments.verify:
        print(f"evidence written under {EVIDENCE.relative_to(ROOT)} for revision {revision}")


if __name__ == "__main__":
    main()
