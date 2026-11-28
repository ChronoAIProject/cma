"""Build, qualify and publish an immutable staging Desktop fixture."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from image_secret_gate import checksum, verified_staging_scan
from os_refresh_key import refresh_key
from staging_cd_build import command, recipe, snapshot
from staging_cd_common import Refusal, matches, require
from staging_cd_publish import push
from staging_cd_registry import Registry, STAGING_DESKTOP_TAG_PREFIX
from staging_cd_versions import DEPLOYABLES


ROOT = Path(__file__).resolve().parents[2]
VARIANTS = {
    "debian": ("runtime", "supervised-debian-desktop"),
    "omarchy": ("runtime-omarchy", "supervised-omarchy"),
}
SUPERVISION = "CHRONO_RUNTIME_DESKTOP_SUPERVISION_ENABLED"
DIGEST = r"sha256:[0-9a-f]{64}"


def require_staging_source(revision, env, runner):
    require(env.get("GITHUB_REPOSITORY") == "ChronoAIProject/cma"
            and env.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and env.get("GITHUB_REF") == "refs/heads/develop"
            and env.get("RUNNER_OS") == "Linux" and env.get("RUNNER_ARCH") == "X64",
            "Desktop publication requires the native develop workflow")
    require(matches(r"[0-9a-f]{40}", revision), "Desktop fixture source must be a full commit SHA")
    history = runner(["git", "rev-list", "--first-parent", "HEAD"], cwd=ROOT, timeout=30)
    require(revision.encode() in history.splitlines(), "Desktop fixture source is outside develop history")


def build_archive(variant, revision, directory, runner):
    component, target = VARIANTS[variant]
    source = directory / "source"
    snapshot(ROOT, revision, source, runner)
    value = recipe(source, component)
    dockerfile = source / value["dockerfile"]
    arguments = dict(value["build_args"], RUNTIME_DESKTOP="1", SOURCE_DATE_EPOCH="0",
                     CHRONO_BUILD="git-" + revision, CHRONO_BUILD_SHA=revision,
                     OS_SECURITY_REFRESH=refresh_key(component, dockerfile.read_text(), revision))
    archive = directory / "desktop.oci.tar"
    args = ["docker", "buildx", "build", "--platform", "linux/amd64", "--provenance=false",
            "--sbom=false", "--file", str(dockerfile), "--target", target,
            "--output", f"type=oci,dest={archive},rewrite-timestamp=true"]
    for key, argument in sorted(arguments.items()):
        args += ["--build-arg", f"{key}={argument}"]
    runner([*args, str(source)], cwd=source, timeout=10800)
    require(archive.is_file(), "Desktop build did not produce an OCI archive")
    return archive, source


def inspect_archive(archive, variant, revision, runner):
    uri = "oci-archive:" + str(archive)
    raw = runner(["skopeo", "inspect", "--raw", uri], timeout=60)
    manifest = json.loads(raw)
    require(manifest.get("schemaVersion") == 2 and isinstance(manifest.get("layers"), list)
            and isinstance(manifest.get("config"), dict), "Expected one Desktop image manifest")
    config_digest = manifest["config"].get("digest")
    require(matches(DIGEST, config_digest), "Invalid Desktop config digest")
    config_raw = runner(["skopeo", "inspect", "--raw", "--config", uri], timeout=60)
    require(config_digest == "sha256:" + hashlib.sha256(config_raw).hexdigest(),
            "Desktop config does not match its manifest")
    config = json.loads(config_raw)
    settings = config.get("config", {})
    labels = settings.get("Labels", {})
    requires_omarchy = variant == "omarchy"
    require(config.get("os") == "linux" and config.get("architecture") == "amd64"
            and settings.get("User") == "10001:10001"
            and labels.get("org.opencontainers.image.source") == "https://github.com/ChronoAIProject/cma"
            and labels.get("org.opencontainers.image.revision") == revision
            and labels.get("dev.chrono.cma.runtime-desktop") == "1"
            and ("dev.chrono.cma.desktop-environment" in labels) == requires_omarchy,
            "Desktop image identity differs from the selected fixture")
    require([entry for entry in settings.get("Env", []) if entry.startswith(SUPERVISION + "=")]
            == [SUPERVISION + "=true"], "Desktop image supervision is not enabled")
    return "sha256:" + hashlib.sha256(raw).hexdigest(), config_digest, labels


def qualify(archive, component, config_digest, revision, directory, runner):
    # Docker loads the OCI image for the Desktop tests. Bind the smoke result
    # back to that config because the registry copy uses the original archive.
    local = "cma-staging-desktop-fixture:" + revision
    runner(["skopeo", "copy", "oci-archive:" + str(archive), "docker-daemon:" + local], timeout=240)
    try:
        loaded = runner(["docker", "image", "inspect", "--format", "{{.Id}}", local], timeout=60)
        require(loaded.decode().strip() == config_digest, "Loaded Desktop config differs from the archive")
        runner(["python3", "-B", str(ROOT / ".github/scripts/runtime_image_hygiene.py"),
                "--image", local, "--component", component,
                "--output", str(directory / "hygiene.json")], cwd=ROOT, timeout=1200)
        smoke = directory / "desktop.json"
        runner(["node", ".github/scripts/desktop-runtime-evidence.mjs", local, revision, str(smoke)],
               cwd=ROOT, timeout=900)
        evidence = json.loads(smoke.read_text())
        require(evidence.get("image") == config_digest and evidence.get("revision") == revision
                and evidence.get("result") == "pass" and evidence.get("cleanup") is True,
                "Desktop smoke receipt does not bind the qualified config")
        return checksum(smoke)
    finally:
        runner(["docker", "image", "rm", local], timeout=120)


def publish_archive(archive, component, digest, config_digest, scanned, directory, actor, credential, registry, runner):
    require(checksum(archive) == scanned, "Desktop archive changed after scanning")
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[component]
    tag = STAGING_DESKTOP_TAG_PREFIX + digest[7:]
    if registry.manifest(repository, tag, absent=True) is None:
        auth = directory / "registry-auth.json"
        auth.touch(mode=0o600, exist_ok=False)
        auth.write_text('{"auths":{}}', encoding="utf-8")
        try:
            runner(["skopeo", "login", "--authfile", str(auth), "--username", actor,
                    "--password-stdin", "ghcr.io"], data=credential.encode(), timeout=60)
            push(archive, repository, tag, digest, auth, registry, runner,
                 lambda: True, {})
        finally:
            auth.unlink()
    published = registry.manifest(repository, digest, settle=True)
    tagged = registry.manifest(repository, tag, settle=True)
    require(published == tagged and published[0] == digest
            and published[1]["config"]["digest"] == config_digest and checksum(archive) == scanned,
            "Registry readback differs from the qualified Desktop archive")
    return f"{repository}@{digest}", tag


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--variant", choices=VARIANTS, required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    try:
        require_staging_source(args.source, os.environ, command)
        component, _ = VARIANTS[args.variant]
        actor, credential = os.environ["GITHUB_ACTOR"], os.environ["REGISTRY_TOKEN"]
        registry = Registry(credential=credential, actor=actor)
        with tempfile.TemporaryDirectory(prefix="cma-staging-desktop-", dir=os.environ["RUNNER_TEMP"]) as work:
            directory = Path(work)
            archive, source = build_archive(args.variant, args.source, directory, command)
            digest, config, labels = inspect_archive(archive, args.variant, args.source, command)
            smoke = qualify(archive, component, config, args.source, directory, command)
            scanned, _ = verified_staging_scan(archive, directory / "scan", component=component, source=source)
            image, tag = publish_archive(archive, component, digest, config, scanned, directory,
                                         actor, credential, registry, command)
        receipt = {"schema": "cma-staging-desktop-publication-v1", "variant": args.variant,
                   "image": image, "tag": tag, "platform": "linux/amd64", "source_revision": args.source,
                   "config_digest": config, "image_labels": labels, "archive_sha256": scanned,
                   "desktop_evidence_sha256": smoke, "publication_run": int(os.environ["GITHUB_RUN_ID"])}
        with args.receipt.open("x", encoding="utf-8") as out:
            out.write(json.dumps(receipt, indent=2) + "\n")
    except (Refusal, OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise SystemExit(f"Staging Desktop publication refused ({type(error).__name__})") from None


if __name__ == "__main__":
    main()
