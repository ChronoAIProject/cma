"""One complete ref snapshot and bounded settlement for insert-only tags."""
import itertools
import json
import re

import ci_retry
from staging_cd_allocation_io import SETTLE_WAITS, VERIFY_RESERVE_SECONDS, TransientRead, UncertainPush, parallel_read
from staging_cd_common import Refusal, matches, require
from staging_cd_github import GitHubNotFound
from staging_cd_receipts import canonical, parse_annotation
from staging_cd_release_tags import LocalTags
from staging_cd_reservations import Reservations, claim_ref, pending_name, validate_reservation
from staging_cd_versions import DEPLOYABLES, require_publishable_release, version_tuple

REMOTE = "https://github.com/ChronoAIProject/cma.git"
# Bound captured Git output even when the release inventory contains many tags.
ANNOTATION_BATCH_SIZE = 256
LOCAL_READ_SECONDS = 5
# GitHub's own answer to a push, which a repeat meets again: a tag ruleset or
# hook declined the refs (a porcelain rejection line), or a 403 alone whose
# message names the missing write permission. A 403 also answers a secondary
# rate limit, and git prints no status for a bad credential ("Authentication
# failed"), so neither is durable. Any other failed push proves nothing.
DECLINED = re.compile(r"^!\t[^\t\n]*\t\[remote rejected\] \([^)\n]*\bdeclined\b", re.MULTILINE)
DENIED = re.compile(r"\bPermission to \S+ denied to\b|\bWrite access to repository not granted\b", re.IGNORECASE)
THROTTLED = re.compile(r"\brate[ -]?limit", re.IGNORECASE)


def declined(pushed):
    stdout, stderr = pushed.stdout or "", pushed.stderr or ""
    if THROTTLED.search(stdout + stderr):
        return False
    statuses = {int(code) for code in ci_retry.STATUS.findall(stderr)}
    return bool(DECLINED.search(stdout)) or (statuses == {403} and bool(DENIED.search(stderr)))


class AllocationConflict(Refusal):
    """A verified competing transaction consumed the planned predecessor."""


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

    def load(self, *, event_source=None):
        require(not self.loaded, "Allocation inventory may only be loaded once")
        self.progress("all", "tag-inventory")
        refs = self.ref_oids()
        develop_head = refs.pop("refs/heads/develop", None)
        require(develop_head is not None, "Develop head could not be verified")
        # Head and watermarks come from the same advertisement. Skip this
        # push only when the advertised head is proved to supersede it.
        self.progress("all", "history")
        history = self.api.history_at(develop_head, root=self.root)
        if event_source is not None and develop_head != event_source:
            require(event_source in history.positions, "Develop history has not reached the push")
            return False
        selected, claims = [], {}
        for reference, oid in refs.items():
            name = reference.removeprefix("refs/tags/")
            if name.startswith("cma-allocation/"):
                claims[reference] = oid
                continue
            for component, deployable in DEPLOYABLES.items():
                for kind, prefix in (("pending", deployable + "/pending/v"), ("final", deployable + "/v")):
                    if name.startswith(prefix):
                        version_tuple(name[len(prefix):])
                        selected.append((component, kind, name, oid))
                        if kind == "pending":
                            self.pending_oids[name] = oid

        self.read_annotations(selected)
        pending_by_version = {(item["component"], item["version"]): item for item in self.pending}
        for tag in self.finals:
            identity = tag["receipt"]["identity"]
            item = pending_by_version.get((identity["component"], identity["version"]))
            if item is not None:
                verify_final_owner(item, tag)
        # A claim may outlive its pending tag, but its burned version must
        # remain visible as either the matching reservation or its final.
        # One listing is not an atomic snapshot: it can show another job's
        # claim before that push's reservation, so an unbacked claim is a
        # transient refusal that a fresh listing settles.
        backed = {claim_ref(tag["receipt"]["identity"]) for tag in self.finals}
        for item in self.pending:
            ref = claim_ref(item)
            if ref in claims:
                require(claims[ref] == self.pending_oids[pending_name(item)], "Allocation claim owner differs")
                backed.add(ref)
        require(claims.keys() <= backed, "Allocation claim has no durable version")
        self.loaded = True
        return self

    def ref_oids(self, *patterns):
        result = self.command("ls-remote", "--refs", REMOTE, *(patterns or ("refs/heads/develop", "refs/tags/*")), read=True)
        require(result.returncode == 0, "Allocation tag inventory is incomplete")
        # A complete advertisement this large stays as large on every read.
        require(len(result.stdout) <= 16 * 1024 * 1024, "Allocation tag inventory exceeds 16 MiB")
        refs = {}
        for line in result.stdout.splitlines():
            parts = line.split("\t")
            require(len(parts) == 2 and matches(r"[0-9a-f]{40}", parts[0])
                    and (parts[1].startswith("refs/tags/") or parts[1] == "refs/heads/develop")
                    and parts[1] not in refs,
                    "Allocation tag inventory is malformed")
            oid, reference = parts
            require(not patterns or reference in patterns, "Unexpected allocation tag ref")
            refs[reference] = oid
        return refs

    def read_annotations(self, selected):
        # The checkout supplies only immutable objects named by the live refs.
        # A ref newer than the checkout, or a tag GitHub decodes differently,
        # still goes through the existing exact-object API path.
        deadline = min(self.io.deadline, self.io.clock() + LOCAL_READ_SECONDS)
        local = LocalTags(self.root, deadline=deadline, clock=self.io.clock)
        for start in range(0, len(selected), ANNOTATION_BATCH_SIZE):
            batch = selected[start:start + ANNOTATION_BATCH_SIZE]
            tags, _ = local.read(row[-1] for row in batch)
            for kind, value in parallel_read(lambda row: self.annotation(row, tags.get(row[-1])), batch):
                (self.pending if kind == "pending" else self.finals).append(value)

    def annotation(self, row, tag=None):
        component, kind, name, oid = row
        if tag is None:
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
        require(value["component"] in DEPLOYABLES, "Unknown reservation component")
        require_publishable_release(value)
        # The plan is computed from this complete inventory, so a version it
        # already holds is a planner defect, never a race with another run.
        require(pending_name(value) not in {pending_name(item) for item in self.pending + self.planned},
                "Allocation planned a duplicate reservation")
        self.planned.append(value)
        return True

    def commit(self):
        require(self.loaded, "Allocation inventory has not been verified")
        require(not self.commit_started, "Allocation commit already attempted")
        self.commit_started = True
        if not self.planned:
            return
        refs, objects = {}, []
        # A finalizer may still own the previous pending version. Every lane
        # lets a started build finish, so allocation never claims its outcome;
        # only that finalizer writes it, atomically with the final tag.
        for value in self.planned:
            text = (f"object {value['source']}\ntype commit\ntag {pending_name(value)}\n"
                    "tagger github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com> 0 +0000\n\n"
                    + canonical(value) + "\n")
            result = self.command("mktag", data=text)
            require(result.returncode == 0 and matches(r"[0-9a-f]{40}", result.stdout.strip()),
                    "Reservation object could not be created")
            name, oid = pending_name(value), result.stdout.strip()
            refs["refs/tags/" + name] = refs[claim_ref(value)] = oid
            objects.append((name, oid, value["component"]))
            self.progress(value["component"], "reservation-push")
        # Git commits all claims and reservations together. A losing worker
        # owns no prefix and cannot leave a partly allocated matrix behind.
        try:
            pushed = self.command("push", "--atomic", "--porcelain", REMOTE,
                                  *(oid + ":" + ref for ref, oid in refs.items()))
        except UncertainPush:
            # Unlike an explicit rejection, a timeout cannot authorize another
            # plan. Accept only exact evidence for every member of this batch.
            self.settle_uncertain(refs)
        else:
            if pushed.returncode:
                self.check_conflict(refs, pushed)
            else:
                require(sum(line.startswith("*\t") and "[new tag]" in line for line in pushed.stdout.splitlines())
                        == len(refs), "An up-to-date push is not a new version reservation")
        parallel_read(lambda row: self.verify(*row), objects)
        self.pending.extend(self.planned)

    def settle_uncertain(self, expected):
        """Wait, within bounds, for a timed-out atomic push the server may still commit.

        The client can give up while the server is still applying the push; it
        then commits all refs together. Settle once every ref holds exactly its
        planned object. A ref holding any other object is a foreign or damaged
        write and is refused at once. Refs that are still absent, or a read that
        failed transiently, keep the batch uncertain while the allocation budget
        lasts; the push is never replayed.
        """
        waits = itertools.chain(SETTLE_WAITS, itertools.repeat(SETTLE_WAITS[-1]))
        while True:
            self.progress("all", "settle-poll")
            try:
                actual = self.ref_oids(*expected)
            except TransientRead:
                # A failed read proves nothing about the push; poll again.
                self.progress("all", "settle-transient")
            else:
                if actual == expected:
                    self.progress("all", "settled")
                    return
                if any(actual.get(ref) not in (None, oid) for ref, oid in expected.items()):
                    break
            # Every poll starts with the verification reserve left, so the last
            # read and the owner readbacks of a settled batch always have time.
            wait = min(next(waits), self.io.remaining() - VERIFY_RESERVE_SECONDS)
            if wait <= 0:
                break
            self.io.sleep(wait)
        self.progress("all", "settle-refused")
        require(False, "Allocation write settlement is uncertain")

    def check_conflict(self, expected, pushed):
        # Ambiguous success must match the entire transaction. A proven foreign
        # successor allows replanning; absence or malformed evidence never does.
        actual = self.ref_oids(*expected)
        if actual == expected:
            return
        for item in self.planned:
            ref = claim_ref(item)
            oid = actual.get(ref)
            if oid is None or oid == expected[ref]:
                continue
            tag = self.api.request("git/tags/" + oid)
            require(isinstance(tag, dict) and tag.get("sha") == oid
                    and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit",
                    "Allocation reservation readback differs")
            winner = validate_reservation(json.loads(tag.get("message", "")))
            require(winner["component"] == item["component"] and winner["previous"] == item["previous"]
                    and tag.get("tag") == pending_name(winner) and tag["object"].get("sha") == winner["source"],
                    "Allocation reservation readback differs")
            raise AllocationConflict("Allocation reservation collision")
        # None of the batch exists and no winner holds a claim. Only GitHub's
        # explicit refusal makes that durable; a lost connection does not.
        require(actual or not declined(pushed), "Reservation push was rejected")
        raise Refusal("Allocation reservation readback differs")

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
