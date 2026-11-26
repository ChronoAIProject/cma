"""Durable per-member intent for a single approved retirement batch."""

import fcntl
import json
import os
import re
import stat
from dataclasses import dataclass, replace


DIGEST = re.compile(r"[0-9a-f]{64}\Z")
MAX_JOURNAL_BYTES = 512 * 1024
AFTER_INTENT = {"retired", "refused", "existing_after_intent"}
WITHOUT_INTENT = {"existing_retired", "skipped"}


def digest(value):
    return isinstance(value, str) and DIGEST.fullmatch(value) is not None


class Refused(ValueError):
    pass


@dataclass(frozen=True)
class Intent:
    member_sha256: str
    guard_sha256: str
    existing: bool = False


class RetirementJournal:
    def __init__(self, path, review_sha256, inventory_sha256, *, create=True):
        if not digest(review_sha256) or not digest(inventory_sha256):
            raise Refused("invalid journal binding")
        self.path = path
        self.create = create
        self.binding = {
            "format": 1,
            "review_sha256": review_sha256,
            "inventory_sha256": inventory_sha256,
        }
        self.next_index = 0
        self.pending = None
        self.stream = None

    def __enter__(self):
        parent = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            metadata = os.fstat(parent)
            if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077:
                raise Refused("journal directory must be private")
            flags = os.O_RDWR | os.O_APPEND | os.O_NOFOLLOW | os.O_NONBLOCK
            if self.create:
                flags |= os.O_CREAT
            descriptor = os.open(self.path.name, flags, 0o600, dir_fd=parent)
            self.stream = os.fdopen(descriptor, "a+b")
            try:
                metadata = os.fstat(descriptor)
                if (
                    not stat.S_ISREG(metadata.st_mode)
                    or metadata.st_uid != os.geteuid()
                    or metadata.st_mode & 0o077
                    or metadata.st_nlink != 1
                ):
                    raise Refused("journal must be a private regular file")
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError as error:
                    raise Refused("retirement journal is already open") from error
                self._load()
                os.fsync(parent)
            except BaseException:
                self.stream.close()
                self.stream = None
                raise
        finally:
            os.close(parent)
        return self

    def __exit__(self, *_):
        self.stream.close()
        self.stream = None

    def _load(self):
        self.stream.seek(0)
        data = self.stream.read(MAX_JOURNAL_BYTES + 1)
        if not data:
            if not self.create:
                raise Refused("retained execution journal is empty")
            self._append(self.binding)
            return
        if len(data) > MAX_JOURNAL_BYTES or not data.endswith(b"\n"):
            raise Refused("journal is oversized or has an incomplete write")
        try:
            binding, *records = [json.loads(line) for line in data.splitlines()]
        except ValueError as error:
            raise Refused("journal contains invalid records") from error
        if binding != self.binding:
            raise Refused("journal belongs to a different review or inventory")
        self.next_index = 0
        self.pending = None
        for record in records:
            self._observe(record)

    def _append(self, record):
        data = json.dumps(record, sort_keys=True, separators=(",", ":")).encode() + b"\n"
        self.stream.write(data)
        self.stream.flush()
        os.fsync(self.stream.fileno())

    def _observe(self, record):
        if not isinstance(record, dict) or record.get("index") != self.next_index:
            raise Refused("journal member order is invalid")
        member = record.get("member_sha256")
        if not digest(member):
            raise Refused("invalid member identity")
        if set(record) == {"index", "member_sha256", "guard_sha256"}:
            if self.pending is not None:
                raise Refused("journal contains repeated dispatch intent")
            guard = record["guard_sha256"]
            if not digest(guard):
                raise Refused("journal guard identity is invalid")
            self.pending = Intent(member, guard)
        elif set(record) == {"index", "member_sha256", "existing"} and record["existing"] is True:
            if self.pending is None or self.pending.member_sha256 != member or self.pending.existing:
                raise Refused("journal acknowledgement does not match retained intent")
            self.pending = replace(self.pending, existing=True)
        elif set(record) == {"index", "member_sha256", "unsent"} and record["unsent"] is True:
            if self.pending is None or self.pending.member_sha256 != member or self.pending.existing:
                raise Refused("journal unsent record does not match retained intent")
            self.pending = None
        elif (
            set(record) == {"index", "member_sha256", "outcome", "evidence_sha256"}
            and record["outcome"] in AFTER_INTENT | WITHOUT_INTENT
            and digest(record["evidence_sha256"])
        ):
            expected = member if record["outcome"] in AFTER_INTENT else None
            pending = self.pending.member_sha256 if self.pending is not None else None
            if pending != expected:
                raise Refused("journal outcome does not match retained intent")
            if self.pending is not None and self.pending.existing != (record["outcome"] == "existing_after_intent"):
                raise Refused("journal outcome does not match the dispatch acknowledgement")
            self.pending = None
            self.next_index += 1
        else:
            raise Refused("journal contains an invalid outcome")

    def reserve(self, index, member, guard):
        """Return false for an existing intent. The caller then only observes."""
        if index != self.next_index or not digest(member) or not digest(guard):
            raise Refused("dispatch is outside the current inventory member")
        if self.pending is not None and self.pending.member_sha256 == member and self.pending.guard_sha256 == guard:
            return False
        if self.pending is not None:
            raise Refused("an earlier retirement remains unresolved")
        self._record({"index": index, "member_sha256": member, "guard_sha256": guard})
        return True

    def acknowledge_existing(self, index, member):
        self._record({"index": index, "member_sha256": member, "existing": True})

    def record_unsent(self, index, member):
        self._record({"index": index, "member_sha256": member, "unsent": True})

    def complete(self, index, member, outcome, evidence_sha256):
        self._record({
            "index": index,
            "member_sha256": member,
            "outcome": outcome,
            "evidence_sha256": evidence_sha256,
        })

    def _record(self, record):
        previous = (self.next_index, self.pending)
        self._observe(record)
        try:
            self._append(record)
        except BaseException:
            self.next_index, self.pending = previous
            raise
