"""Build, scan and publish a digest-addressed Web prepare image."""

import json
import os
from pathlib import Path
import tempfile

from image_secret_gate import checksum, verified_staging_scan
from staging_cd_build import command
from staging_cd_common import Refusal, matches, require
from staging_cd_registry import Registry, WEB_REPOSITORY
from staging_cd_versions import SEMVER
from staging_cd_web_overlay import assemble_prepare
from staging_cd_web_publication import archive_base, copy_auxiliary

DIGEST = r"sha256:[0-9a-f]{64}"


def release(reference, registry, auth, runner):
    require(matches(SEMVER, reference) or matches(DIGEST, reference),
            "Web release must be a version or digest")
    digest, _ = registry.manifest(WEB_REPOSITORY, reference)
    image = f"{WEB_REPOSITORY}@{digest}"
    config = json.loads(runner([
        "skopeo", "inspect", "--config", "--authfile", str(auth), "docker://" + image,
    ], timeout=60))
    settings = config.get("config") if isinstance(config, dict) else None
    labels = settings.get("Labels") if isinstance(settings, dict) else None
    require(isinstance(labels, dict), "Web release labels are missing")
    version = labels.get("org.opencontainers.image.version")
    source = labels.get("org.opencontainers.image.revision")
    require(matches(SEMVER, version) and matches(r"[0-9a-f]{40}", source)
            and (matches(DIGEST, reference) or reference == version),
            "Web release labels disagree with the selected release")
    return {"image": image, "version": version, "source_sha": source}


def prepare(incumbent, candidate, directory, registry, auth, runner):
    old = release(incumbent, registry, auth, runner)
    new = release(candidate, registry, auth, runner)
    require(old["image"] != new["image"], "Web releases must be distinct")
    base = archive_base(old, directory, auth, runner)
    promote = archive_base(new, directory, auth, runner)
    return assemble_prepare(base, promote, directory, max_files=1000, max_bytes=64 * 1024**2)


def publish(incumbent, candidate, directory, actor, credential, *, registry=None, runner=command):
    registry = Registry() if registry is None else registry
    auth = directory / "registry-auth.json"
    auth.write_text('{"auths":{}}', encoding="utf-8")
    auth.chmod(0o600)
    try:
        result = prepare(incumbent, candidate, directory, registry, auth, runner)
        prepared = result["prepare"]
        archive, digest = prepared["archive"], prepared["digest"]
        scanned, _ = verified_staging_scan(archive, directory / "scan", component="web")
        require(scanned == prepared["archive_sha256"] == checksum(archive),
                "Web prepare archive changed before publication")
        runner(["skopeo", "login", "--authfile", str(auth), "--username", actor,
                "--password-stdin", "ghcr.io"], data=credential.encode(), timeout=60)
        # The canonical auxiliary tag is derived from the scanned manifest.
        copy_auxiliary((archive, digest, scanned), registry, auth, runner, lambda: True, {})
        require(registry.exists(WEB_REPOSITORY, digest, settle=True),
                "Web prepare image is unavailable by digest")
    finally:
        auth.unlink(missing_ok=True)
    return {
        "image": f"{WEB_REPOSITORY}@{digest}",
        "tag": "asset-sha256-" + digest[7:],
        "incumbent_digest": result["incumbent_digest"],
        "candidate_digest": result["candidate_digest"],
        "archive_sha256": scanned,
        "asset_manifest_sha256": result["asset_manifest_sha256"],
        "asset_count": result["asset_count"],
    }


def main():
    try:
        with tempfile.TemporaryDirectory(prefix="cma-web-prepare-", dir=os.environ["RUNNER_TEMP"]) as directory:
            report = publish(os.environ["INCUMBENT"], os.environ["CANDIDATE"], Path(directory),
                             os.environ["GITHUB_ACTOR"], os.environ["REGISTRY_TOKEN"])
        summary = json.dumps(report, indent=2)
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
            stream.write("## Web prepare image\n\n```json\n" + summary + "\n```\n")
        print(summary)
    except Refusal as error:
        raise SystemExit(f"Web prepare publication refused at {error.site}") from None
    except (KeyError, TypeError, ValueError, OSError):
        raise SystemExit("Web prepare publication could not read its inputs or evidence") from None


if __name__ == "__main__":
    main()
