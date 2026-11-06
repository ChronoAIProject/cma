#!/usr/bin/env python3
"""Mount a stale retained-assets volume and browser-test the exact web image."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def verify_image(reference):
    root = Path(__file__).resolve().parents[2]
    image_id = output("docker", "image", "inspect", reference, "--format", "{{.Id}}")
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
            subprocess.run(
                ["npx", "--no-install", "playwright", "test", "--config", "e2e/image-smoke.config.ts"],
                cwd=root / "web", env={**os.environ, "CMA_WEB_IMAGE_URL": url},
                check=True, timeout=180,
            )
            print(json.dumps({"image": image_id, "stale_asset_mount": True, "passed": True}))
        finally:
            subprocess.run(["docker", "rm", "-f", container], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    verify_image(parser.parse_args().image)
