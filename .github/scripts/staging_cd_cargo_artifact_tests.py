import hashlib
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path

import staging_cd_cargo_artifact as transport

REPO = "ChronoAIProject/cma"
SOURCE = "a" * 40
CURRENT = "b" * 40
FILES = {"manifest.json": b'{"format": 1}', "registry.tar.gz": b"registry", "target.tar.gz": b"target"}


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
                "size_in_bytes": 1024, "digest": digest,
                "workflow_run": {"id": run_id, "head_sha": SOURCE, "repository_id": 7, "head_repository_id": 7},
                **altered}

    def __call__(self, path, *, timeout, output=None):
        self.paths.append(path)
        assert 0 < timeout <= 180
        prefix = f"repos/{REPO}/actions/"
        if path.startswith(prefix + "artifacts?name="):
            assert path == f"{prefix}artifacts?name=cma-cd-cargo-{self.component}&per_page=5"
            return {"artifacts": [spec["artifact"] for spec in self.runs]}
        for spec in self.runs:
            run_id = spec["artifact"]["workflow_run"]["id"]
            if path == f"{prefix}runs/{run_id}":
                return {"id": run_id, "run_attempt": 1, "head_sha": SOURCE, "path": ".github/workflows/staging-cd.yml",
                        "event": "push", "head_branch": "develop", "status": "completed",
                        "repository": {"full_name": REPO, "id": 7}, "head_repository": {"full_name": REPO, "id": 7},
                        **spec.get("run", {})}
            if path == f"{prefix}runs/{run_id}/attempts/1/jobs?per_page=100":
                job = {"name": f"Release {self.component} / Publish", "status": "completed", "conclusion": "success",
                       "head_sha": SOURCE, "run_id": run_id, "run_attempt": 1, **spec.get("job", {})}
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
                    "CD_COMPONENT": "am", "RUNNER_TEMP": str(self.root / "runner")}
        self.accepted = self.root / "runner/cma-build-cache/accepted"

    def tearDown(self):
        self.temp.cleanup()

    def spec(self, run_id, *, members=None, symlink=None, digest=None, **altered):
        path = self.root / f"bundle-{run_id}.zip"
        real = bundle(path, members, symlink=symlink)
        remote = Remote(self.root, [])
        artifact = remote.artifact(run_id, digest or real, **altered.pop("artifact", {}))
        return {"artifact": artifact, "zip": path, **altered}

    def test_restores_exact_sealed_files_from_a_verified_develop_publication(self):
        remote = Remote(self.root, [self.spec(41)])
        result = transport.restore(self.env, get=remote)
        self.assertEqual(result["state"], "RESTORED")
        self.assertEqual((result["producer_run"], result["producer_source"]), (41, SOURCE))
        self.assertEqual({p.name: p.read_bytes() for p in self.accepted.iterdir()}, FILES)
        self.assertFalse((self.accepted.parent / "artifact.zip").exists())

    def test_refuses_every_unverified_producer_and_falls_back_to_the_next(self):
        for altered in ({"run": {"path": ".github/workflows/ci.yml"}}, {"run": {"event": "pull_request"}},
                        {"run": {"head_branch": "feature"}}, {"run": {"repository": {"full_name": "fork/cma", "id": 7}}},
                        {"run": {"head_repository": {"full_name": REPO, "id": 8}}},
                        {"run": {"head_sha": "c" * 40}}, {"job": {"conclusion": "failure"}},
                        {"job": {"status": "in_progress"}}, {"job": {"name": "Release eg / Publish"}},
                        {"artifact": {"expired": True}}, {"artifact": {"name": "cma-cd-cargo-eg"}},
                        {"artifact": {"size_in_bytes": transport.MAX_ARTIFACT_BYTES + 1}},
                        {"digest": "sha256:" + "0" * 64}, {"members": {**FILES, "extra": b"x"}},
                        {"members": {"manifest.json": b"{}", "registry.tar.gz": b"r"}},
                        {"symlink": "target.tar.gz"}, {"artifact": {"workflow_run": {"id": 900, "head_sha": SOURCE,
                                                                    "repository_id": 7, "head_repository_id": 7}}}):
            with self.subTest(altered=altered):
                bad = self.spec(51, **altered)
                remote = Remote(self.root, [bad])
                self.assertEqual(transport.restore(self.env, get=remote), {"state": "MISS"})
                self.assertFalse(self.accepted.exists())
                good = self.spec(52)
                result = transport.restore(self.env, get=Remote(self.root, [bad, good]))
                self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 52))
                for path in self.accepted.iterdir():
                    path.unlink()
                self.accepted.rmdir()

    def test_context_and_existing_destination_refuse_before_any_read(self):
        for altered in ({"GITHUB_REPOSITORY": "fork/cma"}, {"GITHUB_EVENT_NAME": "pull_request"},
                        {"GITHUB_REF": "refs/heads/feature"}, {"CD_COMPONENT": "web"}, {"GITHUB_RUN_ID": "0"}):
            with self.subTest(altered=altered):
                remote = Remote(self.root, [self.spec(61)])
                with self.assertRaises(transport.Refused):
                    transport.restore(self.env | altered, get=remote)
                self.assertEqual(remote.paths, [])
        self.accepted.mkdir(parents=True)
        remote = Remote(self.root, [self.spec(62)])
        with self.assertRaises(transport.Refused):
            transport.restore(self.env, get=remote)
        self.assertEqual(remote.paths, [])

    def test_deadline_stops_further_candidates_without_partial_output(self):
        ticks = iter([0, 1, 2, 3, transport.DEADLINE_SECONDS + 1, transport.DEADLINE_SECONDS + 2])
        remote = Remote(self.root, [self.spec(71), self.spec(72)])
        result = transport.restore(self.env, get=remote, clock=lambda: next(ticks))
        self.assertEqual(result, {"state": "MISS"})
        self.assertFalse(self.accepted.exists())
        self.assertFalse(any("/zip" in path for path in remote.paths))

    def test_github_reader_only_reads_this_repository(self):
        for path in ("repos/other/cma/actions/artifacts", f"repos/{REPO}/../other"):
            with self.assertRaises(transport.Refused):
                transport.github(path, timeout=1)


if __name__ == "__main__":
    unittest.main()
