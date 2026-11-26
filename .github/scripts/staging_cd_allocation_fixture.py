"""Synthetic remote with immutable refs and observable allocation boundaries."""
import collections
import fnmatch
import hashlib
import json
import subprocess
import threading
import time

from staging_cd_github import GitHub, GitHubNotFound
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_reservations import next_reservation, pending_name
from staging_cd_versions import DEPLOYABLES


class AllocationServer(GitHub):
    def __init__(self, count=3, *, reserved=True, latency=0, components=("am",)):
        super().__init__("fixture")
        self.components = components
        self.sources = [f"{index + 1:040x}" for index in range(count + 1)]
        self._history = DevelopHistory(self.sources[-1], list(reversed(self.sources)))
        self.refs, self.objects = {}, {}
        self.calls, self.commands = [], []
        self.latency, self.active, self.peak = latency, 0, 0
        self.lock = threading.Lock()
        self.collision = self.ambiguous = self.corrupt_readback = False
        # (returncode, stdout, stderr) of a push the server answers without writing.
        self.rejection = None
        pending = []
        for index, source in enumerate(self.sources[:-1] if reserved else []):
            for component in components:
                value = next_reservation(component, source, "e" * 40, index + 1, "patch", index + 10, 1, [], pending)
                pending.append(value)
                self.refs[pending_name(value)] = self.object(value)
        self.original = dict(self.refs)

    def delay(self):
        with self.lock:
            self.active += 1
            self.peak = max(self.peak, self.active)
        try:
            time.sleep(self.latency)
        finally:
            with self.lock:
                self.active -= 1

    def object(self, value):
        oid = hashlib.sha1(canonical(value).encode()).hexdigest()
        self.objects[oid] = {"sha": oid, "tag": pending_name(value), "message": canonical(value),
                             "object": {"type": "commit", "sha": value["source"]}}
        return oid

    def request(self, path, **_kwargs):
        self.calls.append(path)
        self.delay()
        if path.startswith("git/matching-refs/tags/"):
            prefix = path.removeprefix("git/matching-refs/tags/")
            return [{"ref": "refs/tags/" + name, "object": {"type": "tag", "sha": oid}}
                    for name, oid in self.refs.items() if name.startswith(prefix)]
        if path.startswith("git/tags/"):
            return self.objects[path.removeprefix("git/tags/")]
        if path.startswith("git/ref/tags/"):
            name = path.removeprefix("git/ref/tags/")
            if name not in self.refs:
                raise GitHubNotFound("GitHub resource was not found")
            return {"ref": "refs/tags/" + name, "object": {"type": "tag",
                    "sha": "0" * 40 if self.corrupt_readback else self.refs[name]}}
        if path.startswith("actions/runs/"):
            return {"id": int(path.split("/")[2]), "run_attempt": 1, "event": "push", "status": "completed",
                    "head_branch": "develop", "repository": {"full_name": "ChronoAIProject/cma"}}
        index = self.sources.index(path.split("/")[1]) if path.startswith("commits/") else int(path.split("/")[1]) - 1
        pr = {"number": index + 1, "merged": True, "merged_at": "fixture", "merge_commit_sha": self.sources[index],
              "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
              "labels": [{"name": "version-upgrade:patch"}]}
        return [pr] if path.startswith("commits/") else pr

    def command(self, args, **kwargs):
        self.commands.append(args)
        if args[1] == "ls-remote":
            self.delay()
            refs = {"refs/heads/develop": self.history().head,
                    **{"refs/tags/" + name: oid for name, oid in self.refs.items()}}
            output = "".join(oid + "\t" + ref + "\n" for ref, oid in refs.items()
                             if any(fnmatch.fnmatchcase(ref, pattern) for pattern in args[4:]))
        elif args[1] == "mktag":
            output = self.object(json.loads(kwargs["input"].split("\n\n", 1)[1])) + "\n"
        else:
            assert args[1] == "push" and "--force" not in args
            self.delay()
            updates = [arg.split(":refs/tags/", 1) for arg in args if ":refs/tags/" in arg]
            if self.rejection is not None:
                return subprocess.CompletedProcess(args, *self.rejection)
            if self.collision or any(name in self.refs for _oid, name in updates):
                return subprocess.CompletedProcess(args, 1, "", "")
            self.refs.update({name: oid for oid, name in updates})
            output = "*\tnew:ref\t[new tag]\n" * len(updates)
            if self.ambiguous:
                return subprocess.CompletedProcess(args, 1, "", "")
        return subprocess.CompletedProcess(args, 0, output, "")

    def queue(self):
        return {"head": self.sources[-1], "event_source": self.sources[-1], "changes": [
            {"source": source, "tree": "e" * 40, "deployables": sorted(DEPLOYABLES[c] for c in self.components)}
            for source in self.sources]}

    def counts(self):
        return dict(collections.Counter(path.split("/", 2)[0] for path in self.calls))
