import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = {
    "fluzo-core": set(),
    "fluzo-runtime": {"fluzo-core"},
    "fluzo-tui": {"fluzo-core"},
    "fluzo-cli": {"fluzo-core", "fluzo-runtime", "fluzo-tui"},
}
SERDE_PACKAGES = {"serde", "serde_core", "serde_derive", "syn", "proc-macro2", "quote", "unicode-ident"}
TOML_PACKAGES = {"toml_edit", "toml_parser", "toml_writer", "toml_datetime", "serde_spanned", "indexmap", "hashbrown", "equivalent", "winnow", "memchr"}
EXTERNAL = {
    "fluzo-core": set(SERDE_PACKAGES),
    "fluzo-runtime": SERDE_PACKAGES | TOML_PACKAGES,
    "fluzo-tui": set(SERDE_PACKAGES),
    "fluzo-cli": SERDE_PACKAGES | TOML_PACKAGES,
}
DIRECT_EXTERNAL = {"fluzo-core": {"serde"}, "fluzo-runtime": {"toml_edit"}, "fluzo-tui": set(), "fluzo-cli": set()}
REVIEWED_PACKAGES = {
    "serde": ("1.0.229", {"derive", "serde_derive", "std"}),
    "serde_core": ("1.0.229", {"alloc", "default", "result", "std"}),
    "serde_derive": ("1.0.229", {"default"}),
    "syn": ("3.0.5", {"clone-impls", "default", "derive", "full", "parsing", "printing", "proc-macro"}),
    "proc-macro2": ("1.0.107", {"default", "proc-macro"}),
    "quote": ("1.0.47", {"default", "proc-macro"}),
    "unicode-ident": ("1.0.24", set()),
    "toml_edit": ("0.25.15+spec-1.1.0", {"display", "parse", "serde"}),
    "toml_parser": ("1.1.3+spec-1.1.0", {"alloc", "default", "std"}),
    "toml_writer": ("1.1.2+spec-1.1.0", {"alloc", "default", "std"}),
    "toml_datetime": ("1.1.1+spec-1.1.0", {"alloc", "default", "serde", "std"}),
    "serde_spanned": ("1.1.1", {"alloc", "default", "serde", "std"}),
    "indexmap": ("2.14.2", {"default", "std"}),
    "hashbrown": ("0.17.1", set()),
    "equivalent": ("1.0.2", set()),
    "winnow": ("1.0.4", {"alloc", "ascii", "binary", "default", "parser", "std"}),
    "memchr": ("2.8.3", {"alloc", "std"}),
}
SIMULATOR_PACKAGES = {
    "atomic-waker": ("1.1.2", set()),
    "bytes": ("1.12.1", {"default", "std"}),
    "futures-channel": ("0.3.34", {"alloc", "default", "std"}),
    "futures-core": ("0.3.34", {"alloc", "default", "std"}),
    "http": ("1.5.0", {"default", "std"}),
    "http-body": ("1.1.0", set()),
    "http-body-util": ("0.1.5", set()),
    "httparse": ("1.10.1", {"default", "std"}),
    "httpdate": ("1.0.3", set()),
    "hyper": ("1.11.1", {"client", "default", "http1", "server"}),
    "hyper-util": ("0.1.20", {"tokio"}),
    "itoa": ("1.0.18", set()),
    "libc": ("0.2.189", {"default", "std"}),
    "mio": ("1.2.3", {"net", "os-ext", "os-poll"}),
    "pin-project-lite": ("0.2.17", set()),
    "serde_json": ("1.0.151", {"std"}),
    "smallvec": ("1.16.1", {"const_generics", "const_new"}),
    "socket2": ("0.6.5", {"all"}),
    "tokio": ("1.53.1", {"bytes", "default", "io-util", "libc", "macros", "mio", "net", "rt", "socket2", "sync", "time", "tokio-macros", "windows-sys"}),
    "tokio-macros": ("2.7.2", set()),
    "try-lock": ("0.2.5", set()),
    "want": ("0.3.1", set()),
    "wasi": ("0.11.1+wasi-snapshot-preview1", {"default", "std"}),
    "windows-link": ("0.2.1", set()),
    "windows-sys": ("0.61.2", {
        "Wdk", "Wdk_Foundation", "Wdk_Storage", "Wdk_Storage_FileSystem", "Wdk_System", "Wdk_System_IO",
        "Win32", "Win32_Foundation", "Win32_Networking", "Win32_Networking_WinSock", "Win32_Security",
        "Win32_Storage", "Win32_Storage_FileSystem", "Win32_System", "Win32_System_IO", "Win32_System_Pipes",
        "Win32_System_SystemServices", "Win32_System_Threading", "Win32_System_WindowsProgramming", "default",
    }),
    "zmij": ("1.0.23", set()),
}
REVIEWED_PACKAGES.update(SIMULATOR_PACKAGES)
SIMULATOR_DIRECT = {"serde", "serde_json", "hyper", "hyper-util", "http-body-util", "tokio"}
DISCOVERY_PACKAGES = set(SIMULATOR_PACKAGES) | {"log"}
EXTERNAL["fluzo-runtime"] |= DISCOVERY_PACKAGES
EXTERNAL["fluzo-cli"] |= DISCOVERY_PACKAGES
DIRECT_EXTERNAL["fluzo-runtime"] |= {"serde_json", "hyper", "hyper-util", "http-body-util", "tokio"}
TUI_PACKAGES = {
    ("allocator-api2", "0.2.21"): {"alloc"},
    ("bitflags", "2.13.2"): {"std"},
    ("cassowary", "0.3.0"): set(),
    ("castaway", "0.2.4"): {"alloc"},
    ("cfg-if", "1.0.5"): set(),
    ("compact_str", "0.8.2"): {"default", "std"},
    ("crossterm", "0.28.1"): {"bracketed-paste", "default", "events", "windows"},
    ("crossterm_winapi", "0.9.1"): set(),
    ("darling", "0.24.1"): {"default", "suggestions"},
    ("darling_core", "0.24.1"): {"strsim", "suggestions"},
    ("darling_macro", "0.24.1"): set(),
    ("either", "1.18.0"): {"std", "use_std"},
    ("errno", "0.3.14"): {"default", "std"},
    ("foldhash", "0.1.5"): set(),
    ("hashbrown", "0.15.5"): {"allocator-api2", "default", "default-hasher", "equivalent", "inline-more", "raw-entry"},
    ("heck", "0.5.0"): set(),
    ("ident_case", "1.0.1"): set(),
    ("indoc", "2.0.7"): set(),
    ("instability", "0.3.14"): set(),
    ("itertools", "0.13.0"): {"default", "use_alloc", "use_std"},
    ("linux-raw-sys", "0.4.15"): {"elf", "errno", "general", "ioctl", "no_std"},
    ("lock_api", "0.4.14"): {"atomic_usize", "default"},
    ("log", "0.4.34"): set(),
    ("lru", "0.12.5"): {"default", "hashbrown"},
    ("parking_lot", "0.12.5"): {"default"},
    ("parking_lot_core", "0.9.12"): set(),
    ("paste", "1.0.15"): set(),
    ("ratatui", "0.29.0"): {"crossterm"},
    ("redox_syscall", "0.5.18"): {"default", "userspace"},
    ("rustix", "0.38.44"): {"alloc", "libc-extra-traits", "std", "stdio", "termios"},
    ("rustversion", "1.0.23"): set(),
    ("ryu", "1.0.23"): set(),
    ("scopeguard", "1.2.0"): set(),
    ("signal-hook", "0.3.18"): {"channel", "default", "iterator"},
    ("signal-hook-mio", "0.2.5"): {"mio-1_0", "support-v1_0"},
    ("signal-hook-registry", "1.4.8"): set(),
    ("static_assertions", "1.1.0"): set(),
    ("strsim", "0.11.1"): set(),
    ("strum", "0.26.3"): {"default", "derive", "std", "strum_macros"},
    ("strum_macros", "0.26.4"): set(),
    ("syn", "2.0.119"): {"clone-impls", "default", "derive", "extra-traits", "parsing", "printing", "proc-macro"},
    ("unicode-segmentation", "1.13.3"): set(),
    ("unicode-truncate", "1.1.0"): {"default", "std"},
    ("unicode-width", "0.1.14"): {"cjk", "default"},
    ("unicode-width", "0.2.0"): {"cjk", "default"},
    ("winapi", "0.3.9"): {"consoleapi", "handleapi", "impl-default", "processenv", "synchapi", "winbase", "winerror", "winuser"},
    ("winapi-i686-pc-windows-gnu", "0.4.0"): set(),
    ("winapi-x86_64-pc-windows-gnu", "0.4.0"): set(),
    ("windows-sys", "0.59.0"): {"Win32", "Win32_Foundation", "Win32_NetworkManagement", "Win32_NetworkManagement_IpHelper", "Win32_Networking", "Win32_Networking_WinSock", "Win32_System", "Win32_System_Threading", "default"},
    ("windows-targets", "0.52.6"): set(),
    **{(name, "0.52.6"): set() for name in (
        "windows_aarch64_gnullvm", "windows_aarch64_msvc", "windows_i686_gnu",
        "windows_i686_gnullvm", "windows_i686_msvc", "windows_x86_64_gnu",
        "windows_x86_64_gnullvm", "windows_x86_64_msvc",
    )},
}
REVIEWED_PACKAGES["syn"][1].add("extra-traits")
REVIEWED_PACKAGES["libc"][1].add("extra_traits")
REVIEWED_PACKAGES["mio"][1].update({"default", "log"})
REVIEWED_PACKAGES["windows-sys"][1].update({"Win32_System_Diagnostics", "Win32_System_Diagnostics_Debug"})
TUI_EXTERNAL = {name for name, _ in TUI_PACKAGES} | {"itoa", "libc", "mio", "smallvec", "wasi", "windows-link", "equivalent"}
EXTERNAL["fluzo-tui"] |= TUI_EXTERNAL
EXTERNAL["fluzo-cli"] |= TUI_EXTERNAL
DIRECT_EXTERNAL["fluzo-tui"] = {"ratatui", "crossterm", "unicode-segmentation", "unicode-width", "signal-hook"}
COMMON_SKILLS = {
    "convention-document", "delivery-review-github", "git-conventional-commit",
    "issue-refine-github", "plan-create", "plan-execute", "release-prepare-github",
}
COMMON_REVISION = "b73053c28ba6e3fc1e4993d47fce933a5d861e85"
FLUZO_SKILLS = {"fluzo-deterministic-testing", "fluzo-rust-boundaries", "tui-design", "rust-practices", "rust-review"}
FLUZO_REVISION = "48a1ac36fc229ccbcb1fe671d5dbd9774a567d56"
SKILLS = FLUZO_SKILLS | COMMON_SKILLS


def validate_graph(metadata):
    packages = {package["id"]: package for package in metadata["packages"]}
    members = {packages[identity]["name"] for identity in metadata["workspace_members"]}
    if members != set(ALLOWED):
        raise ValueError(f"Unexpected workspace members: {members}")
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    for identity in metadata["workspace_members"]:
        package = packages[identity]
        if package["license"] != "MIT" or package["rust_version"] != "1.98":
            raise ValueError(f"Unexpected license/MSRV: {package['name']}")
        if package["features"] or any(dependency["optional"] for dependency in package["dependencies"]):
            raise ValueError(f"New feature declarations require an explicit graph policy review: {package['name']}")
        for dependency in nodes[identity]["deps"]:
            if any(kind["kind"] == "build" for kind in dependency["dep_kinds"]):
                raise ValueError(f"Forbidden production path: direct build dependency in {package['name']}")
            if any(kind["kind"] == "dev" for kind in dependency["dep_kinds"]):
                name = packages[dependency["pkg"]]["name"]
                if package["name"] != "fluzo-runtime" or name not in SIMULATOR_DIRECT:
                    raise ValueError(f"Unreviewed test dependency: {package['name']} -> {name}")
        pending = [(identity, [package["name"]])]
        visited = set()
        while pending:
            current, path = pending.pop()
            if current in visited:
                continue
            visited.add(current)
            for dependency in nodes[current]["deps"]:
                if not any(kind["kind"] in (None, "build") for kind in dependency["dep_kinds"]):
                    continue
                target = dependency["pkg"]
                name = packages[target]["name"]
                if name not in ALLOWED[package["name"]] | EXTERNAL[package["name"]]:
                    raise ValueError(f"Forbidden production path: {' -> '.join(path + [name])}")
                if current == identity and name not in ALLOWED and name not in DIRECT_EXTERNAL[package["name"]]:
                    raise ValueError(f"Forbidden direct dependency: {package['name']} -> {name}")
                pending.append((target, path + [name]))
    for node in nodes.values():
        package = packages[node["id"]]
        name = package["name"]
        if name in ALLOWED:
            allowed_features = set()
        else:
            identity = (name, package["version"])
            if identity in TUI_PACKAGES:
                allowed_features = TUI_PACKAGES[identity]
            elif name in REVIEWED_PACKAGES:
                version, allowed_features = REVIEWED_PACKAGES[name]
                if package["version"] != version:
                    raise ValueError(f"Unreviewed package source/version: {name}")
            else:
                raise ValueError(f"Unreviewed package source/version: {name}")
            if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
                raise ValueError(f"Unreviewed package source/version: {name}")
        if not set(node["features"]) <= allowed_features:
            raise ValueError("New feature combinations require an explicit graph policy review")


def validate_collection(root, collection, names, revision):
    directory = root / ".agents/skills"
    manifest = tomllib.loads((root / f".agents/{collection}.toml").read_text())
    if manifest["source"] != f"https://github.com/fluzo-labs/{collection}" or manifest["revision"] != revision:
        raise ValueError(f"Unexpected {collection} source/revision")
    installed = set()
    for name in names:
        folder = directory / name
        if folder.is_symlink():
            raise ValueError(f"Linked collection skill: {name}")
        for path in folder.rglob("*"):
            if path.is_symlink():
                raise ValueError(f"Linked collection skill resource: {path}")
            if path.is_file():
                installed.add(path.relative_to(directory).as_posix())
        if not (folder / "LICENSE").is_file() or not (folder / "ORIGIN.md").is_file():
            raise ValueError(f"Missing collection skill license/provenance: {name}")
    if installed != set(manifest["files"]):
        raise ValueError("Collection skill file inventory mismatch")
    for name, digest in manifest["files"].items():
        if hashlib.sha256((directory / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"Collection skill checksum mismatch: {name}")


def validate_skills(root=ROOT):
    directory = root / ".agents/skills"
    entries = sorted(directory.glob("*/SKILL.md"))
    if {entry.parent.name for entry in entries} != SKILLS:
        raise ValueError("Unexpected skill inventory")
    validate_collection(root, "common-skills", COMMON_SKILLS, COMMON_REVISION)
    validate_collection(root, "fluzo-skills", FLUZO_SKILLS, FLUZO_REVISION)
    for entry in entries:
        text = entry.read_text()
        if not text.startswith("---\n") or "\n---\n" not in text[4:]:
            raise ValueError(f"Missing frontmatter: {entry}")
        frontmatter = text.split("---", 2)[1]
        if f"name: {entry.parent.name}\n" not in frontmatter:
            raise ValueError(f"Skill name mismatch: {entry}")
        if not re.search(r"^description: .+", frontmatter, re.M):
            raise ValueError(f"Missing skill trigger: {entry}")
        if "user-invocable: true" not in frontmatter:
            raise ValueError(f"Missing explicit invocation: {entry}")
    for entry in directory.rglob("*.md"):
        for target in re.findall(r"\]\(([^)]+)\)", entry.read_text()):
            if "://" not in target and not target.startswith("#"):
                if not (entry.parent / target.split("#")[0]).is_file():
                    raise ValueError(f"Broken skill reference: {entry}: {target}")
    for name, revision in {
        "rust-practices": "fd2a861ab0406a4ac536a55274d14ea6fd1ca9c9",
        "rust-review": "eb485a5ddb68e0ded3d79549e994f63ec0a6f6c0",
    }.items():
        if revision not in (directory / name / "ORIGIN.md").read_text():
            raise ValueError(f"Unpinned skill: {name}")
        if not (directory / name / "LICENSE").is_file():
            raise ValueError(f"Missing license: {name}")


def main():
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
    assert toolchain["channel"] == "1.98.0"
    assert {"rust-analyzer", "rust-src", "clippy", "rustfmt"} <= set(toolchain["components"])
    expected = "lsp add rust-analyzer --command rustup --args run --args 1.98.0 --args rust-analyzer --filetypes rust --root-markers Cargo.toml\n"
    assert (ROOT / "crushrc").read_text() == expected
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
        cwd=ROOT,
        text=True,
    )
    validate_graph(json.loads(metadata))
    validate_skills()
    print(f"Resolved foundation graph, toolchain, LSP config and {len(SKILLS)} skills checked.")


if __name__ == "__main__":
    main()
