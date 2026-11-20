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
from staging_cd_publication_diagnostics import ImageCommandRefusal
from staging_cd_receipts import IDENTITY_LABEL, canonical, sha256, validate_identity
from staging_cd_reservations import validate_reservation
from staging_cd_versions import DEPLOYABLES
from staging_cd_build_progress import summarize
from staging_cd_build_profile import AM_PROFILE, AM_PROFILE_LABEL, for_source as source_profile
from staging_cd_build_guard import run as run_owned_build
from staging_cd_contract import from_source, labels as contract_labels, verify_labels

ALLOWED_ARGUMENTS = {"RUNTIME_DESKTOP", "OMARCHY_CMA_BASE", "CHRONO_WEB_REPLICA_RECOVERY_ENABLED"}
PROCESS_KEYS = ("PATH", "HOME", "LANG", "LC_ALL", "DOCKER_HOST", "DOCKER_CONFIG", "BUILDX_BUILDER")


def command(args, *, cwd=None, env=None, data=None, timeout=3600, progress=None, check_owner=None):
    child = {key: value for key, value in os.environ.items() if key in PROCESS_KEYS}
    if env:
        child.update(env)
    if env and "CMA_RUNTIME_EVIDENCE_ID" in env:
        require(data is None, "Runtime canary cannot receive input credentials")
        return canary_run(args, child, timeout, cwd=cwd)
    if check_owner is None:
        result = subprocess.run(args, cwd=cwd, env=child, input=data, capture_output=True,
                                timeout=timeout, check=False)
    else:
        require(data is None and args[:3] == ["docker", "buildx", "build"], "Invalid guarded build command")
        result = run_owned_build(args, cwd=cwd, env=child, timeout=timeout, check_owner=check_owner)
    if progress is not None:
        progress.update(summarize(result.stderr))
    if result.returncode != 0:
        raise ImageCommandRefusal(args, result.stderr)
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


def identity(reservation, source, *, staging_am_opt_level=""):
    validate_reservation(reservation)
    value = recipe(source, reservation["component"])
    arguments = dict(value["build_args"], CHRONO_BUILD=reservation["version"], CHRONO_BUILD_SHA=reservation["source"],
                     OS_SECURITY_REFRESH=reservation["source"])
    level = source_profile(source, reservation["component"], staging_am_opt_level)
    if level:
        arguments[AM_PROFILE] = level
    if reservation["component"] in {"runtime", "runtime-omarchy"}:
        arguments["SOURCE_DATE_EPOCH"] = "0"
    inputs = {"tree": reservation["tree"], "dockerfile": hashlib.sha256((source / value["dockerfile"]).read_bytes()).hexdigest(),
              "context": ".", "target": value["target"], "platform": "linux/amd64", "arguments": arguments}
    compatibility = from_source(reservation["component"], reservation["source"], source)
    if compatibility is not None:
        inputs["compatibility"] = compatibility
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


def build(root, reservation, directory, runner=command, *, source=None, cache=None, progress=None,
          check_owner=None, staging_am_opt_level=""):
    if source is None:
        source = directory / "source"
        snapshot(root, reservation["source"], source, runner)
    release, value, arguments = identity(reservation, source, staging_am_opt_level=staging_am_opt_level)
    archive = directory / "image.oci.tar"
    exporter = "type=oci,dest=" + str(archive)
    if reservation["component"] in {"runtime", "runtime-omarchy"}:
        exporter += ",rewrite-timestamp=true"
    args = ["docker", "buildx", "build", "--progress=rawjson", "--platform", "linux/amd64", "--provenance=false", "--sbom=false",
            "--file", str(source / value["dockerfile"]), "--output", exporter,
            "--label", IDENTITY_LABEL + "=" + canonical(release),
            "--label", "org.opencontainers.image.source=https://github.com/ChronoAIProject/cma",
            "--label", "org.opencontainers.image.revision=" + release["source"],
            "--label", "org.opencontainers.image.version=" + release["version"]]
    if AM_PROFILE in arguments:
        args += ["--label", AM_PROFILE_LABEL + "=" + arguments[AM_PROFILE]]
    for name, label in schema_labels(reservation["component"], source).items():
        args += ["--label", name + "=" + label]
    for name, label in contract_labels(from_source(reservation["component"], release["source"], source)).items():
        args += ["--label", name + "=" + label]
    if value["target"]:
        args += ["--target", value["target"]]
    for name, argument in sorted(arguments.items()):
        args += ["--build-arg", name + "=" + argument]
    if cache is not None:
        args += cache.build_options()
    options = {"progress": progress} if progress is not None and runner is command else {}
    if check_owner is not None:
        options["check_owner"] = check_owner
    if cache is not None:
        options["env"] = cache.build_environment()
    runner([*args, str(source)], timeout=14400, **options)
    require(archive.is_file(), "Native build produced no image archive")
    return archive, release


def inspect(archive, expected, runner=command, *, allow_qualified=False, compatibility=None):
    raw = runner(["skopeo", "inspect", "--raw", "oci-archive:" + str(archive)], timeout=120)
    manifest = json.loads(raw)
    require(manifest.get("schemaVersion") == 2 and isinstance(manifest.get("config"), dict)
            and isinstance(manifest.get("layers"), list), "Native archive contains an unexpected image index")
    config_raw = runner(["skopeo", "inspect", "--config", "oci-archive:" + str(archive)], timeout=120)
    config = json.loads(config_raw)
    require(config.get("architecture") == "amd64" and config.get("os") == "linux"
            and isinstance(config.get("config"), dict), "Native image platform is invalid")
    labels = config["config"].get("Labels", {})
    require((labels.get(IDENTITY_LABEL) == canonical(expected)
             or (allow_qualified and compatibility is None and IDENTITY_LABEL not in labels))
            and labels.get("org.opencontainers.image.source") == "https://github.com/ChronoAIProject/cma"
            and labels.get("org.opencontainers.image.revision") == expected["source"]
            and labels.get("org.opencontainers.image.version") == expected["version"],
            "Built image does not match its reserved release identity")
    if compatibility is not None:
        verify_labels(labels, compatibility, expected["component"])
    expected_users = ("101", "101:101") if expected["component"] == "web" else ("10001", "10001:10001")
    require(config["config"].get("User") in expected_users,
            "Native image must use its expected unprivileged numeric user")
    return "sha256:" + hashlib.sha256(raw).hexdigest()
