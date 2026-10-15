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
    "serde": ("1.0.229", {"alloc", "default", "derive", "rc", "serde_derive", "std"}),
    "serde_core": ("1.0.229", {"alloc", "default", "rc", "result", "std"}),
    "serde_derive": ("1.0.229", {"default"}),
    "syn": ("3.0.5", {"clone-impls", "default", "derive", "fold", "full", "parsing", "printing", "proc-macro", "visit"}),
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
    "memchr": ("2.8.3", {"alloc", "default", "std"}),
}
SIMULATOR_PACKAGES = {
    "atomic-waker": ("1.1.2", set()),
    "bytes": ("1.12.1", {"default", "std"}),
    "futures-channel": ("0.3.34", {"alloc", "default", "futures-sink", "sink", "std"}),
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
    "serde_json": ("1.0.151", {"default", "std"}),
    "smallvec": ("1.16.1", {"const_generics", "const_new"}),
    "socket2": ("0.6.5", {"all"}),
    "tokio": ("1.53.1", {"bytes", "default", "fs", "io-util", "libc", "macros", "mio", "net", "rt", "socket2", "sync", "time", "tokio-macros", "windows-sys"}),
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
# DAT-01 store dependency graph: reviewed transitive closure of sqlx 0.9.0
# (runtime-tokio + sqlite-bundled). Keyed by (name, version) because the
# resolution carries two versions of foldhash and unicode-width. Feature sets
# are ceilings equal to the activated features at review time.
SQLX_PACKAGES = {
    ("atoi", "2.0.0"): {"default", "std"},
    ("autocfg", "1.5.1"): set(),
    ("base64", "0.22.1"): {"alloc"},
    ("block-buffer", "0.10.4"): set(),
    ("block-buffer", "0.12.1"): set(),  # DAT-02a sha2 0.11 closure
    ("cassowary", "0.3.0"): set(),
    ("cc", "1.6.0"): set(),
    ("cfg-if", "1.0.5"): set(),
    ("cpufeatures", "0.2.17"): set(),
    ("cpufeatures", "0.3.1"): set(),  # DAT-02a sha2 0.11 closure
    ("crc", "3.4.0"): set(),
    ("crc-catalog", "2.5.0"): set(),
    ("crossbeam-queue", "0.3.14"): {"alloc", "default", "std"},
    ("crossbeam-utils", "0.8.23"): {"std"},
    ("crossterm_winapi", "0.9.1"): set(),
    ("crypto-common", "0.1.7"): {"std"},
    ("crypto-common", "0.2.2"): set(),  # DAT-02a sha2 0.11 closure
    ("darling_macro", "0.24.1"): set(),
    ("digest", "0.10.7"): {"alloc", "block-buffer", "core-api", "default", "std"},
    ("digest", "0.11.3"): {"alloc", "block-api", "default"},  # DAT-02a sha2 0.11 closure
    ("displaydoc", "0.2.7"): set(),
    ("dotenvy", "0.15.7"): set(),
    ("event-listener", "5.4.2"): {"default", "parking", "std"},
    ("find-msvc-tools", "0.1.14"): set(),
    ("flume", "0.12.0"): {"async", "futures-core", "futures-sink"},
    ("foldhash", "0.1.5"): set(),
    ("foldhash", "0.2.0"): set(),
    ("form_urlencoded", "1.2.2"): {"alloc", "default", "std"},
    ("futures-executor", "0.3.34"): {"default", "std"},
    ("futures-intrusive", "0.5.0"): {"alloc", "default", "parking_lot", "std"},
    ("futures-io", "0.3.34"): {"default", "std"},
    ("futures-sink", "0.3.34"): set(),
    ("futures-task", "0.3.34"): {"alloc", "std"},
    ("futures-util", "0.3.34"): {"alloc", "futures-io", "futures-sink", "io", "memchr", "sink", "slab", "std"},
    ("generic-array", "0.14.7"): {"more_lengths"},
    ("hashbrown", "0.16.1"): {"allocator-api2", "default", "default-hasher", "equivalent", "inline-more", "raw-entry"},
    ("hashlink", "0.11.1"): set(),
    ("heck", "0.5.0"): set(),
    ("hybrid-array", "0.4.15"): set(),  # DAT-02a sha2 0.11 closure (crypto-common 0.2 dep)
    ("hex", "0.4.3"): {"alloc", "default", "std"},
    ("icu_collections", "2.3.0"): set(),
    ("icu_locale_core", "2.3.0"): {"zerovec"},
    ("icu_normalizer", "2.3.0"): {"compiled_data"},
    ("icu_normalizer_data", "2.3.0"): set(),
    ("icu_properties", "2.3.0"): {"compiled_data"},
    ("icu_properties_data", "2.3.0"): set(),
    ("icu_provider", "2.3.1"): {"baked"},
    ("ident_case", "1.0.1"): set(),
    ("idna", "1.1.0"): {"alloc", "compiled_data", "std"},
    ("idna_adapter", "1.2.2"): {"compiled_data"},
    ("indoc", "2.0.7"): set(),
    ("instability", "0.3.14"): set(),
    ("libsqlite3-sys", "0.37.0"): {"bundled", "bundled_bindings", "cc", "pkg-config", "vcpkg"},
    ("litemap", "0.8.3"): set(),
    ("log", "0.4.34"): set(),
    ("num-traits", "0.2.19"): {"std"},
    ("once_cell", "1.21.4"): {"alloc", "default", "race", "std"},
    ("parking", "2.2.1"): set(),
    ("parking_lot_core", "0.9.12"): set(),
    ("paste", "1.0.15"): set(),
    ("percent-encoding", "2.3.2"): {"alloc", "default", "std"},
    ("pkg-config", "0.3.34"): set(),
    ("potential_utf", "0.1.6"): {"zerovec"},
    ("rustversion", "1.0.23"): set(),
    ("ryu", "1.0.23"): set(),
    ("scopeguard", "1.2.0"): set(),
    ("sha2", "0.10.9"): {"default", "std"},
    ("sha2", "0.11.0"): {"alloc"},  # DAT-02a direct artifact content hashing
    ("shlex", "2.0.1"): {"default", "std"},
    ("signal-hook-registry", "1.4.8"): set(),
    ("slab", "0.4.12"): {"std"},
    ("spin", "0.9.9"): {"barrier", "default", "lazy", "lock_api", "lock_api_crate", "mutex", "once", "rwlock", "spin_mutex"},
    ("sqlx", "0.9.0"): {"_rt-tokio", "_sqlite", "runtime-tokio", "sqlite-bundled", "sqlx-sqlite"},
    ("sqlx-core", "0.9.0"): {"_rt-tokio", "any", "crc", "default", "migrate", "offline", "serde", "sha2", "tokio", "tokio-stream"},
    ("sqlx-macros", "0.9.0"): {"_rt-tokio", "default", "sqlite"},
    ("sqlx-macros-core", "0.9.0"): {"_rt-tokio", "_sqlite", "default", "sqlite", "sqlx-sqlite", "tokio"},
    ("sqlx-sqlite", "0.9.0"): {"bundled", "migrate", "offline", "serde"},
    ("stable_deref_trait", "1.2.1"): set(),
    ("static_assertions", "1.1.0"): set(),
    ("strsim", "0.11.1"): set(),
    ("strum_macros", "0.26.4"): set(),
    ("synstructure", "0.14.0"): {"default", "proc-macro"},
    ("thiserror", "2.0.21"): {"std"},
    ("thiserror-impl", "2.0.21"): set(),
    ("tinystr", "0.8.4"): {"zerovec"},
    ("tokio-stream", "0.1.19"): {"default", "fs", "time"},
    ("tracing", "0.1.44"): {"attributes", "default", "log", "std", "tracing-attributes"},
    ("tracing-attributes", "0.1.31"): set(),
    ("tracing-core", "0.1.36"): {"once_cell", "std"},
    ("typenum", "1.20.1"): {"const-generics"},  # widened: sha2 0.11 activates const-generics
    ("unicode-segmentation", "1.13.3"): set(),
    ("url", "2.5.8"): {"default", "std"},
    ("utf8_iter", "1.0.4"): set(),
    ("vcpkg", "0.2.15"): set(),
    ("version_check", "0.9.5"): set(),
    ("winapi-i686-pc-windows-gnu", "0.4.0"): set(),
    ("winapi-x86_64-pc-windows-gnu", "0.4.0"): set(),
    ("windows-targets", "0.52.6"): set(),
    ("windows_aarch64_gnullvm", "0.52.6"): set(),
    ("windows_aarch64_msvc", "0.52.6"): set(),
    ("windows_i686_gnu", "0.52.6"): set(),
    ("windows_i686_gnullvm", "0.52.6"): set(),
    ("windows_i686_msvc", "0.52.6"): set(),
    ("windows_x86_64_gnu", "0.52.6"): set(),
    ("windows_x86_64_gnullvm", "0.52.6"): set(),
    ("windows_x86_64_msvc", "0.52.6"): set(),
    ("writeable", "0.6.4"): set(),
    ("yoke", "0.8.3"): {"derive", "zerofrom"},
    ("yoke-derive", "0.8.4"): set(),
    ("zerofrom", "0.1.8"): {"derive"},
    ("zerofrom-derive", "0.1.8"): set(),
    ("zerotrie", "0.2.5"): {"yoke", "zerofrom"},
    ("zerovec", "0.11.8"): {"derive", "yoke"},
    ("zerovec-derive", "0.11.6"): set(),
}
SQLX_PACKAGE_NAMES = {"sqlx", "allocator-api2", "atoi", "autocfg", "base64", "bitflags", "block-buffer", "bytes", "cc", "cfg-if", "cpufeatures", "crc", "crc-catalog", "crossbeam-queue", "crossbeam-utils", "crypto-common", "digest", "displaydoc", "dotenvy", "either", "equivalent", "event-listener", "find-msvc-tools", "flume", "foldhash", "form_urlencoded", "futures-channel", "futures-core", "futures-executor", "futures-intrusive", "futures-io", "futures-sink", "futures-task", "futures-util", "generic-array", "hashbrown", "hashlink", "heck", "hex", "hybrid-array", "icu_collections", "icu_locale_core", "icu_normalizer", "icu_normalizer_data", "icu_properties", "icu_properties_data", "icu_provider", "idna", "idna_adapter", "indexmap", "itoa", "libc", "libsqlite3-sys", "litemap", "lock_api", "log", "memchr", "mio", "num-traits", "once_cell", "parking", "parking_lot", "parking_lot_core", "percent-encoding", "pin-project-lite", "pkg-config", "potential_utf", "proc-macro2", "quote", "redox_syscall", "scopeguard", "serde", "serde_core", "serde_derive", "serde_json", "sha2", "shlex", "slab", "smallvec", "socket2", "spin", "sqlx-core", "sqlx-macros", "sqlx-macros-core", "sqlx-sqlite", "stable_deref_trait", "syn", "synstructure", "thiserror", "thiserror-impl", "tinystr", "tokio", "tokio-macros", "tokio-stream", "tracing", "tracing-attributes", "tracing-core", "typenum", "unicode-ident", "url", "utf8_iter", "vcpkg", "version_check", "wasi", "windows-link", "windows-sys", "writeable", "yoke", "yoke-derive", "zerofrom", "zerofrom-derive", "zerotrie", "zerovec", "zerovec-derive", "zmij"}
EXTERNAL["fluzo-runtime"] |= SQLX_PACKAGE_NAMES
EXTERNAL["fluzo-cli"] |= SQLX_PACKAGE_NAMES
DIRECT_EXTERNAL["fluzo-runtime"] |= {"sqlx"}
DIRECT_EXTERNAL["fluzo-runtime"] |= {"sha2"}  # DAT-02a artifact content hashing
DIRECT_EXTERNAL["fluzo-runtime"] |= {"rustix"}  # DAT-02b free-space reserve probe
# DAT-02b: rustix and its already-locked closure. Only `fs`/`std`
# features are activated on the crate crossterm already pulled in; no new
# package enters the workspace. The windows-targets tail is walked by the
# graph checker even though it never compiles on the Linux target.
DAT02B_PACKAGES = {
    "rustix",
    "errno",
    "linux-raw-sys",
    "windows-targets",
    "windows_aarch64_gnullvm",
    "windows_aarch64_msvc",
    "windows_i686_gnu",
    "windows_i686_gnullvm",
    "windows_i686_msvc",
    "windows_x86_64_gnu",
    "windows_x86_64_gnullvm",
    "windows_x86_64_msvc",
}
EXTERNAL["fluzo-runtime"] |= DAT02B_PACKAGES
EXTERNAL["fluzo-cli"] |= DAT02B_PACKAGES
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
    ("either", "1.18.0"): {"default", "serde", "std", "use_std"},
    ("errno", "0.3.14"): {"default", "std"},
    ("foldhash", "0.1.5"): set(),
    ("hashbrown", "0.15.5"): {"allocator-api2", "default", "default-hasher", "equivalent", "inline-more", "raw-entry"},
    ("heck", "0.5.0"): set(),
    ("hybrid-array", "0.4.15"): set(),  # DAT-02a sha2 0.11 closure (crypto-common 0.2 dep)
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
    # DAT-02b adds `fs` for the statvfs free-space reserve probe. The
    # crate was already locked transitively via crossterm; no new package
    # or closure entry is introduced.
    ("rustix", "0.38.44"): {"alloc", "fs", "libc-extra-traits", "std", "stdio", "termios"},
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
    ("syn", "2.0.119"): {"clone-impls", "default", "derive", "extra-traits", "full", "parsing", "printing", "proc-macro", "visit-mut"},
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
            elif identity in SQLX_PACKAGES:
                allowed_features = SQLX_PACKAGES[identity]
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
    expected = (
        "lsp add rust-analyzer --command rustup --args run --args 1.98.0 "
        "--args rust-analyzer --filetypes rust --root-markers Cargo.toml\n"
        "mcp add tui-test --type stdio --command uvx "
        "--args git+https://github.com/GeorgePearse/mcp-tui-test@bbdd9003eaefd0609d86707abdb5d32d1540b25c "
        "--args mcp-tui-test\n"
    )
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
