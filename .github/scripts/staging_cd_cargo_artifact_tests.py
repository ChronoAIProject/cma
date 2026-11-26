import hashlib
import json
import stat
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import ci_retry
import staging_cd_cargo_artifact as transport
from staging_cd_build_cargo_keys import CargoKey

REPO = "ChronoAIProject/cma"
SOURCE = "a" * 40
CURRENT = "b" * 40
FAMILY = "cma-cd-cargo-v4-am-" + "c" * 64 + "-"
OTHER_FAMILY = "cma-cd-cargo-v4-am-" + "d" * 64 + "-"
GRAPH = "e" * 64
INDEX = f"graph-{FAMILY}{GRAPH}"


def manifest(family, graph=GRAPH, *, source=None):
    metadata = {"format": 1, "prefix": CargoKey(family, graph, source or "").prefix}
    if source is not None:
        metadata.update(format=2, graph=graph, source=source)
    return json.dumps(metadata).encode()


FILES = {"manifest.json": manifest(FAMILY), "registry.tar.gz": b"registry", "target.tar.gz": b"target"}


def bundle(path, members=None, *, symlink=None):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as archive:
        for name, data in (members or FILES).items():
            if name != symlink:
                archive.writestr(name, data)
        if symlink:
            info = zipfile.ZipInfo(symlink)
            info.external_attr = (stat.S_IFLNK | 0o777) << 16
            archive.writestr(info, "/etc/passwd")
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


class Remote:
    """Answers the four GitHub reads; each producer run can be altered independently."""

    def __init__(self, root, runs, component="am"):
        self.root, self.runs, self.component, self.paths = root, runs, component, []

    def artifact(self, run_id, digest, **altered):
        return {"id": run_id * 10, "name": "cma-cd-cargo-" + self.component, "expired": False,
                "size_in_bytes": 1024, "digest": digest, "created_at": "2026-11-20T00:10:00Z",
                "workflow_run": {"id": run_id, "head_sha": SOURCE, "head_branch": "develop",
                                 "repository_id": 7, "head_repository_id": 7},
                **altered}

    def __call__(self, path, *, timeout, output=None):
        self.paths.append(path)
        assert 0 < timeout <= transport.DOWNLOAD_SECONDS
        prefix = f"repos/{REPO}/actions/"
        if path.startswith(prefix + "artifacts?name="):
            name, _, rest = path.removeprefix(prefix + "artifacts?name=").partition("&")
            assert rest == "per_page=50"
            return {"artifacts": [spec["artifact"] for spec in self.runs if spec["artifact"]["name"] == name]}
        if path.startswith(prefix + "runs/") and "/artifacts?name=" in path:
            run_id, _, query = path.removeprefix(prefix + "runs/").partition("/artifacts?name=")
            name, _, rest = query.partition("&")
            assert rest == "per_page=2"
            rows = [spec["artifact"] for spec in self.runs if spec["artifact"]["name"] == name
                    and str(spec["artifact"]["workflow_run"]["id"]) == run_id]
            return {"total_count": len(rows), "artifacts": rows[:2]}
        for spec in self.runs:
            run_id = spec["artifact"]["workflow_run"]["id"]
            if path == f"{prefix}runs/{run_id}":
                return {"id": run_id, "run_attempt": 1, "head_sha": SOURCE, "path": ".github/workflows/staging-cd.yml",
                        "event": "push", "head_branch": "develop", "status": "completed",
                        "repository": {"full_name": REPO, "id": 7}, "head_repository": {"full_name": REPO, "id": 7},
                        **spec.get("run", {})}
            if path == f"{prefix}runs/{run_id}/attempts/1/jobs?per_page=100":
                job = {"name": f"Release {self.component} / Publish", "status": "completed", "conclusion": "success",
                       "head_sha": SOURCE, "run_id": run_id, "run_attempt": 1,
                       "started_at": "2026-11-20T00:00:00Z", "completed_at": "2026-11-20T00:20:00Z",
                       **spec.get("job", {})}
                return {"total_count": 1, "jobs": [job]}
            if path == f"{prefix}artifacts/{spec['artifact']['id']}/zip":
                with output.open("xb") as stream:
                    stream.write(spec["zip"].read_bytes())
                return output
        raise AssertionError("unexpected read " + path)


class CargoArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "runner").mkdir()
        self.env = {"GITHUB_REPOSITORY": REPO, "GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/develop",
                    "GITHUB_SHA": CURRENT, "GITHUB_RUN_ID": "900", "GITHUB_RUN_ATTEMPT": "1",
                    "CD_COMPONENT": "am", "CD_CARGO_CACHE_FAMILY": FAMILY,
                    "RUNNER_TEMP": str(self.root / "runner")}
        self.accepted = self.root / "runner/cma-build-cache/accepted"

    def tearDown(self):
        self.temp.cleanup()

    def restore(self, remote, env=None, *, ancestry=lambda root, source, head, timeout: True, **options):
        return transport.restore(self.env if env is None else env, get=remote, root=self.root,
                                 ancestry=ancestry, **options)

    def spec(self, run_id, *, members=None, symlink=None, digest=None, **altered):
        path = self.root / f"bundle-{run_id}-{altered.get('artifact', {}).get('id', run_id)}.zip"
        real = bundle(path, members, symlink=symlink)
        remote = Remote(self.root, [])
        artifact = remote.artifact(run_id, digest or real, **altered.pop("artifact", {}))
        return {"artifact": artifact, "zip": path, **altered}

    def test_restores_exact_sealed_files_from_a_verified_develop_publication(self):
        remote = Remote(self.root, [self.spec(41, artifact={"name": FAMILY.removesuffix("-")}),
                                    self.spec(42, artifact={"name": OTHER_FAMILY.removesuffix("-")}),
                                    self.spec(43)])
        result = self.restore(remote)
        self.assertEqual(result["state"], "RESTORED")
        self.assertEqual((result["producer_run"], result["producer_source"]), (41, SOURCE))
        self.assertEqual({p.name: p.read_bytes() for p in self.accepted.iterdir()}, FILES)
        self.assertFalse((self.accepted.parent / "artifact.zip").exists())
        self.assertEqual([path for path in remote.paths if "artifacts?" in path],
                         [f"repos/{REPO}/actions/artifacts?name={FAMILY.removesuffix('-')}&per_page=50"])
        self.assertEqual([path for path in remote.paths if path.endswith("/zip")],
                         [f"repos/{REPO}/actions/artifacts/410/zip"])

    def test_legacy_fallback_checks_manifest_and_skips_an_incompatible_newer_bundle(self):
        incompatible = {**FILES, "manifest.json": manifest(OTHER_FAMILY)}
        remote = Remote(self.root, [self.spec(42, artifact={"name": FAMILY.removesuffix("-")}, members=incompatible),
                                    self.spec(43, members=incompatible), self.spec(41)])
        result = self.restore(remote)
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
        self.assertEqual({p.name: p.read_bytes() for p in self.accepted.iterdir()}, FILES)
        self.assertEqual([path.split("/")[-2] for path in remote.paths if path.endswith("/zip")],
                         ["420", "430", "410"])

    def test_exact_graph_discovers_the_older_matching_bundle_without_downloading_the_newest(self):
        family_name = FAMILY.removesuffix("-")
        partial = {**FILES, "manifest.json": manifest(FAMILY, "f" * 64)}
        remote = Remote(self.root, [self.spec(42, artifact={"name": family_name}, members=partial),
                                    self.spec(41, artifact={"name": family_name}),
                                    self.spec(41, artifact={"name": INDEX, "id": 411})])
        result = self.restore(remote, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["producer_run"], result["selection"]), ("RESTORED", 41, "exact-graph"))
        self.assertEqual((self.accepted / "manifest.json").read_bytes(), FILES["manifest.json"])
        self.assertEqual([path.split("/")[-2] for path in remote.paths if path.endswith("/zip")], ["410"])

    def test_wrong_graph_behind_a_valid_marker_cannot_be_restored_as_an_exact_match(self):
        family_name = FAMILY.removesuffix("-")
        partial = {**FILES, "manifest.json": manifest(FAMILY, "f" * 64)}
        remote = Remote(self.root, [self.spec(42, artifact={"name": family_name}, members=partial),
                                    self.spec(42, artifact={"name": INDEX, "id": 421}),
                                    self.spec(41)])
        result = self.restore(remote, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
        self.assertNotIn("selection", result)
        self.assertEqual((self.accepted / "manifest.json").read_bytes(), FILES["manifest.json"])
        self.assertEqual([path.split("/")[-2] for path in remote.paths if path.endswith("/zip")], ["420", "410"])

    def test_new_source_restores_the_indexed_workspace_bundle_from_the_same_graph(self):
        producer = {**FILES, "manifest.json": manifest(FAMILY, source=SOURCE)}
        remote = Remote(self.root, [self.spec(41, artifact={"name": FAMILY.removesuffix("-")}, members=producer),
                                    self.spec(41, artifact={"name": INDEX, "id": 411})])
        result = self.restore(remote, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["selection"]), ("RESTORED", "exact-graph"))
        stored = json.loads((self.accepted / "manifest.json").read_bytes())
        self.assertEqual(stored["source"], SOURCE)
        self.assertNotEqual(stored["prefix"], CargoKey(FAMILY, GRAPH, CURRENT).prefix)

    def test_graph_marker_cannot_bypass_workspace_generation_binding(self):
        metadata = json.loads(manifest(FAMILY, "f" * 64, source=SOURCE))
        metadata["graph"] = GRAPH
        corrupt = {**FILES, "manifest.json": json.dumps(metadata).encode()}
        remote = Remote(self.root, [self.spec(42, artifact={"name": FAMILY.removesuffix("-")}, members=corrupt),
                                    self.spec(42, artifact={"name": INDEX, "id": 421}),
                                    self.spec(41)])
        result = self.restore(remote, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
        self.assertNotIn("selection", result)

    def test_marker_cannot_authorize_a_bulk_archive_outside_its_successful_publish_attempt(self):
        family_name = FAMILY.removesuffix("-")
        remote = Remote(self.root, [self.spec(42, artifact={"name": INDEX, "id": 421}),
                                    self.spec(42, artifact={"name": family_name, "created_at": "2026-11-20T00:30:00Z"}),
                                    self.spec(41)])
        result = self.restore(remote, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
        self.assertEqual([path.split("/")[-2] for path in remote.paths if path.endswith("/zip")], ["410"])

    def test_unavailable_graph_index_preserves_family_fallback(self):
        remote = Remote(self.root, [self.spec(41, artifact={"name": FAMILY.removesuffix("-")})])

        def unavailable(path, **kwargs):
            if f"artifacts?name={INDEX}&" in path:
                raise transport.Refused("read")
            return remote(path, **kwargs)

        result = self.restore(unavailable, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH})
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
        self.assertNotIn("selection", result)

    def test_index_lookup_deadline_leaves_the_regular_bundle_download_budget(self):
        remote = Remote(self.root, [self.spec(41, artifact={"name": FAMILY.removesuffix("-")})])
        now = 0

        def slow_index(path, *, timeout, output=None):
            nonlocal now
            if f"artifacts?name={INDEX}&" in path:
                now = transport.EXACT_LOOKUP_SECONDS
                raise subprocess.TimeoutExpired("gh", timeout)
            if path.endswith("/zip"):
                self.assertEqual(timeout, transport.DOWNLOAD_SECONDS)
            return remote(path, timeout=timeout, output=output)

        result = self.restore(slow_index, self.env | {"CD_CARGO_CACHE_GRAPH": GRAPH},
                              clock=lambda: now, sleep=self.fail)
        self.assertEqual(result["state"], "RESTORED")

    def test_legacy_fallback_shares_the_candidate_budget(self):
        bad = [self.spec(run, members={**FILES, "manifest.json": b"{}"},
                         artifact={"name": FAMILY.removesuffix("-")} if run % 2 else {})
               for run in range(50, 50 + transport.CANDIDATES)]
        remote = Remote(self.root, [*bad, self.spec(41)])
        self.assertEqual(self.restore(remote), {"state": "MISS"})
        self.assertEqual(sum(path.endswith("/zip") for path in remote.paths), transport.CANDIDATES)
        self.assertFalse(self.accepted.exists())

    def test_refuses_every_unverified_producer_and_falls_back_to_the_next(self):
        for altered in ({"run": {"path": ".github/workflows/ci.yml"}}, {"run": {"event": "pull_request"}},
                        {"run": {"head_branch": "feature"}}, {"run": {"repository": {"full_name": "fork/cma", "id": 7}}},
                        {"run": {"head_repository": {"full_name": REPO, "id": 8}}},
                        {"run": {"head_sha": "c" * 40}}, {"job": {"conclusion": "failure"}},
                        {"job": {"status": "in_progress"}}, {"job": {"name": "Release eg / Publish"}},
                        {"job": {"started_at": "2026-11-20T01:00:00Z", "completed_at": "2026-11-20T01:20:00Z"}},
                        {"artifact": {"created_at": "2026-11-20T00:30:00Z"}},
                        {"artifact": {"expired": True}}, {"artifact": {"name": "cma-cd-cargo-eg"}},
                        {"artifact": {"size_in_bytes": transport.MAX_ARTIFACT_BYTES + 1}},
                        {"digest": "sha256:" + "0" * 64}, {"members": {**FILES, "extra": b"x"}},
                        {"members": {"manifest.json": b"{}", "registry.tar.gz": b"r"}},
                        {"members": {**FILES, "manifest.json": b" " * 4096}},
                        {"members": {**FILES, "manifest.json": b"{}"}},
                        {"symlink": "target.tar.gz"},
                        {"artifact": {"workflow_run": {"id": 900, "head_sha": SOURCE, "head_branch": "develop",
                                                       "repository_id": 7, "head_repository_id": 7}}},
                        {"artifact": {"workflow_run": {"id": 51, "head_sha": SOURCE, "head_branch": "develop",
                                                       "repository_id": 7, "head_repository_id": 8}}}):
            with self.subTest(altered=altered):
                bad = self.spec(51, **altered)
                remote = Remote(self.root, [bad])
                self.assertEqual(self.restore(remote), {"state": "MISS"})
                self.assertFalse(self.accepted.exists())
                good = self.spec(52)
                result = self.restore(Remote(self.root, [bad, good]))
                self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 52))
                for path in self.accepted.iterdir():
                    path.unlink()
                self.accepted.rmdir()

    def test_a_producer_off_develop_history_is_never_trusted(self):
        # A tag named develop reports head_branch "develop" for an arbitrary commit.
        calls = []
        def ancestry(root, source, head, timeout):
            calls.append((root, source, head))
            return False
        remote = Remote(self.root, [self.spec(81)])
        self.assertEqual(self.restore(remote, ancestry=ancestry), {"state": "MISS"})
        self.assertEqual(calls, [(self.root, SOURCE, CURRENT)])
        self.assertFalse(any("/zip" in path for path in remote.paths))
        self.assertFalse(self.accepted.exists())

    def test_rows_from_other_branches_cannot_crowd_out_develop_producers(self):
        foreign = [self.spec(90 + n, artifact={"workflow_run": {"id": 90 + n, "head_sha": SOURCE,
                   "head_branch": "feature", "repository_id": 7, "head_repository_id": 7}}) for n in range(3)]
        # A fork's own develop branch has the right name but another repository.
        foreign += [self.spec(95 + n, artifact={"workflow_run": {"id": 95 + n, "head_sha": SOURCE,
                    "head_branch": "develop", "repository_id": 7, "head_repository_id": 9}}) for n in range(3)]
        good = self.spec(20)
        result = self.restore(Remote(self.root, [*foreign, good]))
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 20))

    def test_real_ancestry_check_uses_git_history(self):
        repo = self.root / "repo"
        repo.mkdir()
        git = lambda *args: subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True)
        git("init", "-q")
        for name in ("one", "two"):
            (repo / name).write_text(name)
            git("add", name)
            git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", name)
        first = git("rev-parse", "HEAD").stdout.strip()
        # A merged side-branch commit is a develop ancestor but never a develop push head.
        git("checkout", "-q", "-b", "side")
        (repo / "side").write_text("side")
        git("add", "side")
        git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "side")
        side = git("rev-parse", "HEAD").stdout.strip()
        git("checkout", "-q", "-")
        git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "merge", "-q", "--no-ff", "-m", "merge", "side")
        head = git("rev-parse", "HEAD").stdout.strip()
        self.assertTrue(transport.on_develop(repo, first, head, 5))
        self.assertTrue(transport.on_develop(repo, head, head, 5))
        self.assertEqual(git("merge-base", "--is-ancestor", side, head).returncode, 0)
        self.assertFalse(transport.on_develop(repo, side, head, 5))
        self.assertFalse(transport.on_develop(repo, head, first, 5))
        self.assertFalse(transport.on_develop(repo, "c" * 40, head, 5))

    def test_context_and_existing_destination_refuse_before_any_read(self):
        for altered in ({"GITHUB_REPOSITORY": "fork/cma"}, {"GITHUB_EVENT_NAME": "pull_request"},
                        {"GITHUB_REF": "refs/heads/feature"}, {"CD_COMPONENT": "web"}, {"GITHUB_RUN_ID": "0"},
                        {"CD_CARGO_CACHE_FAMILY": ""}, {"CD_CARGO_CACHE_FAMILY": FAMILY.replace("-am-", "-eg-")},
                        {"CD_CARGO_CACHE_GRAPH": "malformed"}):
            with self.subTest(altered=altered):
                remote = Remote(self.root, [self.spec(61)])
                with self.assertRaises(transport.Refused):
                    self.restore(remote, self.env | altered)
                self.assertEqual(remote.paths, [])
        self.accepted.mkdir(parents=True)
        remote = Remote(self.root, [self.spec(62)])
        with self.assertRaises(transport.Refused):
            self.restore(remote)
        self.assertEqual(remote.paths, [])

    def test_deadline_stops_further_candidates_without_partial_output(self):
        remote = Remote(self.root, [self.spec(71), self.spec(72)])
        def clock():
            return transport.DEADLINE_SECONDS + 1 if any("/runs/" in path for path in remote.paths) else 0
        result = self.restore(remote, clock=clock)
        self.assertEqual(result, {"state": "MISS"})
        self.assertFalse(self.accepted.exists())
        self.assertFalse(any("/zip" in path for path in remote.paths))

    def test_transport_failures_repeat_inside_the_deadline_without_partial_bytes(self):
        remote = Remote(self.root, [self.spec(31)])
        dropped = transport.Refused("download")
        dropped.__cause__ = ci_retry.TransientError()
        failures = {"artifacts?name=": [subprocess.TimeoutExpired("gh", 10)], "/zip": [dropped]}
        def flaky(path, *, timeout, output=None):
            for key, pending in failures.items():
                if key in path and pending:
                    if output is not None:
                        output.write_bytes(b"partial")
                    raise pending.pop(0)
            return remote(path, timeout=timeout, output=output)
        sleeps = []
        result = self.restore(flaky, sleep=sleeps.append)
        self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 31))
        self.assertEqual(len(sleeps), 2)
        self.assertEqual({p.name: p.read_bytes() for p in self.accepted.iterdir()}, FILES)
        # A refusal is not transport: the candidate is skipped after one read.
        remote = Remote(self.root, [self.spec(32)])
        def refused(path, *, timeout, output=None):
            if "/runs/" in path:
                remote.paths.append(path)
                raise transport.Refused("read")
            return remote(path, timeout=timeout, output=output)
        for path in self.accepted.iterdir():
            path.unlink()
        self.accepted.rmdir()
        self.assertEqual(self.restore(refused, sleep=sleeps.append), {"state": "MISS"})
        self.assertEqual(sum("/runs/" in path for path in remote.paths), 1)
        self.assertEqual(len(sleeps), 2)

    def test_github_reader_marks_only_transport_failures_as_repeatable(self):
        for stderr, expected in ((b"read: connection reset by peer", ci_retry.TRANSIENT),
                                 (b"gh: Not Found (HTTP 404)", ci_retry.PERMANENT)):
            with self.subTest(stderr=stderr), self.assertRaises(transport.Refused) as raised, \
                    patch.object(transport.subprocess, "run",
                                 return_value=subprocess.CompletedProcess([], 1, b"", stderr)):
                transport.github(f"repos/{REPO}/actions/runs/1", timeout=1)
            self.assertEqual(ci_retry.classify(raised.exception), expected)

    def test_github_reader_only_reads_this_repository(self):
        for path in ("repos/other/cma/actions/artifacts", f"repos/{REPO}/../other"):
            with self.assertRaises(transport.Refused):
                transport.github(path, timeout=1)


if __name__ == "__main__":
    unittest.main()
