"""Check two real read-only runtime boots without exposing machine identities."""
import contextlib
import hashlib
import hmac
import json
import os
import subprocess
import sys
import uuid

# Both sandboxes start together. Wait inside the offline sandbox until its own
# supervisor advertises readiness and compare the paths at that point.
PROBE = """
import hmac, os, pathlib, re, stat, sys, time
from urllib.request import urlopen
from urllib.error import URLError
deadline = time.monotonic() + 45
while time.monotonic() < deadline:
    try:
        if sys.argv[1] == 'pool':
            if stat.S_ISSOCK(os.stat('/var/lib/cma-runtime/assignment.sock').st_mode):
                break
        else:
            with urlopen('http://127.0.0.1:9090/ready', timeout=1) as response:
                if response.status == 200:
                    break
    except (OSError, URLError):
        pass
    time.sleep(0.1)
else:
    sys.exit(1)
value = pathlib.Path('/etc/machine-id').read_bytes()
assert re.fullmatch(rb'[0-9a-f]{32}\\n', value) and value != b'0' * 32 + b'\\n'
assert pathlib.Path('/var/lib/dbus/machine-id').read_bytes() == value
assert os.statvfs('/').f_flag & os.ST_RDONLY and os.geteuid() == 10001
assert pathlib.Path('/proc/1/comm').read_text().strip() == 'layer4-runtime'
# Only the parent sees this run-specific equality token. It is never logged.
print(hmac.digest(sys.stdin.buffer.read(), value, 'sha256').hex())
"""


def docker(*args, data=None):
    result = subprocess.run(["docker", *args], input=data, capture_output=True, timeout=70)
    if result.returncode:
        raise RuntimeError("Sandbox fixture failed at " + args[0])
    return result.stdout.strip()


def verify(image):
    if not image or image.startswith("-"):
        raise ValueError("Invalid fixture image")
    selected = docker("image", "inspect", "--format", "{{.Id}}", image).decode()
    comparison_key = os.urandom(32)
    tokens = []
    with contextlib.ExitStack() as cleanup:
        containers = []
        for mode in ("normal", "pool"):
            name = "cma-machine-id-fixture-" + uuid.uuid4().hex
            command = ["create", "--name", name, "--pull=never", "--platform", "linux/amd64",
                       "--network=none", "--ipc=private", "--read-only", "--cap-drop=ALL",
                       "--security-opt=no-new-privileges", "--user=10001:10001",
                       "--pids-limit=128", "--memory=512m",
                       "--tmpfs=/tmp:rw,nosuid,nodev,mode=1777",
                       "--tmpfs=/var/lib/cma-runtime:rw,nosuid,nodev,uid=10001,gid=10001,mode=700",
                       "--tmpfs=/home/cma:rw,nosuid,nodev,uid=10001,gid=10001,mode=700"]
            if mode == "pool":
                command += ["--env", "CHRONO_RUNTIME_POD_UID=" + str(uuid.uuid4()), selected, "--wait-for-assignment"]
            else:
                command += ["--env", "CHRONO_SANDBOX_ID=" + name,
                            "--env", "CHRONO_PLATFORM_BASE_URL=http://127.0.0.1:9192", selected]
            docker(*command)
            cleanup.callback(docker, "rm", "--force", "--volumes", name)
            docker("start", name)
            containers.append((name, mode))
        for name, mode in containers:
            token = docker("exec", "-i", name, "/usr/bin/python3", "-I", "-B", "-c", PROBE, mode,
                           data=comparison_key)
            if len(token) != hashlib.sha256().digest_size * 2:
                raise ValueError("Invalid comparison result")
            tokens.append(bytes.fromhex(token.decode("ascii")))
        if hmac.compare_digest(*tokens):
            raise ValueError("Machine identity was reused")
    return {"two_supervisors_ready": True, "roots_read_only": True, "unprivileged": True,
            "both_ids_valid": True, "dbus_paths_agree": True, "sandbox_ids_distinct": True,
            "containers_removed": True}


if __name__ == "__main__":
    try:
        print(json.dumps(verify(sys.argv[1])))
    except RuntimeError as error:
        sys.exit(str(error))
    except (IndexError, OSError, subprocess.TimeoutExpired, ValueError):
        sys.exit("Read-only runtime machine identity check failed")
