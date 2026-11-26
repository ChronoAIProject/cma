"""Qualify the scanned runtime archive before publication or default promotion."""
import hashlib
import json
from pathlib import Path
import subprocess
import time
import uuid

from staging_cd_common import require
from staging_cd_canary_process import CanaryProcessFailure
from staging_cd_common import Refusal
from staging_cd_targets import RUNTIME_TARGETS


# Reconciliation is idempotent: it removes only this campaign's labelled
# containers and volume, then its own local tag. A slow Docker daemon on a
# shared runner (a multi-gigabyte image removal, a lagging forced removal) must
# not fail a release whose canary already passed, so one further pass follows a
# short wait. Identity refusals are never repeated: they mean the inventory is
# not what this campaign created.
CLEANUP_ATTEMPTS = 2
CLEANUP_RETRY_SECONDS = 5
LIST_SECONDS = 30
CONTAINER_REMOVE_SECONDS = 60
IMAGE_REMOVE_SECONDS = 120
# Fixed vocabularies for the public cleanup receipt; never helper or daemon text.
SMOKE_STATES = ("not_run", "interrupted", "failed", "passed")
CLEANUP_STEPS = ("quiesce", "container_list", "container_remove", "volume_list", "volume_remove",
                 "verify_empty", "image_remove", "interrupted", "unknown")
CLEANUP_FAILURES = ("helpers_unquiesced", "timeout", "command_failed", "incomplete", "identity_refused",
                    "interrupted", "error")


class CleanupIncomplete(Refusal):
    def __init__(self, campaign, quiesced, reconciled, *, smoke="not_run", step=None, failure=None,
                 attempts=0):
        super().__init__("Runtime canary cleanup incomplete; retain the campaign receipt")
        self.receipt = {"schema": "cma-runtime-cleanup-v2", "campaign": campaign,
                        "helpers_quiesced": quiesced, "inventory_reconciled": reconciled,
                        "cleanup": "incomplete",
                        "smoke": smoke if smoke in SMOKE_STATES else "not_run",
                        "failed_step": step if step in CLEANUP_STEPS else None,
                        "failure": failure if failure in CLEANUP_FAILURES else None,
                        "attempts": attempts}


class ReconcileFailure(Exception):
    """One reconciliation pass failed at a named step; carries no daemon text."""

    def __init__(self, step, failure):
        super().__init__(step + ":" + failure)
        self.step, self.failure = step, failure


def _call(runner, step, args, timeout):
    try:
        return runner(args, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise ReconcileFailure(step, "timeout") from None
    except Refusal:
        raise ReconcileFailure(step, "command_failed") from None


def _reconcile_once(runner, campaign, local):
    label = "dev.chrono.cma.evidence=" + campaign
    containers = _call(runner, "container_list",
                       ["docker", "ps", "--all", "--quiet", "--filter", "label=" + label], LIST_SECONDS)
    for container in containers.decode().split():
        if not (12 <= len(container) <= 64 and all(c in "0123456789abcdef" for c in container)):
            raise ReconcileFailure("container_list", "identity_refused")
        _call(runner, "container_remove", ["docker", "rm", "--force", container], CONTAINER_REMOVE_SECONDS)
    volumes = _call(runner, "volume_list",
                    ["docker", "volume", "ls", "--quiet", "--filter", "label=" + label], LIST_SECONDS)
    for volume in volumes.decode().split():
        if volume != campaign:
            raise ReconcileFailure("volume_list", "identity_refused")
        _call(runner, "volume_remove", ["docker", "volume", "rm", volume], LIST_SECONDS)
    remaining = (_call(runner, "verify_empty",
                       ["docker", "ps", "--all", "--quiet", "--filter", "label=" + label], LIST_SECONDS).strip()
                 or _call(runner, "verify_empty",
                          ["docker", "volume", "ls", "--quiet", "--filter", "label=" + label], LIST_SECONDS).strip())
    if remaining:
        raise ReconcileFailure("verify_empty", "incomplete")
    try:
        _call(runner, "image_remove", ["docker", "image", "rm", local], IMAGE_REMOVE_SECONDS)
    except ReconcileFailure as failure:
        # The tag is unique to this attempt, so its absence proves an earlier
        # pass's removal completed after its command outlasted the bound.
        if failure.failure != "command_failed" or _call(
                runner, "image_remove", ["docker", "image", "ls", "--quiet", local], LIST_SECONDS).strip():
            raise


def reconcile(runner, campaign, local, *, attempts=CLEANUP_ATTEMPTS, sleep=None):
    """Remove this campaign's resources; return the number of passes used.

    Raises ReconcileFailure naming the last failed step when every pass failed.
    """
    for attempt in range(1, attempts + 1):
        try:
            _reconcile_once(runner, campaign, local)
            return attempt
        except ReconcileFailure as failure:
            if failure.failure == "identity_refused" or attempt == attempts:
                failure.attempts = attempt
                raise
        except Exception:
            failure = ReconcileFailure("unknown", "error")
            if attempt == attempts:
                failure.attempts = attempt
                raise failure from None
        (sleep or time.sleep)(CLEANUP_RETRY_SECONDS)
    raise AssertionError("unreachable")


def validate_smoke(value, *, source, digest, archive):
    require(isinstance(value, dict) and set(value) == {
        "schema", "source", "digest", "archive", "evidence", "scope", "passed"
    } and value["schema"] == "cma-runtime-smoke-v1"
        and value["source"] == source and value["digest"] == digest
        and value["archive"] == archive and value["passed"] is True
        and value["scope"] == "local-contract-3-no-provider-or-model"
        and isinstance(value["evidence"], str) and len(value["evidence"]) == 64
        and all(c in "0123456789abcdef" for c in value["evidence"]),
        "Runtime image smoke receipt is missing or differs")
    return value


def qualify(archive, identity, digest, scanned, directory, runner, *, source=None):
    if identity["component"] not in RUNTIME_TARGETS:
        return None
    source = Path.cwd() if source is None else source
    attempt = str(uuid.uuid4())
    local = "cma-runtime-qualification:" + attempt
    campaign = "cma-runtime-ci-" + attempt
    evidence = directory / "runtime-smoke.json"
    quiesced, interrupted, smoke = True, False, "not_run"
    try:
        runner(["skopeo", "copy", "oci-archive:" + str(archive), "docker-daemon:" + local], timeout=180)
        manifest = json.loads(runner(["skopeo", "inspect", "--raw", "oci-archive:" + str(archive)], timeout=30))
        imported = json.loads(runner(["docker", "image", "inspect", local], timeout=30))
        require(isinstance(imported, list) and len(imported) == 1
                and imported[0].get("Id") == manifest["config"]["digest"],
                "Imported runtime differs from scanned image config")
        # The Omarchy desktop image carries the desktop Codex pins and probes; the
        # default runtime keeps the unchanged default contract.
        profile = {"CMA_RUNTIME_EVIDENCE_PROFILE": "omarchy"} if identity["component"] == "runtime-omarchy" else {}
        try:
            runner(["node", str(Path(__file__).resolve().with_name("runtime-image-evidence.mjs")),
                    "run", local, identity["source"], str(evidence.resolve())], timeout=300,
                   env={"CMA_RUNTIME_EVIDENCE_ID": campaign,
                        "CMA_RUNTIME_EVIDENCE_DOCKERFILE": str(source / "api/crates/layer4-runtime/Dockerfile"),
                        **profile})
        except BaseException as error:
            interrupted, smoke = True, "interrupted"
            quiesced = isinstance(error, CanaryProcessFailure) and error.quiesced
            raise
        smoke = "failed"
        raw = evidence.read_bytes()
        require(len(raw) <= 1048576, "Runtime smoke evidence is oversized")
        result = json.loads(raw)
        require(result.get("result") == "pass" and result.get("commit") == identity["source"]
                and result.get("cleanup") == {"containers": 0, "volumes": 0},
                "Runtime smoke or cleanup failed")
        smoke = "passed"
        return validate_smoke({"schema": "cma-runtime-smoke-v1", "source": identity["source"],
                               "digest": digest, "archive": scanned, "passed": True,
                               "scope": "local-contract-3-no-provider-or-model",
                               "evidence": hashlib.sha256(raw).hexdigest()},
                              source=identity["source"], digest=digest, archive=scanned)
    finally:
        # Never race inventory against a live creator. Even after killing all
        # helpers, Docker may still complete an accepted daemon-side request:
        # empty lists after interruption cannot prove completed cleanup.
        if not quiesced:
            raise CleanupIncomplete(campaign, False, False, smoke=smoke, step="quiesce",
                                    failure="helpers_unquiesced") from None
        try:
            passes = reconcile(runner, campaign, local)
        except ReconcileFailure as failure:
            raise CleanupIncomplete(campaign, True, False, smoke=smoke, step=failure.step,
                                    failure=failure.failure, attempts=getattr(failure, "attempts", 0)) from None
        except BaseException:
            raise CleanupIncomplete(campaign, True, False, smoke=smoke, step="interrupted",
                                    failure="interrupted") from None
        if interrupted:
            raise CleanupIncomplete(campaign, True, True, smoke=smoke, step="interrupted",
                                    failure="interrupted", attempts=passes) from None
