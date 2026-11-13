#!/usr/bin/env python3
"""Mount a stale retained-assets volume and browser-test the exact web image."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from web_image_processes import OwnedProcesses, child_subreaper


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def run_browser(command, *, cwd, env, evidence, timeout, grace=30):
    """Interrupt the runner first so Playwright can retain its trace and screenshot."""
    with child_subreaper():
        return supervise_browser(command, cwd=cwd, env=env, evidence=evidence,
                                 timeout=timeout, grace=grace)


def supervise_browser(command, *, cwd, env, evidence, timeout, grace):
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=cwd, env=env, start_new_session=True)
    owned = OwnedProcesses(process.pid)
    timed_out = False
    forced = False

    def record():
        evidence.mkdir(parents=True, exist_ok=True)
        (evidence / "browser-process.json").write_text(json.dumps({
            "timeout_seconds": timeout, "interrupt_grace_seconds": grace,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "timed_out": timed_out, "forced_kill": forced,
            "returncode": process.returncode,
            "process_cleanup": owned.evidence(),
        }, indent=2) + "\n")

    try:
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            # Capture detached Chrome groups BEFORE SIGINT can reparent them.
            owned.capture()
            # Save evidence before signalling, even if the runner cannot respond.
            record()
            print(f"Web browser watchdog expired after {timeout}s; "
                  f"interrupting Playwright with {grace}s to retain evidence", flush=True)
            process.send_signal(signal.SIGINT)
            deadline = time.monotonic() + grace
            while process.poll() is None:
                owned.capture()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    forced = True
                    print("Playwright did not stop; terminating its owned descendants", flush=True)
                    break
                try:
                    process.wait(timeout=min(.1, remaining))
                except subprocess.TimeoutExpired:
                    continue  # Keep tracking children throughout the bounded grace.
            # A timeout remains a failure even if an interrupted runner exits zero.
            raise subprocess.TimeoutExpired(command, timeout)
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, command)
    finally:
        try:
            owned.terminate()
        finally:
            process.kill()
            process.wait()
            try:
                owned.reap()
            finally:
                record()


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
            "--add-host", "cma-image-manager:127.0.0.1",
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
            evidence = Path(os.environ.get("CMA_WEB_IMAGE_EVIDENCE_DIR")
                            or tempfile.mkdtemp(prefix="cma-web-image-evidence-"))
            # Passing CI suites already consume 176s before CLI overhead. Allow
            # 240s per engine, then SIGINT the actual runner with 30s for artifacts.
            run_browser(
                ["node", "node_modules/@playwright/test/cli.js", "test", "--config", "e2e/image-smoke.config.ts"],
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
