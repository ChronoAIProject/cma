"""Walk the approved member order with read-only recovery of ambiguous writes."""

from dataclasses import dataclass
from enum import Enum


class Eligibility(Enum):
    ELIGIBLE = "eligible"
    EXISTING = "existing"
    SKIP = "skip"
    UNKNOWN = "unknown"


class Admission(Enum):
    ACCEPTED = "accepted"
    EXISTING = "existing"
    REFUSED = "refused"
    UNKNOWN = "unknown"
    UNSENT = "unsent"


@dataclass(frozen=True)
class Observation:
    # The boundary returns this only after checking the original application
    # transition and the independent provider disposition.
    evidence_sha256: str


def execute(members, journal, boundary, approval):
    """Return false when the current member remains unresolved."""
    for index in range(journal.next_index, len(members)):
        member = members[index]
        identity = member["member_sha256"]
        if journal.pending is not None:
            if journal.pending.member_sha256 != identity:
                raise ValueError("retained intent does not match the approved member")
            guard = None if journal.pending.existing else journal.pending.guard_sha256
            observation = boundary.observe(member, guard_sha256=guard)
            if observation is None:
                return False
            outcome = "existing_after_intent" if journal.pending.existing else "retired"
            journal.complete(index, identity, outcome, observation.evidence_sha256)
            continue

        eligibility, evidence = boundary.preflight(member)
        if eligibility is Eligibility.SKIP:
            journal.complete(index, identity, "skipped", evidence)
            continue
        if eligibility is Eligibility.EXISTING:
            observation = boundary.observe(member, guard_sha256=None)
            if observation is None:
                return False
            journal.complete(index, identity, "existing_retired", observation.evidence_sha256)
            continue
        if eligibility is not Eligibility.ELIGIBLE or approval is None or not approval():
            return False

        if not journal.reserve(index, identity, evidence):
            raise ValueError("retirement intent already exists")
        admission, response_sha256 = boundary.dispatch(member, approval.before_send)
        if admission is Admission.UNSENT:
            journal.record_unsent(index, identity)
            return False
        if admission is Admission.REFUSED:
            journal.complete(index, identity, "refused", response_sha256)
            continue
        if admission is Admission.EXISTING:
            journal.acknowledge_existing(index, identity)
        guard = None if admission is Admission.EXISTING else evidence
        observation = boundary.observe(member, guard_sha256=guard)
        if observation is None:
            return False
        outcome = "existing_after_intent" if admission is Admission.EXISTING else "retired"
        journal.complete(index, identity, outcome, observation.evidence_sha256)
    return True
