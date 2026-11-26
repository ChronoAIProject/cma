#!/usr/bin/env python3
"""The CI campaign runs only the reviewed, still-matching reader identities."""
import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr

import migration_campaign_pins as pins


def document():
    return pins.load()


class Registry:
    def __init__(self, rows):
        self.rows = rows
        self.calls = []

    def manifest(self, repository, version):
        self.calls.append((repository, version))
        row = self.rows.get(version)
        if isinstance(row, Exception):
            raise row
        return row


def matching(value):
    return {row["release"].split("/v", 1)[1]: (row["image"].split("@", 1)[1], {"config": {"digest": row["config"]}})
            for row in value["readers"]}


def tags(value):
    return {row["release"]: row["source"] for row in value["readers"]}.get


class CampaignPinTests(unittest.TestCase):
    def write(self, value):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "pins.json"
        path.write_text(json.dumps(value))
        return path

    def test_reviewed_pins_are_well_formed_and_name_two_distinct_releases(self):
        value = document()
        self.assertEqual([row["role"] for row in value["readers"]], ["old", "new"])
        self.assertEqual(len(pins.images(value)), 5)
        self.assertTrue(all("@sha256:" in image for image in pins.images(value)))

    def test_malformed_pins_are_refused_by_name(self):
        cases = {
            "tag instead of digest": lambda v: v["readers"][0].update(image=pins.REPOSITORY + ":0.20.0"),
            "foreign repository": lambda v: v["readers"][0].update(
                image="ghcr.io/example-owner/cma-agent-manager@sha256:" + "1" * 64),
            "short source": lambda v: v["readers"][1].update(source="b66b3d4d06"),
            "non-release tag": lambda v: v["readers"][1].update(release="cma-agent-manager/pending/v0.22.0"),
            "same release twice": lambda v: v["readers"].__setitem__(1, dict(v["readers"][0], role="new")),
            "role order": lambda v: v["readers"].reverse(),
            "extra field": lambda v: v.update(latest=True),
            "unpinned node": lambda v: v["kind"].update(node="kindest/node:v1.34.0"),
            "unpinned kind binary": lambda v: v["kind"].update(linux_amd64_sha256="latest"),
            "unreviewed identity image": lambda v: v.update(identity="docker.io/library/python@sha256:" + "2" * 64),
            "unpinned mongo": lambda v: v.update(mongo="docker.io/library/mongo:8.0"),
        }
        for name, change in cases.items():
            with self.subTest(name=name):
                value = copy.deepcopy(document())
                change(value)
                with self.assertRaises(pins.PinError) as raised:
                    pins.load(self.write(value))
                self.assertEqual(raised.exception.stage, "pins")

    def test_verify_requires_both_tags_of_record_to_name_the_pin(self):
        value = document()
        registry = Registry(matching(value))
        self.assertEqual(pins.verify(value, registry, tags(value)),
                         ["cma-agent-manager/v0.20.0", "cma-agent-manager/v0.22.0"])
        self.assertEqual(registry.calls, [(pins.REPOSITORY, "0.20.0"), (pins.REPOSITORY, "0.22.0")])

    def test_a_moved_registry_tag_or_config_is_refused(self):
        value = document()
        for name, change in {
            "digest": lambda rows: rows.update({"0.22.0": ("sha256:" + "9" * 64, rows["0.22.0"][1])}),
            "config": lambda rows: rows.update({"0.20.0": (rows["0.20.0"][0], {"config": {"digest": "sha256:" + "8" * 64}})}),
            "absent": lambda rows: rows.update({"0.22.0": None}),
            "unreadable": lambda rows: rows.update({"0.20.0": RuntimeError("Registry request failed")}),
        }.items():
            with self.subTest(name=name):
                rows = matching(value)
                change(rows)
                with self.assertRaises(pins.PinError) as raised:
                    pins.verify(value, Registry(rows), tags(value))
                self.assertEqual(raised.exception.stage, "registry")

    def test_a_moved_or_unreadable_git_tag_is_refused_before_the_registry(self):
        value = document()
        registry = Registry(matching(value))
        with self.assertRaises(pins.PinError) as raised:
            pins.verify(value, registry, lambda release: "c" * 40)
        self.assertEqual(raised.exception.stage, "git-tag")
        self.assertEqual(registry.calls, [])

        def unreadable(release):
            raise subprocess.TimeoutExpired("git", 60)
        with self.assertRaises(pins.PinError) as raised:
            pins.verify(value, registry, unreadable)
        self.assertEqual(raised.exception.stage, "git-tag")

    def test_environment_exports_only_validated_identities(self):
        lines = pins.environment(document()).splitlines()
        keys = [line.split("=", 1)[0] for line in lines]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertTrue(all(key.startswith("CAMPAIGN_") for key in keys))
        self.assertTrue(all("\n" not in line and " " not in line for line in lines))

    def test_refusal_is_one_named_actionable_error_line(self):
        value = copy.deepcopy(document())
        value["kind"]["node"] = "kindest/node:latest"
        stderr = io.StringIO()
        with redirect_stderr(stderr), self.assertRaises(SystemExit):
            pins.main(["check", "--pins", str(self.write(value))])
        self.assertRegex(stderr.getvalue(),
                         r"^::error title=Migration campaign pins::kind node image must be pinned by digest\. Review ")


if __name__ == "__main__":
    unittest.main()
