"""Probe pinned Codex's native sandbox directly, without Agent/model/home IO.

Run only in an explicitly selected disposable or owned diagnostic runtime.
The two commands touch temporary fixtures and assert filesystem/network denial.
The caller supplies the reviewed bubblewrap executable and its exact digest.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile

NATIVE = {
    "x86_64": ("x64", "x86_64", "cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40"),
    "aarch64": ("arm64", "aarch64", "e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2"),
}

CHILD = r'''
import ctypes, errno, json, os, socket, sys
from pathlib import Path
fixture = Path(sys.argv[1])
mode, parent_net = sys.argv[2:4]
net = os.readlink('/proc/self/ns/net')
assert net != parent_net, 'network namespace was shared'
interfaces = [line.split(':')[0].strip() for line in Path('/proc/net/dev').read_text().splitlines()[2:]]
assert interfaces == ['lo'], 'isolated namespace has an external interface'
status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
# gVisor omits NoNewPrivs from /proc even when prctl exposes the state.
libc = ctypes.CDLL(None, use_errno=True)
assert libc.prctl(39, 0, 0, 0, 0) == 1, 'PR_GET_NO_NEW_PRIVS was not set'
if 'NoNewPrivs' in status:
    assert status['NoNewPrivs'].strip() == '1'
for family in (socket.AF_INET, socket.AF_INET6, socket.AF_NETLINK):
    try:
        candidate = socket.socket(family, socket.SOCK_STREAM if family != socket.AF_NETLINK else socket.SOCK_RAW)
    except OSError as error:
        assert error.errno == errno.EPERM, (family, error.errno)
    else:
        candidate.close()
        raise AssertionError('restricted-network socket creation was allowed')
def denied(path):
    try:
        with path.open('a') as stream:
            stream.write('forbidden')
    except OSError as error:
        assert error.errno == errno.EROFS, error.errno
    else:
        raise AssertionError('read-only filesystem write was allowed')
denied(fixture / 'outside')
denied(fixture / 'workspace' / '.git' / 'protected')
target = fixture / 'workspace' / 'ordinary'
if mode == 'read-only':
    denied(target)
else:
    target.write_text('allowed')
print(json.dumps({'mode': mode, 'filesystem': 'pass', 'network': 'pass', 'no_new_privs': True}))
'''


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while data := stream.read(1024 * 1024):
            digest.update(data)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bwrap", type=Path, required=True)
    parser.add_argument("--bwrap-sha256", required=True)
    args = parser.parse_args()
    bwrap = args.bwrap.resolve(strict=True)
    assert sha256(bwrap) == args.bwrap_sha256, "bubblewrap artifact mismatch"
    package_arch, target_arch, expected_native = NATIVE[platform.machine()]
    native = Path(
        "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/"
        f"codex-linux-{package_arch}/vendor/{target_arch}-unknown-linux-musl/bin/codex"
    )
    assert sha256(native) == expected_native, "pinned Codex artifact mismatch"
    results = []
    parent_net = os.readlink("/proc/self/ns/net")
    with tempfile.TemporaryDirectory(prefix="cma-bwrap-native-", dir="/tmp") as scratch:
        fixture = Path(scratch)
        commands = fixture / "commands"
        commands.mkdir()
        (commands / "bwrap").symlink_to(bwrap)
        workspace = fixture / "workspace"
        workspace.mkdir()
        (workspace / ".git").mkdir()
        (fixture / "outside").write_text("retained")
        (workspace / "ordinary").write_text("retained")
        (workspace / ".git" / "protected").write_text("retained")
        for mode in ("read-only", "workspace-write"):
            entries = [{"path": {"type": "special", "value": {"kind": "root"}}, "access": "read"}]
            if mode == "workspace-write":
                entries.append({"path": {"type": "path", "path": str(workspace)}, "access": "write"})
            permissions = {
                "type": "managed", "network": "restricted",
                "file_system": {"type": "restricted", "entries": entries},
            }
            # argv[0] dispatch enters linux-sandbox before dotenv, configuration,
            # provider startup or Codex's home/alias setup (pinned arg0/src/lib.rs).
            process = subprocess.run(
                ["codex-linux-sandbox", "--sandbox-policy-cwd", str(workspace),
                 "--permission-profile", json.dumps(permissions), "--",
                 "/usr/bin/python3", "-I", "-B", "-c", CHILD,
                 str(fixture), mode, parent_net],
                executable=str(native), cwd=workspace,
                env={"PATH": f"{commands}:/usr/bin:/bin", "LANG": "C", "LC_ALL": "C"},
                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                timeout=25, check=False,
            )
            if process.returncode != 0:
                raise RuntimeError(json.dumps({"mode": mode, "exit": process.returncode,
                                              "stderr": process.stderr[:4096]}))
            results.append(json.loads(process.stdout))
            assert (fixture / "outside").read_text() == "retained"
            assert (workspace / ".git" / "protected").read_text() == "retained"
            assert (workspace / "ordinary").read_text() == ("allowed" if mode == "workspace-write" else "retained")
    print(json.dumps({"scope": "native-sandbox-temporary-fixtures-no-model",
                      "native_sha256": expected_native, "bubblewrap_sha256": args.bwrap_sha256,
                      "checks": results, "temporary_fixtures_removed": not fixture.exists()}))


if __name__ == "__main__":
    main()
