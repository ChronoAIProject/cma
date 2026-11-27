import hashlib
import io
import json
import os
import tarfile
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

import yaml

import buildkit_cache_archive as archive
from actions_cache_retention import BUDGET
from actions_cargo_budget import Account as CargoAccount, SCHEMA as CARGO_SCHEMA, stored_bound
from actions_layer_admission import Account, PREFIX, reserve, retire_verified, select, select_am_receipt, source_run
from staging_cd_cache_retention_tests import Inventory, entry
from staging_cd_cache_retention_tests import ROOT


def blob(root, payload, media):
    encoded = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
    digest = hashlib.sha256(encoded).hexdigest()
    (root / "blobs" / "sha256" / digest).write_bytes(encoded)
    return {"digest": "sha256:" + digest, "size": len(encoded), "mediaType": media}


def populate(root):
    (root / "blobs" / "sha256").mkdir(parents=True)
    (root / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')
    data = blob(root, b"synthetic layer", "application/vnd.oci.image.layer.v1.tar+gzip")
    config = blob(root, {"layers": [{"blob": data["digest"], "parent": -1}],
                         "records": [{"digest": "sha256:" + "b" * 64, "layers": [{"layer": 0}]}]},
                  "application/vnd.buildkit.cacheconfig.v0")
    manifest = blob(root, {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                           "config": config, "layers": [data]},
                    "application/vnd.oci.image.manifest.v1+json")
    manifest["annotations"] = {"org.opencontainers.image.ref.name": "latest"}
    (root / "index.json").write_text(json.dumps({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json",
                                                 "manifests": [manifest]}))
    return data


class ArchiveTests(unittest.TestCase):
    def test_verified_oci_cache_round_trip(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            populate(root / "source")
            (root / "source" / "cache.lock").touch()
            archive.seal(root / "source", root / "archive.tar")
            archive.restore(root / "archive.tar", root / "restored")
            self.assertEqual(archive.inventory(root / "source"), archive.inventory(root / "restored"))
            self.assertFalse((root / "restored" / "cache.lock").exists())

    def test_buildkit_ingest_locks_are_validated_and_omitted(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            populate(source)
            locks = source / "ingest-locks"
            locks.mkdir()
            lock = locks / ("a" * 64)
            lock.touch()
            stored = root / "archive.tar"
            archive.seal(source, stored)
            archive.restore(stored, root / "restored")
            self.assertFalse((root / "restored" / "ingest-locks").exists())
            self.assertEqual(archive.inventory(source), archive.inventory(root / "restored"))
            lock.write_text("unexpected content")
            with self.assertRaisesRegex(ValueError, "unexpected cache lock"):
                archive.inventory(source)
            lock.unlink()
            lock.symlink_to(source / "index.json")
            with self.assertRaisesRegex(ValueError, "unexpected cache lock"):
                archive.inventory(source)
            lock.unlink()
            os.link(source / "index.json", lock)
            with self.assertRaisesRegex(ValueError, "unexpected cache lock|unexpected cache member"):
                archive.inventory(source)
            lock.unlink()
            lock.touch()
            with patch.object(archive, "MAX_MEMBERS", len(archive.inventory(source))), self.assertRaisesRegex(ValueError, "member bound"):
                archive.inventory(source)
            lock.unlink()
            (locks / "unknown").touch()
            with self.assertRaisesRegex(ValueError, "unexpected cache member"):
                archive.inventory(source)

    def test_missing_or_changed_layer_prevents_save(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            data = populate(root / "source")
            path = root / "source" / "blobs" / "sha256" / data["digest"][7:]
            original = path.read_bytes()
            for payload in (None, b"changed layer!!"):
                path.unlink()
                if payload is not None:
                    path.write_bytes(payload)
                with self.assertRaises(ValueError):
                    archive.seal(root / "source", root / "archive.tar")
                self.assertFalse((root / "archive.tar").exists())
                if payload is None:
                    path.write_bytes(original)

    def test_member_and_final_archive_bounds(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            populate(root / "source")
            for limit in ({"MAX_MEMBERS": 4}, {"MAX_BYTES": 100}, {"MAX_BYTES": 4096}):
                with patch.multiple(archive, **limit), self.assertRaises(ValueError):
                    archive.seal(root / "source", root / "archive.tar")
                self.assertFalse((root / "archive.tar").exists())

    def test_invalid_member_leaves_no_restore(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            stored = root / "archive.tar"
            with tarfile.open(stored, "w") as output:
                member = tarfile.TarInfo("outside.txt")
                member.size = 4
                output.addfile(member, io.BytesIO(b"data"))
            with self.assertRaises(ValueError):
                archive.restore(stored, root / "restored")
            self.assertFalse((root / "restored").exists())

    def test_restore_publishes_only_after_complete_validation(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            populate(root / 'source')
            stored = root / 'archive.tar'
            archive.seal(root / 'source', stored)
            destination = root / 'restored'
            def interrupted(extracted):
                self.assertTrue((extracted / 'index.json').is_file())
                self.assertFalse(destination.exists())
                raise RuntimeError('synthetic interruption')
            with patch.object(archive, 'inventory', side_effect=interrupted), self.assertRaises(RuntimeError):
                archive.restore(stored, destination)
            self.assertFalse(destination.exists())
            archive.restore(stored, destination)
            self.assertTrue(destination.is_dir())


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.archive = Path(directory.name) / "archive.tar"
        self.archive.write_bytes(b"synthetic")
        self.cap = stored_bound(self.archive.stat().st_size)
        self.key = PREFIX + "am--123-1"
        self.old = PREFIX + "eg--122-1"
        self.account = Account({})
        self.ledger = Mock()
        self.ledger.read.return_value = ("a" * 40, self.account)

    def admission(self, *entries):
        return reserve(Inventory(*entries), self.ledger, self.key, self.archive, "c" * 40)

    def test_whole_archive_charge_must_fit_and_prior_generation_survives(self):
        old = entry(1, self.old, size=20)
        for size in (BUDGET - self.cap - 20, BUDGET - self.cap - 19, BUDGET):
            self.assertFalse(self.admission(old, entry(2, "unrelated", size=size))["allowed"])
        self.ledger.advance.assert_not_called()
        self.assertTrue(self.admission(old, entry(2, "unrelated", size=BUDGET - self.cap - 21))["allowed"])
        self.assertEqual(self.account.charges, {self.key: self.cap})
        self.ledger.advance.assert_called_once()

    def test_unknown_uploads_and_legacy_charges_are_counted(self):
        self.account.charges[self.old] = self.cap
        cargo_key = "cma-cd-cargo-v4-am-" + "a" * 64 + "-" + "b" * 64 + "-22-1"
        self.account.charges[cargo_key] = self.cap
        external = entry(3, "unrelated", size=BUDGET - 3 * self.cap)
        self.assertFalse(self.admission(external)["allowed"])
        self.assertEqual(self.account.charges, {self.old: self.cap, cargo_key: self.cap})
        self.assertFalse(self.admission(external, entry(4, self.old, size=self.cap))["allowed"])
        self.assertEqual(self.account.charges, {cargo_key: self.cap})

    def test_migration_preserves_legacy_charge_and_refuses_legacy_decoder(self):
        cargo_key = "cma-cd-cargo-v4-am-" + "a" * 64 + "-" + "b" * 64 + "-22-1"
        old = {"schema": CARGO_SCHEMA, "charges": {cargo_key: self.cap}}
        account = Account.decode(old)
        self.assertEqual(account.charges, old["charges"])
        account.charges[self.key] = self.cap
        with self.assertRaises(ValueError):
            CargoAccount.decode(account.encode())

    def test_changed_ledger_ref_or_duplicate_candidate_withholds(self):
        self.ledger.read.side_effect = [("a" * 40, self.account), ("b" * 40, Account({}))]
        self.assertFalse(self.admission()["allowed"])
        self.ledger.advance.assert_not_called()
        self.ledger.read.side_effect = None
        self.account.charges.clear()
        self.assertFalse(self.admission(entry(2, self.key))["allowed"])
        self.ledger.advance.assert_not_called()


class ProducerTests(unittest.TestCase):
    def test_detached_am_source_requires_successful_publish_and_finalize(self):
        run = {"id": 42, "run_attempt": 2, "head_sha": "a" * 40}
        artifact = {"name": "staging-receipts-42-2-am", "id": 17, "size_in_bytes": 100,
                    "expired": False, "digest": "sha256:" + "a" * 64}
        job = {"run_id": 42, "run_attempt": 2, "head_sha": "a" * 40,
               "status": "completed", "conclusion": "success"}
        publish = dict(job, name="Release am / Publish", steps=[{
            "name": "Retain only public publication receipts", "conclusion": "success"}])
        finalize = dict(job, name="Release am / Finalize", steps=[{
            "name": "Publish digest-bound final tags and remove successful reservations", "conclusion": "success"}])
        api = Mock()
        api.list.side_effect = [[artifact], [publish, finalize]]
        self.assertEqual(select_am_receipt(api, run), 17)
        for broken in ([publish], [publish, dict(finalize, conclusion="failure")],
                       [publish, dict(finalize, run_attempt=1)]):
            api.list.side_effect = [[artifact], broken]
            self.assertIsNone(select_am_receipt(api, run))

    def test_detached_am_export_is_bound_to_finalized_recipe_and_withholds_failures(self):
        from staging_cd_am_dependency_archive import export
        from staging_cd_receipts_tests import fixture_receipt
        from staging_cd_layer_reuse import read_replay
        receipt = fixture_receipt(component="am", run_id=42, run_attempt=2)
        release = receipt["identity"]
        run = {"id": 42, "run_attempt": 2, "head_sha": "a" * 40}
        with TemporaryDirectory() as directory:
            parent = Path(directory)
            path = parent / "receipts.json"
            path.write_text(json.dumps([receipt]))
            original = path.read_bytes()

            def runner(args, **_):
                if args[0] == "git":
                    return b"b" * 40
                self.assertIn("dependencies", args)
                self.assertIn("type=cacheonly", args)
                self.assertIn("linux/amd64", args)
                location = next(value.split("dest=", 1)[1] for value in args if value.startswith("type=local,"))
                populate(Path(location))
                return b""

            with patch("staging_cd_am_dependency_archive.snapshot"), \
                    patch("staging_cd_am_dependency_archive.identity", return_value=(release, {"dockerfile": "api/crates/cma/Dockerfile"}, {})):
                for problem in (OSError("disk full"), ValueError("oversize archive")):
                    with patch("staging_cd_am_dependency_archive.seal", side_effect=problem):
                        with self.assertRaises(type(problem)):
                            export(parent, parent, run, runner=runner)
                    self.assertFalse((parent / "archive.tar").exists())
                    self.assertFalse((parent / "replay.json").exists())
                    self.assertEqual(path.read_bytes(), original)
                export(parent, parent, run, runner=runner)
                value = read_replay(parent / "replay.json", sealed=True)
                self.assertEqual(value["kind"], "am-dependencies")
                self.assertEqual(value["reservation"]["source"], release["source"])
                self.assertEqual(path.read_bytes(), original)

    def test_only_exact_successful_protected_publication_is_selected(self):
        run = {"id": 42, "run_attempt": 2, "head_sha": "a" * 40}
        artifact = {"name": "cma-layer-cache-am-42-2", "id": 17, "size_in_bytes": 100,
                    "expired": False, "digest": "sha256:" + "a" * 64}
        job = {"name": "Release am / Publish", "run_id": 42, "run_attempt": 2, "head_sha": "a" * 40,
               "status": "completed", "conclusion": "success",
               "steps": [{"name": "Retain sealed layer archive for admission", "conclusion": "success"}]}
        api = Mock()
        api.list.side_effect = [[artifact], [job]]
        self.assertEqual(select(api, run, "am"), 17)
        api.list.side_effect = [[artifact], [dict(job, run_attempt=1)]]
        with self.assertRaises(ValueError):
            select(api, run, "am")
        api.list.side_effect = [[artifact], [dict(job, conclusion="failure")]]
        with self.assertRaises(ValueError):
            select(api, run, "am")

    def test_dispatch_and_nonprotected_publishers_are_refused(self):
        from staging_cd_versions import REPOSITORY
        with TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            run = {"id": 42, "run_attempt": 2, "head_sha": "a" * 40, "status": "completed",
                   "event": "push", "head_branch": "develop", "path": ".github/workflows/staging-cd.yml",
                   "head_repository": {"full_name": REPOSITORY}, "repository": {"full_name": REPOSITORY}}
            env = {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "workflow_run", "GITHUB_EVENT_PATH": str(path),
                   "GITHUB_WORKFLOW_REF": f"{REPOSITORY}/.github/workflows/cache-retention.yml@refs/heads/develop",
                   "CD_LAYER_ARCHIVE_CACHE": "enabled", "CD_LAYER_CACHE_WRITER_FLOOR": "verified"}
            path.write_text(json.dumps({"action": "completed", "workflow_run": run}))
            self.assertEqual(source_run(env), run)
            for changes in ({"GITHUB_EVENT_NAME": "workflow_dispatch"}, {"CD_LAYER_ARCHIVE_CACHE": "disabled"},
                            {"CD_LAYER_CACHE_WRITER_FLOOR": ""}):
                with self.assertRaises(ValueError):
                    source_run(env | changes)
            path.write_text(json.dumps({"action": "completed", "workflow_run": run | {"head_branch": "topic"}}))
            with self.assertRaises(ValueError):
                source_run(env)

    def test_layer_save_is_off_by_default_and_detached_from_publication(self):
        workflows = ROOT / '.github' / 'workflows'
        release = yaml.safe_load((workflows / 'staging-release.yml').read_text())
        publisher = release['jobs']['publish']
        self.assertEqual(publisher['cache-mode'], 'read')
        self.assertEqual(set(release['jobs']), {'publish', 'finalize'})
        step = next(s for s in publisher['steps'] if s.get('id') == 'seal-layer-archive')
        self.assertIn("vars.CMA_BOUNDED_LAYER_CACHE == 'true'", step['if'])
        self.assertIn('success()', step['if'])
        self.assertIn("inputs.component != 'am'", step['if'])
        workflow = yaml.safe_load((workflows / 'cache-retention.yml').read_text())
        writer = workflow['jobs']['retain_layers']
        self.assertEqual(writer['cache-mode'], 'write')
        self.assertEqual(writer['permissions'], {'contents': 'write', 'actions': 'read'})
        self.assertEqual(writer['concurrency'], {'group': 'cma-develop-layer-cache-admission',
                                               'queue': 'max', 'cancel-in-progress': False})
        self.assertIn("vars.CMA_BOUNDED_LAYER_CACHE == 'true'", writer['if'])
        self.assertIn("vars.CMA_LAYER_CACHE_WRITER_FLOOR == 'true'", writer['if'])
        self.assertIn("github.event_name == 'workflow_run'", writer['if'])
        self.assertIn("github.event.workflow_run.path == '.github/workflows/staging-cd.yml'", writer['if'])
        self.assertTrue(writer['continue-on-error'])
        self.assertTrue(any(s.get('name') == 'Reconstruct and seal qualified AM dependency stage'
                            for s in writer['steps']))
        steps = writer['steps']
        candidate = next(s for s in steps if s.get('id') == 'verify-am-candidate')
        reserve_step = next(s for s in steps if s.get('id') == 'reserve-layer-archive')
        self.assertEqual(candidate['if'], "matrix.component == 'am' && steps.layer-candidate.outputs.artifact != ''")
        self.assertIn('--candidate', candidate['run'])
        self.assertLess(steps.index(candidate), steps.index(reserve_step))
        self.assertIn("matrix.component != 'am' || steps.verify-am-candidate.outcome == 'success'", reserve_step['if'])
        self.assertEqual(reserve_step['env']['LAYER_REUSE_ARCHIVE_DIGEST'],
                         "${{ steps.verify-am-candidate.outputs.archive_digest }}")
        save = writer['steps'][-1]
        self.assertTrue(save['uses'].startswith('actions/cache/save@'))
        self.assertEqual(save['if'], "steps.reserve-layer-archive.outputs.allowed == 'true'")
        reader = workflow['jobs']['verify_layers']
        self.assertEqual(reader['needs'], 'retain_layers')
        self.assertEqual(reader['cache-mode'], 'read')
        self.assertEqual(reader['concurrency'], writer['concurrency'])
        replay_condition = "steps.verify-archive.outcome == 'success' && (matrix.component == 'am' || vars.CMA_BOUNDED_LAYER_PREDECESSOR_RETIREMENT == 'true')"
        replay = next(s for s in reader['steps'] if s.get('id') == 'verify-reuse')
        setup = next(s for s in reader['steps'] if s.get('name') == 'Set up the actual BuildKit importer')
        self.assertEqual(replay['if'], replay_condition)
        self.assertEqual(setup['if'], replay_condition)
        self.assertEqual(reader['steps'][-1]['if'],
                         "steps.verify-reuse.outcome == 'success' && vars.CMA_BOUNDED_LAYER_PREDECESSOR_RETIREMENT == 'true'")
        for step in publisher['steps']:
            if step.get('id') in ('producer', 'publication'):
                self.assertEqual(step['env']['CD_LAYER_ARCHIVE_RESTORED'], "${{ steps.layer-validate.outcome == 'success' }}")

    def test_runtime_prefix_excludes_omarchy_in_both_restore_paths(self):
        from actions_layer_admission import archive_key
        run = {'id': 123, 'run_attempt': 1}
        runtime = archive_key('runtime', run)
        omarchy = archive_key('runtime-omarchy', run)
        self.assertTrue(runtime.startswith(PREFIX + 'runtime--'))
        self.assertFalse(omarchy.startswith(PREFIX + 'runtime--'))
        for name, job in (('staging-release', 'publish'), ('ci-images', 'images')):
            document = yaml.safe_load((ROOT / '.github' / 'workflows' / (name + '.yml')).read_text())
            restore = next(step for step in document['jobs'][job]['steps'] if step.get('id') == 'layer-restore')
            self.assertTrue(restore['with']['restore-keys'].endswith(' }}--'))


class RetirementTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.restored = Path(temp.name) / 'restored'
        populate(self.restored)

    def test_only_same_family_and_version_after_fresh_successor(self):
        from dataclasses import replace
        old = entry(1, PREFIX + 'am--10-1')
        keep = entry(4, PREFIX + 'am--11-1')
        unrelated = entry(2, PREFIX + 'eg--10-1')
        another_version = replace(entry(3, PREFIX + 'am--9-1'), version='other')
        newer = entry(5, PREFIX + 'am--12-1')
        legacy = entry(6, 'buildkit-blob-1-sha256:' + 'a' * 64)
        api = Inventory(old, keep, unrelated, another_version, newer, legacy)
        api.active_runs = Mock(return_value=[{'id': 42}])
        self.assertEqual(retire_verified(api, keep.key, 'a' * 40, self.restored)['deleted_bytes'], 0)
        api.delete.assert_not_called()
        api.active_runs.return_value = []
        self.assertEqual(retire_verified(api, keep.key, 'a' * 40, self.restored)['deleted_bytes'], old.size)
        api.delete.assert_called_once_with(old.id)
        self.assertEqual(set(api.rows), {keep.id, unrelated.id, another_version.id, newer.id, legacy.id})

    def test_duplicate_or_changed_successor_refuses_retirement(self):
        from dataclasses import replace
        old = entry(1, PREFIX + 'am--10-1')
        keep = entry(4, PREFIX + 'am--11-1')
        api = Inventory(old, keep, replace(keep, id=5))
        api.active_runs = Mock(return_value=[])
        with self.assertRaises(ValueError):
            retire_verified(api, keep.key, 'a' * 40, self.restored)
        api.delete.assert_not_called()
        api.rows.pop(5)
        api.visible = Mock(return_value=None)
        self.assertEqual(retire_verified(api, keep.key, 'a' * 40, self.restored)['deleted_bytes'], 0)
        api.delete.assert_not_called()

    def test_cyclic_records_and_result_parents_cannot_retire(self):
        for record_cycle in (True, False):
            with self.subTest(record_cycle=record_cycle):
                layer = blob(self.restored, b'layer with valid digest', 'application/vnd.oci.image.layer.v1.tar+gzip')
                config = blob(self.restored, {"layers": [{"blob": layer["digest"], "parent": -1 if record_cycle else 0}],
                    "records": [{"digest": "sha256:" + "b" * 64, "layers": [{"layer": 0}],
                                 "inputs": [[{"link": 0}]] if record_cycle else []}]},
                    "application/vnd.buildkit.cacheconfig.v0")
                manifest = blob(self.restored, {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": config, "layers": [layer]}, "application/vnd.oci.image.manifest.v1+json")
                manifest["annotations"] = {"org.opencontainers.image.ref.name": "latest"}
                (self.restored / "index.json").write_text(json.dumps({"schemaVersion": 2,
                    "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": [manifest]}))
                old = entry(1, PREFIX + 'am--10-1')
                keep = entry(4, PREFIX + 'am--11-1')
                api = Inventory(old, keep)
                with self.assertRaises(ValueError):
                    retire_verified(api, keep.key, 'a' * 40, self.restored)
                api.delete.assert_not_called()

    def test_hash_valid_layer_tree_without_cache_configuration_cannot_retire(self):
        layer = blob(self.restored, b'valid hash, no cache records', 'application/vnd.oci.image.layer.v1.tar+gzip')
        layer['annotations'] = {'org.opencontainers.image.ref.name': 'latest'}
        (self.restored / 'index.json').write_text(json.dumps({'schemaVersion': 2,
            'mediaType': 'application/vnd.oci.image.index.v1+json', 'manifests': [layer]}))
        old = entry(1, PREFIX + 'am--10-1')
        keep = entry(4, PREFIX + 'am--11-1')
        api = Inventory(old, keep)
        with self.assertRaises(ValueError):
            retire_verified(api, keep.key, 'a' * 40, self.restored)
        api.delete.assert_not_called()

    def test_import_without_a_reused_build_result_cannot_authorize_retirement(self):
        from staging_cd_build_progress import summarize
        from staging_cd_layer_reuse import reused
        imported = {'digest': 'import', 'name': 'importing cache manifest from local:synthetic',
                    'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:01Z'}
        step = {'digest': 'step', 'name': '[builder 2/3] COPY fixture /fixture',
                'started': '2026-01-01T00:00:01Z', 'completed': '2026-01-01T00:00:02Z'}
        api = Inventory(entry(1, PREFIX + 'am--10-1'), entry(4, PREFIX + 'am--11-1'))
        for cached in (False, True):
            progress = summarize(json.dumps({'vertexes': [imported, step | {'cached': cached}]}).encode())
            self.assertEqual(reused(progress), cached)
            if not reused(progress):
                api.delete.assert_not_called()

    def test_replay_requires_matching_archive_and_real_build_step_reuse(self):
        from staging_cd_layer_reuse import prove, seal_replay
        from staging_cd_reservations import next_reservation
        parent = self.restored.parent
        stored = parent / 'archive.tar'
        archive.seal(self.restored, stored)
        replay = parent / 'replay.json'
        reservation = next_reservation('am', 'a' * 40, 'b' * 40, 17, 'patch', 42, 2, [], [])
        mount = 'c' * 32
        replay.write_text(json.dumps({'reservation': reservation, 'mount_namespace': mount}))
        seal_replay(replay, stored)
        env = {'CD_COMPONENT': 'am', 'RUNNER_TEMP': str(parent)}
        source = {'id': 42, 'run_attempt': 2, 'head_sha': 'd' * 40}
        with patch('actions_layer_admission.source_run', return_value=source), \
                patch('staging_cd_layer_reuse.subprocess.run', return_value=Mock(stdout='b' * 40)):
            for cached_steps in (0, 1):
                def builder(_root, actual, _directory, *, cache, progress):
                    self.assertEqual(actual, reservation)
                    self.assertEqual(cache.build_options(), ['--cache-from', 'type=local,src=' + str(self.restored),
                                                             '--build-arg', 'BUILDKIT_CACHE_MOUNT_NS=' + mount])
                    progress.update(complete=True, local_cache_imports=1, cached_build_steps=cached_steps)
                if cached_steps:
                    self.assertEqual(prove(self.restored, stored, replay, env, builder=builder),
                                     hashlib.sha256(stored.read_bytes()).hexdigest())
                else:
                    with self.assertRaisesRegex(ValueError, 'did not reuse'):
                        prove(self.restored, stored, replay, env, builder=builder)
            replay.write_text(json.dumps({'reservation': reservation, 'mount_namespace': mount,
                                          'kind': 'am-dependencies'}))
            seal_replay(replay, stored)
            for dependency_steps in (0, 1):
                def dependency_builder(_root, _actual, _directory, *, cache, progress):
                    self.assertNotEqual(cache.mount, mount)
                    progress.update(complete=True, local_cache_imports=1, cached_build_steps=1,
                                    cached_dependency_steps=dependency_steps)
                if dependency_steps:
                    self.assertEqual(prove(self.restored, stored, replay, env, builder=dependency_builder),
                                     hashlib.sha256(stored.read_bytes()).hexdigest())
                else:
                    with self.assertRaisesRegex(ValueError, 'did not reuse AM dependencies'):
                        prove(self.restored, stored, replay, env, builder=dependency_builder)


if __name__ == "__main__":
    unittest.main()
