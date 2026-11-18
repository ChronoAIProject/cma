"""Qualify the scanned runtime archive before publication or default promotion."""
import hashlib
import json
from pathlib import Path
import uuid

from staging_cd_common import require
from staging_cd_canary_process import CanaryProcessFailure
from staging_cd_common import Refusal
from staging_cd_targets import RUNTIME_TARGETS


class CleanupIncomplete(Refusal):
    def __init__(self, campaign, quiesced, reconciled):
        super().__init__("Runtime canary cleanup incomplete; retain the campaign receipt")
        self.receipt = {"schema": "cma-runtime-cleanup-v1", "campaign": campaign,
                        "helpers_quiesced": quiesced, "inventory_reconciled": reconciled,
                        "cleanup": "incomplete"}


def reconcile(runner, campaign, local):
    label = "dev.chrono.cma.evidence=" + campaign
    containers = runner(["docker", "ps", "--all", "--quiet", "--filter", "label=" + label], timeout=30)
    for container in containers.decode().split():
        require(12 <= len(container) <= 64 and all(c in "0123456789abcdef" for c in container),
                "Runtime cleanup container identity is invalid")
        runner(["docker", "rm", "--force", container], timeout=30)
    volumes = runner(["docker", "volume", "ls", "--quiet", "--filter", "label=" + label], timeout=30)
    for volume in volumes.decode().split():
        require(volume == campaign, "Runtime cleanup volume identity differs")
        runner(["docker", "volume", "rm", volume], timeout=30)
    require(not runner(["docker", "ps", "--all", "--quiet", "--filter", "label=" + label], timeout=30).strip()
            and not runner(["docker", "volume", "ls", "--quiet", "--filter", "label=" + label], timeout=30).strip(),
            "Runtime cleanup remains incomplete")
    runner(["docker", "image", "rm", local], timeout=30)


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
    quiesced, interrupted = True, False
    try:
        runner(["skopeo", "copy", "oci-archive:" + str(archive), "docker-daemon:" + local], timeout=180)
        manifest = json.loads(runner(["skopeo", "inspect", "--raw", "oci-archive:" + str(archive)], timeout=30))
        imported = json.loads(runner(["docker", "image", "inspect", local], timeout=30))
        require(isinstance(imported, list) and len(imported) == 1
                and imported[0].get("Id") == manifest["config"]["digest"],
                "Imported runtime differs from scanned image config")
        try:
            runner(["node", str(Path(__file__).resolve().with_name("runtime-image-evidence.mjs")),
                    "run", local, identity["source"], str(evidence.resolve())], timeout=300,
                   env={"CMA_RUNTIME_EVIDENCE_ID": campaign,
                        "CMA_RUNTIME_EVIDENCE_DOCKERFILE": str(source / "api/crates/layer4-runtime/Dockerfile")})
        except BaseException as error:
            interrupted = True
            quiesced = isinstance(error, CanaryProcessFailure) and error.quiesced
            raise
        raw = evidence.read_bytes()
        require(len(raw) <= 1048576, "Runtime smoke evidence is oversized")
        result = json.loads(raw)
        require(result.get("result") == "pass" and result.get("commit") == identity["source"]
                and result.get("cleanup") == {"containers": 0, "volumes": 0},
                "Runtime smoke or cleanup failed")
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
            raise CleanupIncomplete(campaign, False, False) from None
        try:
            reconcile(runner, campaign, local)
        except BaseException:
            raise CleanupIncomplete(campaign, True, False) from None
        if interrupted:
            raise CleanupIncomplete(campaign, True, True) from None
