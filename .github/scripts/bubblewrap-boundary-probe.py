"""Root-owned diagnostic: prove setup containment using temporary fixtures only.

No model, home, configuration, system binary changes or external networking.
Every bwrap invocation creates user/PID/network namespaces and a minimal root.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import threading


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bwrap", type=Path, required=True)
    parser.add_argument("--bwrap-sha256", required=True)
    args = parser.parse_args()
    bwrap = args.bwrap.resolve(strict=True)
    assert digest(bwrap) == args.bwrap_sha256
    checks = []
    with tempfile.TemporaryDirectory(prefix="cma-bwrap-boundary-", dir="/tmp") as temporary:
        fixture = Path(temporary)
        inside, outside = fixture / "inside", fixture / "outside"
        inside.mkdir()
        outside.mkdir()
        (inside / "safe").mkdir()
        (outside / "sentinel").write_text("retained")
        (fixture / "payload").write_text("forbidden")
        base = [str(bwrap), "--die-with-parent", "--new-session",
                "--unshare-user", "--unshare-pid", "--unshare-net",
                "--ro-bind", "/usr", "/usr",
                "--symlink", "usr/bin", "/bin",
                "--symlink", "usr/lib", "/lib",
                "--symlink", "usr/lib64", "/lib64",
                "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
                "--bind", str(inside), "/inside"]

        def check_untouched():
            assert (outside / "sentinel").read_text() == "retained"
            assert sorted(path.name for path in outside.iterdir()) == ["sentinel"]

        def run(name, operations, expect_success, fds=()):
            process = subprocess.run(
                base + operations + ["--", "/usr/bin/true"],
                cwd=fixture, env={"PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C"},
                stdin=subprocess.DEVNULL, capture_output=True, timeout=8, pass_fds=fds,
            )
            check_untouched()
            stderr = process.stderr.decode(errors="replace")
            if process.returncode != 0:
                destination = operations[-1]
                expected_error = re.compile(
                    r"bwrap: Can't (?:mkdir parents for|create file) " + re.escape(destination)
                    + r": (?:No such file or directory|Not a directory|Too many levels of symbolic links)\n?"
                )
                assert process.returncode == 1 and expected_error.fullmatch(stderr), (name, process.returncode, stderr)
            if expect_success is not None and (process.returncode == 0) != expect_success:
                raise RuntimeError(json.dumps({"case": name, "exit": process.returncode,
                                              "stderr": process.stderr.decode(errors="replace")[:1024]}))
            checks.append({"case": name, "exit": process.returncode, "outside_untouched": True, "stderr": stderr})

        run("ordinary in-root creation", ["--dir", "/inside/ordinary"], True)
        assert (inside / "ordinary").is_dir()
        run("in-root absolute symlink", ["--symlink", "/inside/safe", "/valid",
                                          "--dir", "/valid/allowed"], True)
        assert (inside / "safe" / "allowed").is_dir()
        run("oldroot absolute escape rejected", ["--symlink", "/oldroot" + str(outside), "/escape",
                                                 "--dir", "/escape/created"], False)
        directory_fd = os.open(outside, os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
        file_fd = os.open(outside / "sentinel", os.O_PATH | os.O_CLOEXEC)
        data_fd = os.open(fixture / "payload", os.O_RDONLY | os.O_CLOEXEC)
        try:
            run("proc descriptor parent escape rejected",
                ["--symlink", f"/proc/self/fd/{directory_fd}", "/escape",
                 "--dir", "/escape/created"], False, (directory_fd,))
            run("proc descriptor file truncation rejected",
                ["--symlink", f"/proc/self/fd/{file_fd}", "/escape",
                 "--file", str(data_fd), "/escape"], False, (file_fd, data_fd))
        finally:
            for fd in (directory_fd, file_fd, data_fd):
                os.close(fd)
        stop = threading.Event()
        failures = []
        swaps = [0]
        race_ready = threading.Event()

        def replace_parent():
            iteration = 0
            try:
                while not stop.is_set():
                    target = "safe" if iteration % 2 else "/oldroot" + str(outside)
                    os.symlink(target, inside / "next")
                    os.replace(inside / "next", inside / "race")
                    iteration += 1
                    swaps[0] = iteration
                    if iteration >= 2:
                        race_ready.set()
            except BaseException as error:
                failures.append(type(error).__name__)

        racer = threading.Thread(target=replace_parent)
        racer.start()
        try:
            assert race_ready.wait(timeout=2), "replacement thread did not initialize target"
            start_swaps = swaps[0]
            for iteration in range(8):
                run(f"concurrent parent replacement {iteration + 1}",
                    ["--dir", "/inside/race/created"], None)
        finally:
            stop.set()
            racer.join(timeout=2)
        assert not racer.is_alive() and not failures
        assert swaps[0] > start_swaps, "no concurrent replacements occurred"
        checks.append({"case": "concurrent replacements observed", "replacements": swaps[0] - start_swaps})
        check_untouched()
    print(json.dumps({"scope": "temporary-bubblewrap-setup-containment-no-model",
                      "bubblewrap_sha256": args.bwrap_sha256, "checks": checks,
                      "temporary_fixtures_removed": not fixture.exists(),
                      "provider_or_model_requests": 0}))


if __name__ == "__main__":
    main()
