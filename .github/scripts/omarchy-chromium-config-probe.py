"""Check that Chromium refresh works without installing shared sign-in flags."""

from pathlib import Path
import subprocess
import sys
import tempfile


def verify(desktop):
    helper = "omarchy-install-chromium-google-account"
    executable = desktop / "bin" / helper
    assert not (executable.exists() or executable.is_symlink()), "unexpected sign-in helper"
    menu = (desktop / "default/omarchy/omarchy-menu.jsonc").read_text()
    refresh = desktop / "bin/omarchy-refresh-chromium"
    assert helper not in menu and "install.service.chromium-account" not in menu
    assert helper not in refresh.read_text()
    defaults = (desktop / "config/chromium-flags.conf").read_bytes()
    options = (b"--oauth2-client-id", b"--oauth2-client-secret")
    assert defaults and all(option not in defaults for option in options)

    # Exercise both a fresh profile and the old refresh branch without credentials.
    for existing in (False, True):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / ".config"
            profile = config / "chromium"
            profile.mkdir(parents=True)
            flags = config / "chromium-flags.conf"
            env = {"HOME": directory, "PATH": f"{desktop}/bin:/usr/bin:/bin", "OMARCHY_PATH": str(desktop)}
            if existing:
                flags.write_bytes(b"\n".join(options) + b"\n")
            subprocess.run([str(refresh)], env=env, check=True, timeout=30,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            assert flags.read_bytes() == defaults, "Chromium refresh changed ordinary flags"
            for host in ("com.omarchy.copy_url", "com.omarchy.ytdlp"):
                assert (profile / "NativeMessagingHosts" / f"{host}.json").is_file()


if __name__ == "__main__":
    verify(Path(sys.argv[1]) if len(sys.argv) == 2 else Path("/usr/share/omarchy"))
