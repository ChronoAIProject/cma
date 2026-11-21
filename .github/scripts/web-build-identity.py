#!/usr/bin/env python3
"""Verify compiled web identity in exact image bytes without starting the image."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify(directory, labels, revision, version):
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "full source revision required")
    require(re.fullmatch(r"v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version)
            or version == f"git-{revision[:12]}", "immutable version required")
    identity = json.loads((directory / "build-info.json").read_text())
    require(identity == {"version": version, "revision": revision, "local": False},
            "compiled web identity differs from expected source/version")
    for key, value in {
        "source": "https://github.com/ChronoAIProject/cma",
        "revision": revision,
        "version": version,
    }.items():
        require(labels.get(f"org.opencontainers.image.{key}") == value,
                f"web OCI {key} differs")
    # Vite may split the UI across chunks, but the compiled identity must exist
    # in one emitted module, not only in unconsumed verification metadata.
    require(any(version in path.read_text() and revision in path.read_text()
                for path in (directory / "assets").glob("*.js")),
            "identity missing from compiled JavaScript")


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def verify_image(reference, revision, version):
    inspected = json.loads(output("docker", "image", "inspect", reference))
    require(len(inspected) == 1, "one web image required")
    image = inspected[0]
    image_id = image["Id"]
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", image_id), "immutable image ID required")
    container = output("docker", "create", image_id)
    try:
        with tempfile.TemporaryDirectory(prefix="cma-web-identity-") as temporary:
            directory = Path(temporary) / "html"
            subprocess.run(["docker", "cp", f"{container}:/usr/share/nginx/html", str(directory)], check=True)
            verify(directory, image.get("Config", {}).get("Labels") or {}, revision, version)
    finally:
        subprocess.run(["docker", "rm", container], check=True, stdout=subprocess.DEVNULL)
    print(f"Web bundle and OCI identity verified: {version} ({revision}), {image_id}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("revision")
    parser.add_argument("version")
    args = parser.parse_args()
    verify_image(args.image, args.revision, args.version)
