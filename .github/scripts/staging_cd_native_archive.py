"""Build and authenticate a tested OCI archive within a protected Staging CD attempt."""
import json
import shutil
import stat
import subprocess
import time
import zipfile
import zlib
from pathlib import Path

from staging_cd_artifacts import MAX_ARTIFACT, REUSE_SECONDS, download
from staging_cd_build import IDENTITY_LABEL, command, inspect
from staging_cd_cargo_artifact import Refused as ArtifactRefused, native_producer_job, validate_artifact, validate_producer
from staging_cd_common import Refusal, matches, require
from staging_cd_contract import from_source
from staging_cd_release_archive import ARCHIVE, MANIFEST, load
from staging_cd_runtime_smoke import qualify
from staging_cd_targets import RUNTIME_TARGETS

COMPONENTS = RUNTIME_TARGETS | {"am"}
METADATA = "metadata.json"
SCHEMA = "cma-reserved-native-image-v1"
IMAGE_ID = r"sha256:[0-9a-f]{64}"
FILES = {ARCHIVE, MANIFEST, METADATA}


def artifact_name(release):
    return (f"staging-native-{release['component']}-{release['source']}-"
            f"{release['run_id']}-{release['run_attempt']}")


def image_identity(archive, release, source, runner):
    digest = inspect(archive, release, runner, compatibility=from_source(
        release["component"], release["source"], source))
    reference = "oci-archive:" + str(archive)
    manifest = json.loads(runner(["skopeo", "inspect", "--raw", reference], timeout=120))
    config_digest = manifest["config"].get("digest")
    require(matches(IMAGE_ID, config_digest), "Native image config digest is missing")
    raw = json.loads(runner(["skopeo", "inspect", "--config", reference], timeout=120))
    labels = raw["config"]["Labels"]
    stamp = json.loads(labels[IDENTITY_LABEL])
    require(stamp == release and labels["org.opencontainers.image.version"] == release["version"],
            "Native image release stamp differs")
    return digest, config_digest, stamp["build"], labels["org.opencontainers.image.version"]


def stamp_metadata(directory, release, source, runner=command):
    prepared = load(directory, release)
    require(prepared is not None, "Native archive handoff is missing")
    artifact = prepared.path
    require(artifact.stat().st_size <= MAX_ARTIFACT, "Native archive exceeds the handoff budget")
    digest, image_id, fingerprint, version = image_identity(artifact, release, source, runner)
    if release["component"] in RUNTIME_TARGETS:
        qualify(artifact, release, digest, prepared.sha256, directory, runner, source=source, check_stamp=True)
        (directory / "runtime-smoke.json").unlink()
    else:
        local = "cma-native-ci:" + release["source"]
        runner(["skopeo", "copy", "oci-archive:" + str(artifact), "docker-daemon:" + local], timeout=180)
        try:
            rows = json.loads(runner(["docker", "image", "inspect", local], timeout=30))
            require(isinstance(rows, list) and len(rows) == 1 and rows[0].get("Id") == image_id,
                    "Imported image differs from the tested archive")
            run = ["docker", "run", "--rm", "--pull=never", "--network=none", "--read-only", image_id]
            stamp = json.loads(runner([*run, "--print-build-info"], timeout=60))
            require(stamp == {"version": version, "source_sha": release["source"]},
                    "Agent Manager executable release stamp differs")
            runner([*run, "--allocator-self-test"], timeout=90)
        finally:
            runner(["docker", "image", "rm", local], timeout=120)
    metadata = {
        "schema": SCHEMA, "identity": release, "buildFingerprint": fingerprint, "versionLabel": version,
        "imageId": image_id, "manifestDigest": digest, "archiveSha256": prepared.sha256,
        "archiveBytes": artifact.stat().st_size,
    }
    with (directory / METADATA).open("x", encoding="utf-8") as stream:
        json.dump(metadata, stream, sort_keys=True)
    return metadata


def fetch(release, artifact_id, directory, source, api, token, *, downloader=download,
          runner=command, clock=time.monotonic):
    """Trust only an immutable artifact uploaded by this attempt's producer job."""
    require(release["component"] in COMPONENTS and matches(r"[1-9][0-9]{0,19}", artifact_id),
            "Native image artifact ID is invalid")
    deadline = clock() + REUSE_SECONDS
    origin = f"actions/runs/{release['run_id']}/attempts/{release['run_attempt']}"
    run = api.request(origin, deadline=deadline)
    jobs = api.request(origin + "/jobs?per_page=100", deadline=deadline)
    artifact = api.request("actions/artifacts/" + artifact_id, deadline=deadline)
    validate_artifact(artifact, artifact_name(release), max_bytes=MAX_ARTIFACT)
    producer = validate_producer(run, jobs, artifact, release["component"],
                                 job_name=native_producer_job(release["component"]))
    require(producer == {"producer_run": release["run_id"], "producer_attempt": release["run_attempt"],
                         "producer_source": release["source"]}
            and artifact["id"] == int(artifact_id), "Native image producer differs from the reservation")

    bundle = directory / "native.zip"
    output = directory / "reserved-native"
    output.mkdir()
    downloader(artifact, bundle, token, deadline=deadline)
    try:
        unpack(bundle, output, deadline, clock)
    finally:
        bundle.unlink()
    prepared = load(output, release)
    metadata = json.loads((output / METADATA).read_text(encoding="utf-8"))
    digest, image_id, fingerprint, version = image_identity(prepared.path, release, source, runner)
    require(prepared.cache_day is None and prepared.cache_namespace is None and metadata == {
        "schema": SCHEMA, "identity": release, "buildFingerprint": fingerprint, "versionLabel": version,
        "imageId": image_id, "manifestDigest": digest, "archiveSha256": prepared.sha256,
        "archiveBytes": prepared.path.stat().st_size,
    }, "Native image metadata differs from the inspected release")
    return prepared


def unpack(bundle, output, deadline, clock):
    with zipfile.ZipFile(bundle) as compressed:
        entries = compressed.infolist()
        require(len(entries) == len(FILES) and {entry.filename for entry in entries} == FILES,
                "Native archive entries differ")
        require(sum(entry.file_size for entry in entries) <= MAX_ARTIFACT, "Native archive is oversized")
        for entry in entries:
            limit = MAX_ARTIFACT if entry.filename == ARCHIVE else 8192
            mode = (entry.external_attr >> 16) & 0o170000
            require(0 < entry.file_size <= limit and mode in (0, stat.S_IFREG)
                    and not entry.flag_bits & 1 and entry.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED),
                    "Native archive entries are unsafe")
            with compressed.open(entry) as incoming, (output / entry.filename).open("xb") as outgoing:
                while chunk := incoming.read(1024 * 1024):
                    require(clock() < deadline, "Native archive extraction budget expired")
                    outgoing.write(chunk)


def try_fetch(release, artifact_id, directory, source, api, token, on_miss):
    try:
        return fetch(release, artifact_id, directory, source, api, token)
    except (Refusal, ArtifactRefused, OSError, subprocess.SubprocessError, ValueError, TypeError,
            KeyError, AttributeError, zipfile.BadZipFile, zlib.error):
        # A failed optional transfer does not supply publication authority.
        # Remove only our scratch copy before giving the builder that space.
        shutil.rmtree(directory / "reserved-native", ignore_errors=True)
        (directory / "native.zip").unlink(missing_ok=True)
        on_miss("reserved-native-unavailable")
        return None
