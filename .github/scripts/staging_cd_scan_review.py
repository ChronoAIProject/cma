#!/usr/bin/env python3
"""Refuse a release whose scanner bundle the pinned deployment controller has not reviewed.

Each receipt names the scanner version and bundle manifest that published it,
and the controller accepts only pairs in its reviewed set. A bundle change can
merge here before the controller's review does. On 21 Nov 2026 every runtime
and Omarchy release after one did so built, scanned and published for up to
30 minutes, then failed deployment with exit 40. This runs in the
infrastructure job, before any release job starts building, against the exact
controller revision that later deploys.
"""
import ast
import base64
import os
import re
import sys

from ci_outcome import report
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_app_metadata import REPOSITORY, github_reader
from staging_cd_common import Refusal, matches, require
from staging_cd_receipts import SUPPORTED_SCANS
from staging_cd_runtime import run_private

POLICY = "staging/scripts/reviewed_scan.py"
DIGEST = re.compile(r"[0-9a-f]{64}")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


def publication_scan():
    """The (version, bundle) pair a publication from this checkout records."""
    pairs = [pair for pair in SUPPORTED_SCANS if pair[1] == MANIFEST_SHA256]
    require(len(pairs) == 1, "The pinned scanner bundle has no single supported version")
    return pairs[0]


def reviewed_scans(source):
    """The literal REVIEWED_SCANS pairs in the controller's policy module.

    The module is parsed, never imported or executed. Anything but one literal
    frozenset of (version, 64-hex digest) pairs refuses.
    """
    tree = ast.parse(source)
    values = [node.value for node in tree.body if isinstance(node, ast.Assign)
              and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
              and node.targets[0].id == "REVIEWED_SCANS"]
    require(len(values) == 1, "Controller scan policy is not a single declaration")
    call = values[0]
    require(isinstance(call, ast.Call) and isinstance(call.func, ast.Name) and call.func.id == "frozenset"
            and len(call.args) == 1 and not call.keywords, "Controller scan policy is not a frozenset")
    try:
        pairs = ast.literal_eval(call.args[0])
    except ValueError:
        raise Refusal("Controller scan policy is not a literal set") from None
    require(isinstance(pairs, set) and pairs and all(
        isinstance(pair, tuple) and len(pair) == 2 and isinstance(pair[0], str) and VERSION.fullmatch(pair[0])
        and isinstance(pair[1], str) and DIGEST.fullmatch(pair[1]) for pair in pairs),
        "Controller scan policy pairs are invalid")
    return frozenset(pairs)


def controller_policy(token, revision, reader_factory=github_reader):
    require(matches(r"[0-9a-f]{40}", revision), "Controller revision is invalid")
    value = reader_factory(token)(f"repos/{REPOSITORY}/contents/{POLICY}?ref={revision}")
    require(value.get("type") == "file" and value.get("encoding") == "base64"
            and isinstance(value.get("content"), str), "Controller scan policy could not be read")
    return base64.b64decode(value["content"], validate=False).decode("utf-8")


def check(env, masker, reader_factory=github_reader):
    token = env.get("CD_INFRA_TOKEN")
    masker(token)
    scan = publication_scan()
    try:
        accepted = reviewed_scans(controller_policy(token, env.get("CD_INFRA_REVISION", ""), reader_factory))
    except Exception as error:  # noqa: BLE001 - every read or parse failure has one public reason.
        raise Unreadable() from error
    if scan not in accepted:
        raise Unreviewed()
    return True


class Unreviewed(Exception):
    """The controller revision does not list this checkout's scanner bundle."""


class Unreadable(Exception):
    """The controller's reviewed scan set could not be read or parsed."""


def main(env=None, reader_factory=github_reader):
    env = os.environ if env is None else env
    outcome = {}

    def operation(masker):
        try:
            return check(env, masker, reader_factory)
        except Unreviewed:
            outcome["code"] = "unreviewed"
        except Unreadable:
            outcome["code"] = "unreadable"
        raise Refusal("Scanner bundle review is not established")

    status = run_private(operation, None, subject="scan-review", explain=False)
    if status:
        code = outcome.get("code")
        if code == "unreviewed":
            report("scan-review", "unreviewed", subject="scan-review")
            # The bundle digest is this public checkout's own pinned value.
            print("SCAN-REVIEW bundle=" + MANIFEST_SHA256, flush=True)
        elif code == "unreadable":
            report("scan-review", "unreadable", subject="scan-review")
        else:
            report("step", "error", subject="scan-review")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
