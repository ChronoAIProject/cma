#!/usr/bin/env python3
"""Exercise Image Manager with actual, isolated MongoDB credentials.

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
    with tempfile.TemporaryDirectory(prefix="cma-im-mongo-") as temporary:
        path = Path(temporary)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        replica_key = path / "replica-key"
        replica_key.write_text(secrets.token_hex(64))
        replica_key.chmod(0o600)
        log = (path / "mongod.log").open("w")
        process = subprocess.Popen(
            [mongod, "--setParameter", "enableTestCommands=1", "--auth", "--replSet", "cma-im-contract", "--keyFile", str(replica_key), "--bind_ip", "127.0.0.1", "--port", str(port),
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
            initialize = subprocess.run([mongosh, "--quiet", f"mongodb://127.0.0.1:{port}/admin?directConnection=true", "--eval",
                f'rs.initiate({{_id:"cma-im-contract",members:[{{_id:0,host:"127.0.0.1:{port}"}}]}}); while (!db.hello().isWritablePrimary) sleep(200)'],capture_output=True,timeout=30)
            if initialize.returncode:
                raise RuntimeError("Disposable replica-set initialization failed")
            environment = os.environ.copy()
            password = secrets.token_hex(24)
            environment["CMA_IM_BOOTSTRAP_PASSWORD"] = password
            bootstrap = subprocess.run(
                [mongosh, "--quiet", f"mongodb://127.0.0.1:{port}/admin", "--eval",
                 'db.createUser({user:"contract-admin",pwd:process.env.CMA_IM_BOOTSTRAP_PASSWORD,roles:["root"]})'],
                env=environment, capture_output=True, timeout=30,
            )
            if bootstrap.returncode:
                raise RuntimeError("Disposable MongoDB credential bootstrap failed")
            environment.pop("CMA_IM_BOOTSTRAP_PASSWORD")
            environment["CMA_IM_TEST_MONGODB_URI"] = (
                f"mongodb://contract-admin:{password}@127.0.0.1:{port}/admin?directConnection=true"
            )
            subprocess.run(["cargo", "build", "--locked", "-p", "cmaeg"], cwd=root / "api", env=environment, check=True)
            print("Running real MongoDB isolation and lifecycle recovery contracts", flush=True)
            result = subprocess.run(
                ["cargo", "test", "--locked", "-p", "cma-image-manager", "--test", "mongo_isolation", "--test", "browser_session",
                 "--", "--ignored", "--test-threads=1"],
                cwd=root / "api", env=environment,
            )
            if result.returncode:
                raise SystemExit(result.returncode)
            for command in [
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "console::session_backend::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "console", "shared_", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "optional_forms_storage", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "optional_media_continuation_storage", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "transaction_terminal::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "nextest", "run", "--workspace", "--locked", "-E",
                 "package(=cmaeg) and kind(lib) and test(~activity::history::mongo_tests::)",
                 "--run-ignored", "only", "--test-threads", "1", "--no-fail-fast"],
                ["cargo", "nextest", "run", "--workspace", "--locked", "-E",
                 "package(=cmaeg) and test(=worker_credentials::mongo::integration_tests::worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier)",
                 "--run-ignored", "only", "--test-threads", "1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "whatsapp_retention_on_real_mongo", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cma", "gateway::tests::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "relay_ingress", "runner_binary::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "replica_admission_mongo", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "management", "binary::compiled_binary_", "--", "--include-ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "management", "binary_independent::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "management", "record_management::mongo_legacy_read_is_owned_and_read_only", "--", "--ignored", "--exact", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "mongo_isolation", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "management", "github_integrations::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "management", "organization_channels::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--test", "github_source", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cmaeg", "--lib", "tools::threads::ledger::mongo_tests::", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cma", "store::isolation::tests::real_", "--", "--ignored", "--test-threads=1"],
                ["cargo", "test", "--locked", "-p", "cma", "store::isolation::shared_schema_contract::", "--", "--ignored", "--test-threads=1"],
            ]:
                result = subprocess.run(command, cwd=root / "api", env=environment)
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
