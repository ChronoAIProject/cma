"""One complete ref snapshot and bounded settlement for insert-only tags."""
import json

from staging_cd_allocation_io import TransientRead, parallel_read
from staging_cd_common import matches, require
from staging_cd_github import GitHubNotFound
from staging_cd_receipts import canonical, parse_annotation
from staging_cd_reservations import Reservations, pending_name, validate_reservation
from staging_cd_versions import DEPLOYABLES, version_tuple

REMOTE = "https://github.com/ChronoAIProject/cma.git"


def verify_final_owner(item, tag):
    identity = tag["receipt"]["identity"]
    require(all(identity[key] == value for key, value in item.items() if key != "schema"),
            "Terminal reservation differs from final receipt")


class AllocationSnapshot(Reservations):
    def __init__(self, api, token, root, io, *, mark=lambda _component, _phase: None):
        super().__init__(api, token, root)
        self.io, self.progress = io, mark
        self.pending, self.finals, self.planned = [], [], []
        self.pending_oids = {}
        self.loaded = False
        self.commit_started = False

    def command(self, *args, data=None, read=False):
        operation = self.io.read if read else self.io.write
        return operation(["git", *args], cwd=self.root, env=self.env, input=data,
                         text=True, capture_output=True, check=False)

    def load(self):
        require(not self.loaded, "Allocation inventory may only be loaded once")
        self.progress("all", "history")
        self.api.history()  # Initialize before concurrent annotation reads.
        self.progress("all", "tag-inventory")
        selected = []
        for reference, oid in self.ref_oids().items():
            name = reference.removeprefix("refs/tags/")
            for component, deployable in DEPLOYABLES.items():
                for kind, prefix in (("pending", deployable + "/pending/v"), ("final", deployable + "/v")):
                    if name.startswith(prefix):
                        version_tuple(name[len(prefix):])
                        selected.append((component, kind, name, oid))
                        if kind == "pending":
                            self.pending_oids[name] = oid

        for kind, value in parallel_read(self.annotation, selected):
            (self.pending if kind == "pending" else self.finals).append(value)
        pending_by_version = {(item["component"], item["version"]): item for item in self.pending}
        for tag in self.finals:
            identity = tag["receipt"]["identity"]
            item = pending_by_version.get((identity["component"], identity["version"]))
            if item is not None:
                verify_final_owner(item, tag)
        self.loaded = True
        return self

    def ref_oids(self, *patterns):
        result = self.command("ls-remote", "--tags", "--refs", REMOTE, *patterns, read=True)
        require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
                "Allocation tag inventory is incomplete")
        refs = {}
        for line in result.stdout.splitlines():
            parts = line.split("\t")
            require(len(parts) == 2 and matches(r"[0-9a-f]{40}", parts[0])
                    and parts[1].startswith("refs/tags/") and parts[1] not in refs,
                    "Allocation tag inventory is malformed")
            oid, reference = parts
            require(not patterns or reference in patterns, "Unexpected allocation tag ref")
            refs[reference] = oid
        return refs

    def annotation(self, row):
        component, kind, name, oid = row
        tag = self.api.request("git/tags/" + oid)
        require(isinstance(tag, dict) and tag.get("sha") == oid and tag.get("tag") == name
                and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit"
                and matches(r"[0-9a-f]{40}", tag["object"].get("sha")),
                "Allocation annotation identity differs")
        if kind == "pending":
            value = validate_reservation(json.loads(tag.get("message", "")))
            require(value["component"] == component and pending_name(value) == name
                    and value["source"] == tag["object"]["sha"], "Pending version ref differs from its reservation")
        else:
            receipt = parse_annotation(tag.get("message"))
            require(receipt["tag"] == name and receipt["source"] == tag["object"]["sha"]
                    and receipt["identity"]["component"] == component,
                    "Tag receipt differs from the annotated commit")
            self.api.history().position(receipt["source"])
            value = {"name": name, "annotation_name": name, "type": "tag", "target_type": "commit",
                     "target": tag["object"]["sha"], "receipt": receipt}
        self.progress(component, "annotation-read")
        return kind, value

    def final_after_owner(self, item):
        # One advertisement observes both sides of the pending-to-final transition
        # after the writer is terminal. Neither ref alone proves a failed release.
        component = item["component"]
        pending = pending_name(item)
        final = DEPLOYABLES[component] + "/v" + item["version"]
        pending_ref, final_ref = ("refs/tags/" + name for name in (pending, final))
        refs = self.ref_oids(pending_ref, final_ref)
        if pending_ref in refs:
            require(refs[pending_ref] == self.pending_oids[pending], "Pending version ref changed")
        if final_ref not in refs:
            require(pending_ref in refs, "Terminal reservation settlement is uncertain")
            return None
        _, tag = self.annotation((component, "final", final, refs[final_ref]))
        verify_final_owner(item, tag)
        return tag

    def list(self, component=None):
        require(self.loaded, "Allocation inventory has not been verified")
        require(component is None or component in DEPLOYABLES, "Unknown reservation component")
        return [value for value in self.pending if component is None or value["component"] == component]

    def insert(self, value):
        require(self.loaded and not self.commit_started, "Allocation planning is closed")
        validate_reservation(value)
        require(value["component"] in DEPLOYABLES
                and pending_name(value) not in {pending_name(item) for item in self.pending + self.planned},
                "Allocation reservation collision")
        self.planned.append(value)
        return True

    def commit(self):
        require(self.loaded, "Allocation inventory has not been verified")
        require(not self.commit_started, "Allocation commit already attempted")
        self.commit_started = True
        if not self.planned:
            return
        for value in self.planned:
            text = (f"object {value['source']}\ntype commit\ntag {pending_name(value)}\n"
                    "tagger github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com> 0 +0000\n\n"
                    + canonical(value) + "\n")
            result = self.command("mktag", data=text)
            require(result.returncode == 0 and matches(r"[0-9a-f]{40}", result.stdout.strip()),
                    "Reservation object could not be created")
            name, oid = pending_name(value), result.stdout.strip()
            self.progress(value["component"], "reservation-push")
            pushed = self.command("push", "--porcelain", REMOTE, oid + ":refs/tags/" + name)
            self.verify(name, oid, value["component"])
            require(pushed.returncode != 0 or sum(line.startswith("*\t") and "[new tag]" in line
                                                 for line in pushed.stdout.splitlines()) == 1,
                    "An up-to-date push is not a new version reservation")
            self.pending.append(value)

    def verify(self, name, oid, component):
        # Each iteration consumes one physical GET, including transient failures.
        for attempt in range(3):
            try:
                reference = self.api.request("git/ref/tags/" + name, allow_not_found=True, single_attempt=True)
            except (GitHubNotFound, TransientRead) as error:
                phase = "readback-missing" if isinstance(error, GitHubNotFound) else "readback-transient"
                self.progress(component, phase)
                require(attempt < 2, "Allocation reservation readback is unavailable")
                self.io.sleep(min(0.2, self.io.remaining()))
                continue
            valid = (isinstance(reference, dict) and reference.get("ref") == "refs/tags/" + name
                     and isinstance(reference.get("object"), dict)
                     and reference["object"].get("type") == "tag" and reference["object"].get("sha") == oid)
            if not valid:
                self.progress(component, "readback-conflict")
            require(valid, "Allocation reservation readback differs")
            self.progress(component, "reservation-readback")
            return


class SnapshotAPI:
    def __init__(self, api, snapshot):
        self.request, self.history = api.request, api.history
        self.snapshot = snapshot

    def tags(self):
        require(self.snapshot.loaded, "Allocation inventory has not been verified")
        return list(self.snapshot.finals)
