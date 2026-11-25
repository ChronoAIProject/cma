#!/usr/bin/env python3
"""Inventory regressions; no pinned Git history, Mongo or service image needed."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import migration_inventory as inventory


OLD, NEW = "a" * 40, "b" * 40


def sources():
    return {
        "store/isolation.rs": '''
const COLLECTIONS: &[&str] = &["agents", super::usage::COLLECTION,
    crate::document_artifacts::QUOTA_COLLECTION];
const MIGRATION_COLLECTION: &str = "agent_manager_migrations";
''',
        "store/isolation/replicas.rs": '''
const COLLECTIONS: &[&str] = &[crate::store::ownership::MEMBERS,
    crate::store::ownership::OWNERS];
''',
        "store/usage.rs": 'const COLLECTION: &str = "sandbox_usage";',
        "store/ownership/mod.rs": '''
const MEMBERS: &str = "cma_agent_manager_members";
const OWNERS: &str = "cma_agent_manager_owners";
''',
        "document_artifacts/mod.rs": 'pub(crate) use quota::{Store, QUOTA_COLLECTION};',
        "document_artifacts/quota.rs": 'const QUOTA_COLLECTION: &str = "document_quotas";',
        "store/provenance_progress/model.rs": 'const COLLECTION: &str = "agent_provenance_progress";',
        "store/gateway_records.rs": '''
const GATEWAY_CHANNEL_RECORDS: &str = "gateway_channel_records";
Self::open_namespace(client, database, "gateway_connection_records").await
''',
        "store/gateway_interactions.rs": 'const COLLECTION: &str = "gateway_interaction_records";',
    }


class GitFixture:
    def __init__(self):
        self.files = {OLD: sources(), NEW: sources()}
        self.calls = []

    def __call__(self, command, **kwargs):
        self.calls.append(command)
        args = command[3:]
        if args[0] == "rev-parse":
            return args[1][:40] + "\n"
        revision, path = args[1].split(":", 1)
        try:
            return self.files[revision][path.removeprefix(inventory.ROOT)]
        except KeyError:
            raise subprocess.CalledProcessError(128, command) from None


class MigrationInventoryTests(unittest.TestCase):
    def setUp(self):
        self.git = GitFixture()
        mock = patch.object(subprocess, "check_output", self.git)
        mock.start()
        self.addCleanup(mock.stop)

    def source(self, revision=NEW):
        return inventory.Source("/unused-local-repo", revision)

    def test_discovers_literals_constants_reexports_replica_and_separate_roles(self):
        rows = self.source().collections()
        self.assertEqual(set(rows), {
            "agents", "sandbox_usage", "document_quotas", "cma_agent_manager_members",
            "cma_agent_manager_owners", "agent_manager_migrations", "agent_provenance_progress",
            "gateway_channel_records", "gateway_connection_records", "gateway_interaction_records",
        })
        self.assertEqual(rows["agents"]["role"], "runtime")
        self.assertEqual(rows["agent_manager_migrations"]["role"], "separate-or-retained")
        definition = rows["document_quotas"]["definition"]
        self.assertEqual(definition["path"], inventory.ROOT + "document_artifacts/quota.rs")
        self.assertEqual(definition["sha256"], inventory.digest(
            self.git.files[NEW]["document_artifacts/quota.rs"].encode()))

    def test_reporting_crate_reexports_preserve_the_original_collection_roles(self):
        files = self.git.files[NEW]
        root = inventory.EXTERNAL_ROOTS["cma_am_reporting"]
        files[root + "lib.rs"] = "pub mod usage; pub use store::{allocation_buckets, ReportingStore};"
        files[root + "usage.rs"] = 'pub const COLLECTION: &str = "sandbox_usage";'
        files[root + "store/allocation_buckets.rs"] = 'pub const QUOTA_COLLECTION: &str = "document_quotas";'
        files["store/usage.rs"] = "use cma_am_reporting::usage; pub use usage::{Reading, COLLECTION};"
        files["document_artifacts/mod.rs"] = "pub use cma_am_reporting::allocation_buckets::QUOTA_COLLECTION;"
        rows = {row["name"]: row for row in inventory.compare(self.source(OLD), self.source())["collections"]}
        for name in ("sandbox_usage", "document_quotas"):
            with self.subTest(name=name):
                self.assertEqual(rows[name]["new"]["role"], rows[name]["old"]["role"])
                self.assertTrue(rows[name]["new"]["definition"]["path"].startswith(root))
        del files[root + "store/allocation_buckets.rs"]
        with self.assertRaisesRegex(ValueError, "missing source module"):
            self.source().collections()

    def test_nested_test_import_does_not_replace_a_registry_binding(self):
        files = self.git.files[NEW]
        files["store/isolation.rs"] = """
use super::usage;
const COLLECTIONS: &[&str] = &[usage::COLLECTION];
const MIGRATION_COLLECTION: &str = "agent_manager_migrations";
const NOTE: &str = r#"the delimiter { is part of a string"#;
/* Block comment with a nested /* } */ brace. */
#[cfg(test)]
mod tests {
use crate::fixtures::usage;
}
"""
        files["fixtures/usage.rs"] = 'const COLLECTION: &str = "fixture_usage";'
        rows = self.source().collections()
        self.assertNotIn("fixture_usage", rows)
        definition = rows["sandbox_usage"]["definition"]
        self.assertEqual(definition["path"], inventory.ROOT + "store/usage.rs")

    def test_added_and_removed_collections_are_retained_in_both_direction_plan(self):
        self.git.files[NEW]["store/usage.rs"] = 'const COLLECTION: &str = "new_usage";'
        rows = {r["name"]: r for r in inventory.compare(self.source(OLD), self.source())["collections"]}
        self.assertIsNone(rows["new_usage"]["old"])
        self.assertIsNone(rows["sandbox_usage"]["new"])
        for row in rows.values():
            self.assertEqual(row["old_to_new"], "NOT_RUN")
            self.assertEqual(row["new_to_old"], "NOT_RUN")

    def test_imported_registry_module_does_not_hide_a_new_namespace(self):
        for files in self.git.files.values():
            files["store/isolation.rs"] = '''
use super::retained_consents;
const COLLECTIONS: &[&str] = &[retained_consents::COLLECTION];
const MIGRATION_COLLECTION: &str = "agent_manager_migrations";
'''
            files["store/retained_consents.rs"] = 'const COLLECTION: &str = "retained_consents";'
        self.git.files[NEW]["store/isolation.rs"] = self.git.files[NEW]["store/isolation.rs"].replace(
            "retained_consents::COLLECTION]", "retained_consents::COLLECTION, super::measured::COLLECTION]")
        self.git.files[NEW]["store/measured.rs"] = 'const COLLECTION: &str = "measured_observations";'
        rows = inventory.compare(self.source(OLD), self.source())["collections"]
        self.assertEqual([row["name"] for row in rows if row["old"] is None], ["measured_observations"])
        self.assertFalse(any(row["new"] is None for row in rows))

    def test_unknown_expression_missing_constant_and_duplicate_cannot_drop_a_row(self):
        for expression in ('make_collection!()', 'super::absent::COLLECTION', '"agents", "agents"'):
            with self.subTest(expression=expression):
                self.git.files[NEW]["store/isolation.rs"] = (
                    'const COLLECTIONS: &[&str] = &[' + expression + '];')
                with self.assertRaises(ValueError):
                    self.source().collections()

    def test_refuses_duplicate_across_runtime_and_replica_catalogs(self):
        self.git.files[NEW]["store/ownership/mod.rs"] = '''
const MEMBERS: &str = "agents";
const OWNERS: &str = "owners";
'''
        with self.assertRaisesRegex(ValueError, "duplicate runtime/replica"):
            self.source().collections()

    def test_missing_registry_or_retained_namespace_fails_closed(self):
        for path in ("store/isolation.rs", "store/gateway_records.rs"):
            with self.subTest(path=path):
                saved = self.git.files[NEW][path]
                self.git.files[NEW][path] = ""
                with self.assertRaises(ValueError):
                    self.source().collections()
                self.git.files[NEW][path] = saved

    def test_reexport_cycles_and_paths_escaping_the_crate_fail(self):
        self.git.files[NEW]["document_artifacts/quota.rs"] = (
            'use crate::document_artifacts::QUOTA_COLLECTION;')
        with self.assertRaisesRegex(ValueError, "cyclic constant"):
            self.source().collections()
        with self.assertRaisesRegex(ValueError, "escapes crate"):
            inventory.Source.resolve("store", "super::super::NAME")
        self.assertEqual(inventory.Source.resolve("store", "self::NAME"), "store::NAME")

    def test_mutable_or_abbreviated_source_is_refused_before_git(self):
        for value in ("HEAD", "develop", "abc123", "-" * 40, "A" * 40):
            with self.assertRaises(ValueError):
                self.source(value)
        self.assertEqual(self.git.calls, [])

    def test_read_cache_preserves_one_snapshot(self):
        source = self.source()
        path = inventory.ROOT + "store/isolation.rs"
        first = source.read(path)
        self.git.files[NEW]["store/isolation.rs"] = "changed"
        self.assertEqual(source.read(path), first)
        self.assertEqual(len(self.git.calls), 2)

    def test_cli_writes_identity_once_and_never_certifies_execution(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            output = Path(directory) / "inventory.json"
            args = ["--repo", ".", "--old", OLD, "--new", NEW, "--output", str(output)]
            with contextlib.redirect_stdout(io.StringIO()) as terminal:
                inventory.main(args)
            receipt = json.loads(terminal.getvalue())
            self.assertEqual(receipt["execution"], "NOT_RUN")
            self.assertEqual(receipt["collections"], 10)
            self.assertEqual(receipt["sha256"], inventory.digest(output.read_bytes()))
            result = json.loads(output.read_text())
            self.assertEqual(result["old"]["source"], OLD)
            self.assertEqual(result["new"]["source"], NEW)
            original = output.read_bytes()
            with self.assertRaises(FileExistsError):
                inventory.main(args)
            self.assertEqual(output.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
