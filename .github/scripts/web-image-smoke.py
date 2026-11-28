#!/usr/bin/env python3
"""Mount a stale retained-assets volume and browser-test the exact web image."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def run_browser(command, *, cwd, env, evidence, timeout, grace=30,
                ready_file=None, startup_timeout=None):
    """Use a dedicated subreaper so unrelated children of this caller stay unowned."""
    evidence = Path(evidence).resolve()
    result_path = evidence / "browser-process.json"
    result_path.unlink(missing_ok=True)  # Never interpret evidence from an earlier run.
    startup = {}
    if ready_file is not None:
        ready_file = Path(ready_file).resolve()
        if startup_timeout is None or not math.isfinite(startup_timeout) or startup_timeout <= 0:
            raise ValueError("readiness requires a finite positive startup timeout")
        ready_file.unlink(missing_ok=True)
        ready_file.with_suffix(".armed.json").unlink(missing_ok=True)
        startup = {"ready_file": str(ready_file), "startup_timeout": startup_timeout}
    elif startup_timeout is not None:
        raise ValueError("startup timeout requires a readiness file")
    helper = Path(__file__).with_name("web_image_supervisor.py")
    result = subprocess.run([sys.executable, str(helper), str(evidence), str(timeout),
                             str(grace), json.dumps(startup), *command], cwd=cwd, env=env)
    if result.returncode:
        if result_path.exists():
            recorded = json.loads(result_path.read_text())
            if recorded["timed_out"]:
                raise subprocess.TimeoutExpired(command, recorded["startup_timeout_seconds"]
                    if recorded["timeout_phase"] == "startup" else timeout)
            if recorded["returncode"]:
                raise subprocess.CalledProcessError(recorded["returncode"], command)
        raise subprocess.CalledProcessError(result.returncode, command)



def verify_retired_routes(origin):
    for path in ("/api/image-manager", "/api/image-manager/", "/api/image-manager/v1/images",
                 "/image-manager/openapi.json"):
        for method in ("GET", "HEAD"):
            request = urllib.request.Request(origin + path, method=method)
            try:
                with urllib.request.urlopen(request, timeout=2):
                    raise AssertionError(f"{method} {path} was not retired")
            except urllib.error.HTTPError as response:
                with response:
                    if (response.code != 404
                            or response.headers.get_content_type() != "application/problem+json"
                            or response.headers.get("Cache-Control") != "no-store"):
                        raise AssertionError(f"{method} {path} returned an invalid retirement response")
                    body = response.read()
                    if method == "HEAD":
                        if body:
                            raise AssertionError(f"HEAD {path} returned a body")
                    elif json.loads(body).get("code") != "not_found":
                        raise AssertionError(f"GET {path} returned an invalid problem type")


def verify_image(reference):
    root = Path(__file__).resolve().parents[2]
    image_id = output("docker", "image", "inspect", reference, "--format", "{{.Id}}")
    labels = json.loads(output("docker", "image", "inspect", image_id,
                               "--format", "{{json .Config.Labels}}")) or {}
    revision = labels.get("org.opencontainers.image.revision", "")
    version = labels.get("org.opencontainers.image.version", "")
    subprocess.run([sys.executable, str(root / ".github/scripts/web-build-identity.py"),
                    image_id, revision, version], check=True)
    with tempfile.TemporaryDirectory(prefix="cma-web-image-") as temporary:
        retained = Path(temporary) / "assets"
        retained.mkdir(mode=0o755)
        # A prior release exists, but contains none of this image's entry assets.
        (retained / "retained-smoke.js").write_text("/* retained release */\n")
        container = output(
            "docker", "run", "-d", "--read-only", "--tmpfs", "/tmp:rw,mode=1777",
            "-p", "127.0.0.1::8080",
            "--add-host", "cma-agent-manager:127.0.0.1",
            "--add-host", "cma-event-gateway:127.0.0.1",
            "--mount", f"type=bind,source={retained},target=/usr/share/nginx/html/assets,readonly",
            image_id,
        )
        try:
            address = output("docker", "port", container, "8080/tcp")
            url = f"http://{address}"
            for attempt in range(50):
                try:
                    with urllib.request.urlopen(url, timeout=2) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(0.2)
            else:
                raise RuntimeError("Web image did not start")
            verify_retired_routes(url)
            evidence = Path(os.environ.get("CMA_WEB_IMAGE_EVIDENCE_DIR")
                            or tempfile.mkdtemp(prefix="cma-web-image-evidence-"))
            # Passing CI suites already consume 176s before CLI overhead. Allow
            # 240s per engine, then SIGINT the actual runner with 30s for artifacts.
            run_browser(
                ["node", "--require", str(Path(__file__).with_name("web-image-phases.cjs").resolve()),
                 "node_modules/@playwright/test/cli.js", "test", "--config", "e2e/image-smoke.config.ts"],
                cwd=root / "web", env={**os.environ, "CMA_WEB_IMAGE_URL": url,
                                      "CMA_WEB_IMAGE_EVIDENCE_DIR": str(evidence.resolve()),
                                      "CMA_EXPECTED_WEB_REVISION": revision,
                                      "CMA_EXPECTED_WEB_VERSION": version},
                evidence=evidence,
                timeout=240 * min(3, len(os.environ.get("CMA_E2E_BROWSERS", "chromium").split(","))),
            )
            print(json.dumps({"image": image_id, "revision": revision, "version": version,
                              "browsers": os.environ.get("CMA_E2E_BROWSERS", "chromium"),
                              "stale_asset_mount": True, "passed": True}))
        finally:
            subprocess.run(["docker", "rm", "-f", container], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    verify_image(parser.parse_args().image)
