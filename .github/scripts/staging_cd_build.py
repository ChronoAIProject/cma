"""Build one native image with public identity and no credential-bearing inputs."""
import hashlib
import json
import os
import subprocess
import tarfile
from pathlib import Path

from ci_tracks.deployable_manifest import validate_names
from eg_schema import labels as schema_labels
from staging_cd_common import require
from staging_cd_canary_process import run as canary_run
from staging_cd_receipts import IDENTITY_LABEL, canonical, sha256, validate_identity
from staging_cd_reservations import validate_reservation
from staging_cd_versions import DEPLOYABLES

ALLOWED_ARGUMENTS = {"RUNTIME_DESKTOP", "OMARCHY_CMA_BASE", "CHRONO_WEB_REPLICA_RECOVERY_ENABLED"}
PROCESS_KEYS = ("PATH", "HOME", "LANG", "LC_ALL", "DOCKER_HOST", "DOCKER_CONFIG", "BUILDX_BUILDER")


def command(args, *, cwd=None, env=None, data=None, timeout=3600):
    child = {key: value for key, value in os.environ.items() if key in PROCESS_KEYS}
    if env:
        child.update(env)
    if env and "CMA_RUNTIME_EVIDENCE_ID" in env:
        require(data is None, "Runtime canary cannot receive input credentials")
        return canary_run(args, child, timeout, cwd=cwd)
    result = subprocess.run(args, cwd=cwd, env=child, input=data, capture_output=True,
                            timeout=timeout, check=False)
    require(result.returncode == 0, "Image operation failed; captured output is private")
    return result.stdout


def recipe(source, component):
    values = json.loads((source / ".github/scripts/deployables.json").read_text())
    require(component in DEPLOYABLES, "Unknown or retired image component")
    validate_names(values, historical=True)
    value = values[DEPLOYABLES[component]]
    require(isinstance(value, dict) and value.get("context") == "." and isinstance(value.get("dockerfile"), str)
            and value["dockerfile"] in ("web/Dockerfile", "api/crates/cma/Dockerfile", "api/crates/cmaeg/Dockerfile",
                                         "api/crates/layer4-runtime/Dockerfile")
            and "linux/amd64" in value.get("platforms", []) and isinstance(value.get("target"), str)
            and isinstance(value.get("build_args"), dict) and set(value["build_args"]).issubset(ALLOWED_ARGUMENTS),
            "Shared build recipe is unsupported")
    require(all(isinstance(v, str) and "\n" not in v for v in value["build_args"].values()),
            "Shared build arguments are invalid")
    ignore = (source / ".dockerignore").read_text().splitlines()
    require(all(pattern in ignore for pattern in (".env*", "*.pem", "*.key", ".git", ".npmrc", "kubeconfig", "credentials")),
            "Build context does not exclude required credential paths")
    require(not (source / (value["dockerfile"] + ".dockerignore")).exists(),
            "Dockerfile-specific context exclusions require review")
    require(value.get("dockerfile_sha256") == hashlib.sha256((source / value["dockerfile"]).read_bytes()).hexdigest(),
            "Dockerfile differs from the shared deployables registry")
    return value


def identity(reservation, source):
    validate_reservation(reservation)
    value = recipe(source, reservation["component"])
    arguments = dict(value["build_args"], CHRONO_BUILD=reservation["version"], CHRONO_BUILD_SHA=reservation["source"],
                     OS_SECURITY_REFRESH=reservation["source"])
    inputs = {"tree": reservation["tree"], "dockerfile": hashlib.sha256((source / value["dockerfile"]).read_bytes()).hexdigest(),
              "context": ".", "target": value["target"], "platform": "linux/amd64", "arguments": arguments}
    result = dict(reservation, schema="cma-image-release-v1", build=sha256(inputs))
    return validate_identity(result), value, arguments


def snapshot(root, revision, destination, runner=command):
    archive = destination.parent / "source.tar"
    runner(["git", "archive", "--format=tar", "--output=" + str(archive), revision], cwd=root, timeout=120)
    destination.mkdir()
    with tarfile.open(archive) as contents:
        for member in contents:
            path = Path(member.name)
            require(not path.is_absolute() and ".." not in path.parts
                    and not member.issym() and not member.islnk() and (member.isfile() or member.isdir()),
                    "Build source archive has an unsafe entry")
        contents.extractall(destination)
    archive.unlink()


def build(root, reservation, directory, runner=command, *, source=None):
    if source is None:
        source = directory / "source"
        snapshot(root, reservation["source"], source, runner)
    release, value, arguments = identity(reservation, source)
    archive = directory / "image.oci.tar"
    args = ["docker", "buildx", "build", "--platform", "linux/amd64", "--provenance=false", "--sbom=false",
            "--file", str(source / value["dockerfile"]), "--output", "type=oci,dest=" + str(archive),
            "--label", IDENTITY_LABEL + "=" + canonical(release),
            "--label", "org.opencontainers.image.source=https://github.com/ChronoAIProject/cma",
            "--label", "org.opencontainers.image.revision=" + release["source"],
            "--label", "org.opencontainers.image.version=" + release["version"]]
    for name, label in schema_labels(reservation["component"], source).items():
        args += ["--label", name + "=" + label]
    if value["target"]:
        args += ["--target", value["target"]]
    for name, argument in sorted(arguments.items()):
        args += ["--build-arg", name + "=" + argument]
    runner([*args, str(source)], timeout=14400)
    require(archive.is_file(), "Native build produced no image archive")
    return archive, release


def inspect(archive, expected, runner=command, *, allow_qualified=False):
    raw = runner(["skopeo", "inspect", "--raw", "oci-archive:" + str(archive)], timeout=120)
    manifest = json.loads(raw)
    require(manifest.get("schemaVersion") == 2 and isinstance(manifest.get("config"), dict)
            and isinstance(manifest.get("layers"), list), "Native archive contains an unexpected image index")
    config_raw = runner(["skopeo", "inspect", "--config", "oci-archive:" + str(archive)], timeout=120)
    config = json.loads(config_raw)
    require(config.get("architecture") == "amd64" and config.get("os") == "linux"
            and isinstance(config.get("config"), dict), "Native image platform is invalid")
    labels = config["config"].get("Labels", {})
    require((labels.get(IDENTITY_LABEL) == canonical(expected) or (allow_qualified and IDENTITY_LABEL not in labels))
            and labels.get("org.opencontainers.image.source") == "https://github.com/ChronoAIProject/cma"
            and labels.get("org.opencontainers.image.revision") == expected["source"]
            and labels.get("org.opencontainers.image.version") == expected["version"],
            "Built image does not match its reserved release identity")
    expected_users = ("101", "101:101") if expected["component"] == "web" else ("10001", "10001:10001")
    require(config["config"].get("User") in expected_users,
            "Native image must use its expected unprivileged numeric user")
    return "sha256:" + hashlib.sha256(raw).hexdigest()
