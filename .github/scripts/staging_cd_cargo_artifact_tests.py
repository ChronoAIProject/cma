import hashlib
import json
import io
import tarfile
import stat
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import ci_retry
import staging_cd_cargo_artifact as transport

REPO = "ChronoAIProject/cma"
SOURCE = "a" * 40
CURRENT = "b" * 40
FAMILY = "cma-cd-cargo-v4-am-" + "a" * 64 + "-"
FILES = {"manifest.json": json.dumps({"format": 1, "prefix": FAMILY + "b" * 64 + "-"}).encode(), "registry.tar.gz": b"registry", "target.tar.gz": b"target"}


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
                "workflow_run": {"id": run_id, "head_sha": SOURCE, "head_branch": "develop",
                                 "repository_id": 7, "head_repository_id": 7},
                **altered}

    def __call__(self, path, *, timeout, output=None):
        self.paths.append(path)
        assert 0 < timeout <= transport.DOWNLOAD_SECONDS
        prefix = f"repos/{REPO}/actions/"
        if path.startswith(prefix + "artifacts?name="):
            assert path == f"{prefix}artifacts?name=cma-cd-cargo-{self.component}&per_page=50"
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
                    "CD_COMPONENT": "am", "CD_CARGO_FAMILY": FAMILY, "RUNNER_TEMP": str(self.root / "runner")}
        self.accepted = self.root / "runner/cma-build-cache/accepted"

    def tearDown(self):
        self.temp.cleanup()

    def restore(self, remote, env=None, *, ancestry=lambda root, source, head, timeout: True, **options):
        return transport.restore(self.env if env is None else env, get=remote, root=self.root,
                                 ancestry=ancestry, **options)

    def spec(self, run_id, *, members=None, symlink=None, digest=None, **altered):
        path = self.root / f"bundle-{run_id}.zip"
        real = bundle(path, members, symlink=symlink)
        remote = Remote(self.root, [])
        artifact = remote.artifact(run_id, digest or real, **altered.pop("artifact", {}))
        return {"artifact": artifact, "zip": path, **altered}

    def test_restores_exact_sealed_files_from_a_verified_develop_publication(self):
        remote = Remote(self.root, [self.spec(41)])
        result = self.restore(remote)
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

    def test_newer_incompatible_profile_does_not_hide_older_compatible_bundle(self):
        from staging_cd_build_cache import BuildCache, bridge, prepare
        from staging_cd_build_profile import AM_PROFILE

        root = Path(__file__).resolve().parents[2]
        tool = bridge()
        env = {**self.env, "CD_TASKS": json.dumps([{"component": "am", "mode": "resume"}])}
        for index, (selected, incompatible) in enumerate((("1", "0"), ("0", ""), ("", "1"))):
            with self.subTest(selected=selected, incompatible=incompatible):
                reader_env = {**env, AM_PROFILE: selected}
                reader = BuildCache(root, reader_env, root, "am")
                requested = prepare(root, reader_env)
                self.assertEqual(requested["family"], reader.key.family)
                compatible = self.root / f"compatible-{index}"
                other = self.root / f"other-{index}"
                for directory, profile in ((compatible, selected), (other, incompatible)):
                    directory.mkdir()
                    writer = BuildCache(root, {**env, AM_PROFILE: profile}, root, "am")
                    for name in ("registry", "target"):
                        with tarfile.open(directory / f"{name}.tar.gz", "w:gz") as archive:
                            member = tarfile.TarInfo("release/incremental/fixture/data" if name == "target" else "fixture")
                            member.size = 4
                            archive.addfile(member, io.BytesIO(b"data"))
                    tool.seal(directory, writer.key.prefix, family=writer.key.family)
                def spec(run_id, directory):
                    return self.spec(run_id, members={file.name: file.read_bytes() for file in directory.iterdir()})
                remote = Remote(self.root, [spec(42, other), spec(41, compatible)])
                result = self.restore(remote, {**reader_env, "CD_CARGO_FAMILY": requested["family"]})
                self.assertEqual((result["state"], result["producer_run"]), ("RESTORED", 41))
                self.assertEqual(result["compiler_family"], requested["family"])
                with patch("staging_cd_build_cache.bridge", return_value=tool), patch.object(tool, "transfer") as transfer:
                    self.assertTrue(reader.restore())
                    transfer.assert_called_once()
                    self.assertEqual(transfer.call_args.args[0], "inject")
                transport.discard(self.accepted)

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
                        {"CD_CARGO_FAMILY": ""}, {"CD_CARGO_FAMILY": FAMILY.replace("-am-", "-eg-")}):
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
        ticks = iter([0, 1, 2, 3, transport.DEADLINE_SECONDS + 1, transport.DEADLINE_SECONDS + 2])
        remote = Remote(self.root, [self.spec(71), self.spec(72)])
        result = self.restore(remote, clock=lambda: next(ticks))
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
