"""Reusable tool layers exclude application inputs; linked copies keep selection."""

import json
from pathlib import Path
import unittest

from docker_inputs import stage_sources


class RuntimeLayerInputsTests(unittest.TestCase):
    def test_link_flag_does_not_hide_local_or_transitive_inputs(self):
        for flag in ("--link", "--link=true", "--link=false"):
            recipe = f"FROM scratch AS deps\nCOPY {flag} package-lock.json /lock\nFROM scratch AS app\nCOPY {flag} --from=deps /lock /lock\nCOPY {flag} app.js /app.js\n"
            self.assertEqual(stage_sources(recipe, "app", {}), {"package-lock.json", "app.js"})
        for flag in ("--link=maybe", "--unknown"):
            with self.assertRaises(ValueError):
                stage_sources(f"FROM scratch\nCOPY {flag} app.js /app.js\n", "", {})

    def test_current_tool_payloads_have_no_supervisor_or_desktop_asset_inputs(self):
        root = Path(__file__).resolve().parents[3]
        recipe = (root / "api/crates/layer4-runtime/Dockerfile").read_text()
        args = {"OMARCHY_CMA_BASE": "runtime-tools", "RUNTIME_DESKTOP": "1"}
        for target in ("runtime-tool-payload", "omarchy-tool-payload"):
            sources = stage_sources(recipe, target, args)
            self.assertNotIn("api", sources, "tools must not depend on the whole Rust workspace")
            self.assertFalse(any("layer4-runtime/src/" in path for path in sources))
            self.assertIn("api/crates/layer4-runtime/image/provider", sources)
            self.assertIn("api/crates/layer4-runtime/image/npm", sources)
        for target in ("default-runtime", "omarchy"):
            sources = stage_sources(recipe, target, args)
            self.assertIn("api", sources)
            self.assertIn("api/crates/layer4-runtime/src/browser/command.cjs", sources)

    def test_tool_manifests_lock_the_reviewed_versions_and_transitive_payloads(self):
        root = Path(__file__).resolve().parents[3] / "api/crates/layer4-runtime/image"
        for variant in ("provider", "desktop-provider", "npm"):
            manifest = json.loads((root / variant / "package.json").read_text())
            lock = json.loads((root / variant / "package-lock.json").read_text())
            self.assertEqual(lock["lockfileVersion"], 3)
            self.assertEqual(manifest["dependencies"], lock["packages"][""]["dependencies"])
            for name, entry in lock["packages"].items():
                if not name or entry.get("inBundle"):
                    continue
                self.assertTrue(entry["resolved"].startswith("https://registry.npmjs.org/"))
                self.assertRegex(entry["integrity"], r"^sha512-[A-Za-z0-9+/]+={0,2}$")
            for name, version in manifest["dependencies"].items():
                self.assertEqual(lock["packages"]["node_modules/" + name]["version"], version)


if __name__ == "__main__":
    unittest.main()
