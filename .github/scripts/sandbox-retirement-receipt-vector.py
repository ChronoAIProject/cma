"""Check a Rust-produced retirement member and receipt with the executor."""

import json
import sys

from sandbox_retirement_application import ApplicationRetirement, digest, make_guard


def main():
    member, receipt = json.load(sys.stdin)
    assert member["runtime"]["bound_at"]
    assert member["home"]["receipt"]["native_authority"]["pvc_uid"]
    assert not member["owner"].isascii()
    row = {**member, "id": member["sandbox_id"]}
    sandbox = {**member, "lifecycle": {"home": member["home"]}}
    batch = member["batch_sha256"]
    reconstructed = make_guard(batch, row, sandbox)
    assert reconstructed == member
    fingerprint = digest(reconstructed, ordered=True)
    boundary = ApplicationRetirement(None, None, batch, None, None, 1)
    assert boundary.matches_receipt(row, fingerprint, receipt)
    adjusted = make_guard(batch, {**row, "idle_before": "2026-09-16T00:00:00Z"}, sandbox)
    assert not boundary.matches_receipt(row, digest(adjusted, ordered=True), receipt)


if __name__ == "__main__":
    main()
