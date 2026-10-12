#!/usr/bin/env python3
"""Exercise Sandbox Manager with actual, isolated MongoDB credentials.

Requires mongod and mongosh. Starts a disposable authenticated local server;
does not connect to a developer or production database. Passwords are generated
in memory, passed via environment, and never printed or persisted in source.
"""

import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import time


def main():
    mongod = shutil.which("mongod")
    mongosh = shutil.which("mongosh")
    if not mongod or not mongosh:
        raise SystemExit("The authenticated Mongo contract requires mongod and mongosh")
    root = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix="cma-sm-mongo-") as temporary:
        path = Path(temporary)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        log = (path / "mongod.log").open("w")
        process = subprocess.Popen(
            [mongod, "--auth", "--bind_ip", "127.0.0.1", "--port", str(port),
             "--dbpath", temporary, "--quiet", "--wiredTigerCacheSizeGB", "0.25"],
            stdout=log, stderr=subprocess.STDOUT,
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Disposable MongoDB exited during startup")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Disposable MongoDB did not start") from None
                    time.sleep(0.2)
            environment = os.environ.copy()
            password = secrets.token_hex(24)
            environment["CMA_SM_BOOTSTRAP_PASSWORD"] = password
            bootstrap = subprocess.run(
                [mongosh, "--quiet", f"mongodb://127.0.0.1:{port}/admin", "--eval",
                 'db.createUser({user:"contract-admin",pwd:process.env.CMA_SM_BOOTSTRAP_PASSWORD,roles:["root"]})'],
                env=environment, capture_output=True, timeout=30,
            )
            if bootstrap.returncode:
                raise RuntimeError("Disposable MongoDB credential bootstrap failed")
            environment.pop("CMA_SM_BOOTSTRAP_PASSWORD")
            environment["CMA_SM_TEST_MONGODB_URI"] = (
                f"mongodb://contract-admin:{password}@127.0.0.1:{port}/admin?directConnection=true"
            )
            print("Running real MongoDB isolation and lifecycle recovery contracts", flush=True)
            result = subprocess.run(
                ["cargo", "test", "--locked", "-p", "cma-sandbox-manager", "--test", "mongo_isolation",
                 "--", "--ignored", "--test-threads=1"],
                cwd=root / "api", env=environment,
            )
            if result.returncode:
                raise SystemExit(result.returncode)
        finally:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            log.close()


if __name__ == "__main__":
    main()
