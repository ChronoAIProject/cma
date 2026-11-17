"""Install checksum-pinned deployment tools outside source and private checkouts."""
import hashlib
import io
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

from staging_cd_checkout import checkout_path
from staging_cd_common import require
from staging_cd_runtime import run_private

NYXID_URL = "https://github.com/ChronoAIProject/NyxID/releases/download/v0.78.0/nyxid-cli-x86_64-unknown-linux-gnu.tar.gz"
NYXID_SHA256 = "875cebec39ccdfdbf2f9faee3c2062f0525015df08a9e5aec2bb556840718bd5"
NYXID_MEMBER = "nyxid-cli-x86_64-unknown-linux-gnu/nyxid"
BUILDX_URL = "https://github.com/docker/buildx/releases/download/v0.30.1/buildx-v0.30.1.linux-amd64"
BUILDX_SHA256 = "c37114fcd034025ec68e224657c8a5a850df472ded3ddcbca75ad3a7ebb9710d"
REQUIREMENTS = ("PyYAML==6.0.3", "jsonschema==4.25.1")


def tools_path(env):
    temporary = Path(env["RUNNER_TEMP"])
    require(temporary.is_absolute() and temporary.is_dir() and not temporary.is_symlink(),
            "Runner temporary directory is invalid")
    root = temporary / "cma-deploy-tools"
    require(not root.is_symlink(), "Deployment tool links are forbidden")
    return root


def environment(env):
    root = tools_path(env)
    for path in ("home", "docker", "docker/cli-plugins", "bin", "venv", "venv/bin"):
        require((root / path).is_dir() and not (root / path).is_symlink(),
                "Deployment tools are unavailable")
    require(all((root / path).is_file() and not (root / path).is_symlink()
                for path in ("bin/nyxid", "docker/cli-plugins/docker-buildx", "docker/config.json")),
            "Deployment tool executable is unavailable")
    # These empty homes contain no runner login, App token or registry auth.
    # GHCR images are public; Docker needs this config only to discover Buildx.
    return {"HOME": str(root / "home"), "DOCKER_CONFIG": str(root / "docker"),
            "PATH": str(root / "bin") + os.pathsep + env["PATH"]}


def download(url, digest):
    with urllib.request.urlopen(url, timeout=60) as response:
        require(response.status == 200, "Deployment tool download failed")
        data = response.read(128 * 1024 * 1024 + 1)
    require(len(data) <= 128 * 1024 * 1024 and hashlib.sha256(data).hexdigest() == digest,
            "Deployment tool checksum differs")
    return data


def executable(path, content):
    path.write_bytes(content)
    path.chmod(0o700)


def install(env, fetch=download, runner=subprocess.run):
    require(sys.platform == "linux" and os.uname().machine == "x86_64",
            "Deployment tooling requires the reviewed Linux runner")
    root = tools_path(env)
    require(not root.exists(), "Deployment tools already exist")
    root.mkdir(mode=0o700)
    for path in ("home", "bin", "docker/cli-plugins"):
        (root / path).mkdir(parents=True, mode=0o700)
    (root / "docker/config.json").write_text("{}\n", encoding="utf-8")
    child_env = {key: env[key] for key in ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP") if key in env}
    child_env.update(HOME=str(root / "home"), DOCKER_CONFIG=str(root / "docker"))
    def command(args):
        result = runner(args, env=child_env, capture_output=True, text=True, timeout=300, check=False)
        require(result.returncode == 0, "Deployment tool setup failed")
        return result.stdout
    command([sys.executable, "-m", "venv", str(root / "venv")])
    archive = fetch(NYXID_URL, NYXID_SHA256)
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
        members = [entry for entry in bundle.getmembers() if entry.name == NYXID_MEMBER]
        require(len(members) == 1 and members[0].isreg() and members[0].size <= 128 * 1024 * 1024,
                "Deployment CLI archive is invalid")
        stream = bundle.extractfile(members[0])
        require(stream is not None, "Deployment CLI archive is empty")
        executable(root / "bin/nyxid", stream.read())
    executable(root / "docker/cli-plugins/docker-buildx", fetch(BUILDX_URL, BUILDX_SHA256))
    child_env.update(environment(env))
    help_text = command([str(root / "bin/nyxid"), "proxy", "request", "--help"])
    require("--access-token-env" in help_text and "--data" in help_text and "--header" in help_text,
            "Deployment CLI interface differs")
    require("v0.30.1" in command(["docker", "buildx", "version"]), "Buildx version differs")
    command(["docker", "buildx", "imagetools", "inspect", "--help"])
    require(command(["gitleaks", "version"]).strip() == "8.30.1", "Scanner backend version differs")
    return True


def install_requirements(env, runner=subprocess.run):
    root = tools_path(env)
    staging = checkout_path(env) / "staging"
    requirements = staging / "requirements.txt"
    require(staging.is_dir() and not staging.is_symlink() and requirements.is_file()
            and not requirements.is_symlink() and requirements.stat().st_size <= 4096,
            "Private controller requirements are unavailable")
    lines = [line.strip() for line in requirements.read_text(encoding="utf-8").splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    require(sorted(lines) == sorted(REQUIREMENTS), "Private controller dependency pins require review")
    child_env = {key: env[key] for key in ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP") if key in env}
    child_env.update(environment(env))
    for args in ([str(root / "venv/bin/python"), "-m", "pip", "--isolated", "--disable-pip-version-check",
                  "install", "--no-input", "--index-url", "https://pypi.org/simple", "-r", str(requirements)],
                 [str(root / "venv/bin/python"), "-c", "import yaml, jsonschema"]):
        result = runner(args, env=child_env, capture_output=True, text=True, timeout=300, check=False)
        require(result.returncode == 0, "Private controller dependency setup failed")
    return True


def cleanup(env):
    root = tools_path(env)
    if root.exists():
        shutil.rmtree(root)
    return True


def main():
    def operation(_masker):
        if sys.argv[1:] == ["install"]:
            return install(os.environ)
        if sys.argv[1:] == ["requirements"]:
            return install_requirements(os.environ)
        if sys.argv[1:] == ["cleanup"]:
            return cleanup(os.environ)
        return False
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
