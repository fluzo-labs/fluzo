import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from check_dev_setup import ROOT, validate_graph, validate_skills


class BoundaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
            cwd=ROOT,
            text=True,
        ))

    def test_current_graph_is_valid(self):
        validate_graph(self.metadata)

    def test_forbidden_edges_are_detected(self):
        for source, target in [
            ("fluzo-tui", "fluzo-runtime"),
            ("fluzo-runtime", "fluzo-tui"),
            ("fluzo-core", "fluzo-cli"),
        ]:
            with self.subTest(source=source, target=target):
                metadata = copy.deepcopy(self.metadata)
                identities = {package["name"]: package["id"] for package in metadata["packages"]}
                node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identities[source])
                node["deps"].append({"pkg": identities[target], "dep_kinds": [{"kind": None}]})
                with self.assertRaisesRegex(ValueError, "Forbidden production path"):
                    validate_graph(metadata)

    def test_simulator_dependencies_cannot_become_production_or_tui_edges(self):
        for source, target, kind in [
            ("fluzo-runtime", "hyper", None),
            ("fluzo-runtime", "tokio", "build"),
            ("fluzo-tui", "hyper", "dev"),
            ("fluzo-core", "serde_json", "dev"),
        ]:
            with self.subTest(source=source, target=target, kind=kind):
                metadata = copy.deepcopy(self.metadata)
                identities = {package["name"]: package["id"] for package in metadata["packages"]}
                node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identities[source])
                node["deps"].append({"pkg": identities[target], "dep_kinds": [{"kind": kind, "target": "cfg(unix)"}]})
                with self.assertRaisesRegex(ValueError, "Forbidden production path|Unreviewed test dependency"):
                    validate_graph(metadata)

    def test_unreviewed_http_features_and_versions_fail(self):
        for mutation in ["http2", "version"]:
            metadata = copy.deepcopy(self.metadata)
            package = next(package for package in metadata["packages"] if package["name"] == "hyper")
            if mutation == "version":
                package["version"] = "999.0.0"
            else:
                node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == package["id"])
                node["features"].append("http2")
            with self.assertRaisesRegex(ValueError, "feature combinations|Unreviewed package source/version"):
                validate_graph(metadata)

    def test_terminal_libraries_remain_outside_core_and_runtime(self):
        for source in ["fluzo-core", "fluzo-runtime"]:
            metadata = copy.deepcopy(self.metadata)
            identities = {package["name"]: package["id"] for package in metadata["packages"]}
            node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identities[source])
            node["deps"].append({"pkg": identities["ratatui"], "dep_kinds": [{"kind": "build", "target": "cfg(unix)"}]})
            with self.assertRaisesRegex(ValueError, "Forbidden production path"):
                validate_graph(metadata)

    def test_terminal_duplicate_versions_are_checked_independently(self):
        for name in ["syn", "unicode-width", "windows-sys", "hashbrown"]:
            for original in [item for item in self.metadata["packages"] if item["name"] == name]:
                metadata = copy.deepcopy(self.metadata)
                package = next(item for item in metadata["packages"] if item["id"] == original["id"])
                package["version"] = "999.0.0"
                with self.assertRaisesRegex(ValueError, "Unreviewed package source/version"):
                    validate_graph(metadata)

    def test_external_transitive_dependency_is_detected(self):
        metadata = copy.deepcopy(self.metadata)
        metadata["packages"].append({"id": "external", "name": "external-io"})
        core = next(package["id"] for package in metadata["packages"] if package["name"] == "fluzo-core")
        node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == core)
        node["deps"].append({"pkg": "external", "dep_kinds": [{"kind": None}]})
        with self.assertRaisesRegex(ValueError, "Forbidden production path"):
            validate_graph(metadata)

    def test_unreviewed_features_are_detected(self):
        metadata = copy.deepcopy(self.metadata)
        metadata["resolve"]["nodes"][0]["features"] = ["new-feature"]
        with self.assertRaisesRegex(ValueError, "feature combinations"):
            validate_graph(metadata)

    def test_toml_stays_outside_core_and_tui(self):
        for source in ["fluzo-core", "fluzo-tui"]:
            metadata = copy.deepcopy(self.metadata)
            identities = {package["name"]: package["id"] for package in metadata["packages"]}
            node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identities[source])
            node["deps"].append({"pkg": identities["toml_edit"], "dep_kinds": [{"kind": None, "target": "cfg(unix)"}]})
            with self.assertRaisesRegex(ValueError, "Forbidden production path"):
                validate_graph(metadata)

    def test_transitive_fluzo_coupling_through_external_package_is_rejected(self):
        metadata = copy.deepcopy(self.metadata)
        identities = {package["name"]: package["id"] for package in metadata["packages"]}
        node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identities["serde"])
        node["deps"].append({"pkg": identities["fluzo-runtime"], "dep_kinds": [{"kind": "build"}]})
        with self.assertRaisesRegex(ValueError, "Forbidden production path: fluzo-core -> serde -> fluzo-runtime"):
            validate_graph(metadata)

    def test_dependency_source_version_and_features_are_reviewed(self):
        for field, value in [("version", "999.0.0"), ("source", "git+https://example.invalid/unreviewed")]:
            metadata = copy.deepcopy(self.metadata)
            package = next(package for package in metadata["packages"] if package["name"] == "serde")
            package[field] = value
            with self.assertRaisesRegex(ValueError, "Unreviewed package source/version"):
                validate_graph(metadata)
        metadata = copy.deepcopy(self.metadata)
        identity = next(package["id"] for package in metadata["packages"] if package["name"] == "toml_edit")
        node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == identity)
        node["features"].append("unbounded")
        with self.assertRaisesRegex(ValueError, "feature combinations"):
            validate_graph(metadata)

    def test_inactive_cargo_features_are_detected(self):
        declarations = [
            '\n[features]\nunreviewed = []\n',
            '\n[dependencies.fluzo-runtime]\npath = "../fluzo-runtime"\noptional = true\n',
            '\n[dependencies.fluzo-runtime]\npath = "../fluzo-runtime"\noptional = true\n'
            '\n[features]\nunreviewed = ["dep:fluzo-runtime"]\n',
        ]
        for declaration in declarations:
            with self.subTest(declaration=declaration):
                with tempfile.TemporaryDirectory(prefix="fluzo-feature-") as temporary:
                    root = Path(temporary)
                    for name in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]:
                        shutil.copy2(ROOT / name, root / name)
                    shutil.copytree(ROOT / "crates", root / "crates")
                    manifest = root / "crates/fluzo-tui/Cargo.toml"
                    manifest.write_text(manifest.read_text() + declaration)
                    subprocess.run(["cargo", "generate-lockfile", "--offline"],
                                   cwd=root, check=True, capture_output=True, timeout=30)
                    metadata = json.loads(subprocess.check_output(
                        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
                        cwd=root, text=True, timeout=30,
                    ))
                    self.assertTrue(all(not node["features"] for node in metadata["resolve"]["nodes"]
                                        if node["id"] in metadata["workspace_members"]))
                    with self.assertRaisesRegex(ValueError, "feature declarations"):
                        validate_graph(metadata)


class SkillTests(unittest.TestCase):
    def test_common_skill_changes_are_detected(self):
        mutations = ["content", "missing-reference", "extra-file", "revision", "license", "symlink"]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                with tempfile.TemporaryDirectory(prefix="fluzo-common-skills-") as temporary:
                    root = Path(temporary)
                    shutil.copytree(ROOT / ".agents", root / ".agents")
                    folder = root / ".agents/skills/plan-create"
                    if mutation == "content":
                        path = folder / "SKILL.md"
                        path.write_text(path.read_text() + "\nUnreviewed change.\n")
                    elif mutation == "missing-reference":
                        (folder / "references/plan-template.md").unlink()
                    elif mutation == "extra-file":
                        (folder / "unexpected.md").write_text("Unexpected resource\n")
                    elif mutation == "revision":
                        path = root / ".agents/common-skills.toml"
                        path.write_text(path.read_text().replace(
                            "7c366791aa23715e7bb772e5d9d2dc4acebecc04", "0" * 40))
                    elif mutation == "license":
                        (folder / "LICENSE").unlink()
                    else:
                        path = folder / "references/plan-template.md"
                        original = path.read_bytes()
                        path.unlink()
                        target = root / "outside.md"
                        target.write_bytes(original)
                        path.symlink_to(target)
                    with self.assertRaises(ValueError):
                        validate_skills(root)

    def test_fluzo_skill_changes_are_detected(self):
        for name in ["fluzo-deterministic-testing", "fluzo-rust-boundaries", "tui-design", "rust-practices", "rust-review"]:
            for mutation in ["content", "missing-reference", "license", "extra-file", "revision", "symlink"]:
                with self.subTest(skill=name, mutation=mutation):
                    with tempfile.TemporaryDirectory(prefix="fluzo-collection-") as temporary:
                        root = Path(temporary)
                        shutil.copytree(ROOT / ".agents", root / ".agents")
                        folder = root / ".agents/skills" / name
                        reference = next((folder / "references").glob("*.md"))
                        if mutation == "content":
                            path = folder / "SKILL.md"
                            path.write_text(path.read_text() + "\nSuperseded local instructions.\n")
                        elif mutation == "missing-reference":
                            reference.unlink()
                        elif mutation == "license":
                            (folder / "LICENSE").unlink()
                        elif mutation == "extra-file":
                            (folder / "stale.md").write_text("Old resource\n")
                        elif mutation == "revision":
                            path = root / ".agents/fluzo-skills.toml"
                            path.write_text(path.read_text().replace(
                                "48a1ac36fc229ccbcb1fe671d5dbd9774a567d56", "0" * 40))
                        else:
                            target = root / "outside.md"
                            target.write_bytes(reference.read_bytes())
                            reference.unlink()
                            reference.symlink_to(target)
                        with self.assertRaises(ValueError):
                            validate_skills(root)

    def test_installed_skills_are_valid(self):
        validate_skills()

    def test_broken_reference_is_detected(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-skills-") as temporary:
            root = Path(temporary)
            shutil.copytree(ROOT / ".agents", root / ".agents")
            skill = root / ".agents/skills/rust-review/SKILL.md"
            old_digest = hashlib.sha256(skill.read_bytes()).hexdigest()
            skill.write_text(skill.read_text() + "\n[missing](missing.md)\n")
            manifest = root / ".agents/fluzo-skills.toml"
            manifest.write_text(manifest.read_text().replace(
                old_digest, hashlib.sha256(skill.read_bytes()).hexdigest()))
            with self.assertRaisesRegex(ValueError, "Broken skill reference"):
                validate_skills(root)

    def test_missing_license_is_detected(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-skills-") as temporary:
            root = Path(temporary)
            shutil.copytree(ROOT / ".agents", root / ".agents")
            (root / ".agents/skills/rust-review/LICENSE").unlink()
            with self.assertRaises(ValueError):
                validate_skills(root)


if __name__ == "__main__":
    unittest.main()
