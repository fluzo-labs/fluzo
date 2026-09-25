import copy
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
    def test_installed_skills_are_valid(self):
        validate_skills()

    def test_broken_reference_is_detected(self):
        with tempfile.TemporaryDirectory(prefix="fluzo-skills-") as temporary:
            root = Path(temporary)
            shutil.copytree(ROOT / ".agents", root / ".agents")
            skill = root / ".agents/skills/rust-review/SKILL.md"
            skill.write_text(skill.read_text() + "\n[missing](missing.md)\n")
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
