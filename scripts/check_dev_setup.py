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
SKILLS = {"rust-practices", "rust-review", "fluzo-rust-boundaries", "fluzo-deterministic-testing"}


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
        pending = [identity]
        visited = set()
        while pending:
            current = pending.pop()
            if current in visited:
                continue
            visited.add(current)
            for dependency in nodes[current]["deps"]:
                if not any(kind["kind"] in (None, "build") for kind in dependency["dep_kinds"]):
                    continue
                target = dependency["pkg"]
                name = packages[target]["name"]
                if name not in ALLOWED[package["name"]]:
                    raise ValueError(f"Forbidden production path: {package['name']} -> {name}")
                pending.append(target)
    for node in nodes.values():
        if node["features"]:
            raise ValueError("New feature combinations require an explicit graph policy review")


def validate_skills(root=ROOT):
    directory = root / ".agents/skills"
    entries = sorted(directory.glob("*/SKILL.md"))
    if {entry.parent.name for entry in entries} != SKILLS:
        raise ValueError("Unexpected skill inventory")
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
    print("Resolved bootstrap graph, toolchain, LSP config and four skills checked.")


if __name__ == "__main__":
    main()
