#!/bin/bash
# Build-time only: install signed packages and the pinned upstream desktop.
set -euo pipefail
ulimit -c 0
pacman-key --init
pacman-key --populate archlinux
curl -fsSL https://pkgs.omarchy.org/stable/x86_64/omarchy-keyring-20251027-1-any.pkg.tar.zst -o /tmp/omarchy-keyring.pkg.tar.zst
echo 'f4bc258879f493462736f9a072c4f4c7f753eb559f6b346f5bdf8ed816f75f66  /tmp/omarchy-keyring.pkg.tar.zst' | sha256sum -c -
pacman -U --noconfirm /tmp/omarchy-keyring.pkg.tar.zst
pacman-key --populate omarchy
curl -fsSL https://api.github.com/repos/omacom/omarchy/tarball/237f1b4071a0bff2296c6d740ba2b9fd695ebfa0 -o /tmp/omarchy.tar.gz
echo '01fa4191939a61a804b6169b84c4fe3e29eab0d8ee973ff078172d85959b3647  /tmp/omarchy.tar.gz' | sha256sum -c -
mkdir -p /usr/share/omarchy /usr/share/cma-omarchy
tar -xzf /tmp/omarchy.tar.gz -C /usr/share/omarchy --strip-components=1
cp /usr/share/omarchy/default/pacman/pacman-stable.conf /etc/pacman.conf
cp /usr/share/omarchy/default/pacman/mirrorlist-stable /etc/pacman.d/mirrorlist
# Docker build already isolates this step; its seccomp profile denies Landlock.
# This flag affects only the build-time downloader, not package signatures.
pacman -Syu --disable-sandbox --noconfirm --needed \
    bash bubblewrap ca-certificates curl git python python-pillow python-numpy \
    ripgrep unzip xz zstd openssl dbus glibc libunwind icu krb5 gmp \
    hyprland quickshell foot chromium nautilus grim slurp wl-clipboard wtype \
    xorg-server-xvfb xorg-xdpyinfo xorg-xset xorg-xrandr xorg-xwininfo xorg-xprop xdotool \
    wayvnc tigervnc wlr-randr python-pip scrot labwc mesa libglvnd xorg-xwayland \
    qt6-imageformats qt6-multimedia qt6-svg qt6-wayland \
    noto-fonts noto-fonts-emoji ttf-jetbrains-mono-nerd-basic yaru-icon-theme \
    jq gum imagemagick inotify-tools libnotify procps-ng util-linux which \
    xdg-utils xdg-terminal-exec xdg-desktop-portal-hyprland python-gobject pipewire wireplumber
curl -fsSL https://files.pythonhosted.org/packages/19/e8/6476cd85a71f7cfb04ca1e199809b75bc8580d075d4338ffcde4c761d620/websockify-0.13.0-py3-none-any.whl -o /tmp/websockify-0.13.0-py3-none-any.whl
echo 'ac497a8dafc7f51d28ca989f01cdf5f8add663a497d425a56b159dcb8d6a314c  /tmp/websockify-0.13.0-py3-none-any.whl' | sha256sum -c -
python -m venv --without-pip --system-site-packages /opt/cma-stream
python -m pip --python /opt/cma-stream/bin/python install --no-deps --no-index /tmp/websockify-0.13.0-py3-none-any.whl
ln -s /opt/cma-stream/bin/websockify /usr/local/bin/websockify
rm /tmp/websockify-0.13.0-py3-none-any.whl
# pip is used only to install this pinned wheel; it is not a desktop dependency.
pacman -Rns --noconfirm python-pip
/opt/cma-stream/bin/python -c 'from importlib.metadata import version; assert version("websockify") == "0.13.0"'
websockify --help >/dev/null
bash /tmp/secure-omarchy-tools.sh
pacman -Q > /usr/share/cma-omarchy/packages.txt
find /var/cache/pacman/pkg -type f -name '*.pkg.tar.*' ! -name '*.sig' -exec sha256sum {} + > /usr/share/cma-omarchy/package-sha256.txt
pacman -Q hyprland aquamarine > /usr/share/cma-omarchy/hyprland-version.txt
# This is a user session supervised by CMA; no system login manager is started.
cat > /usr/local/bin/omarchy-launch-shell <<'EOF'
#!/bin/bash
exec env QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 quickshell -n -p /usr/share/omarchy/shell
EOF
cat > /usr/local/bin/uwsm-app <<'EOF'
#!/bin/bash
[[ ${1:-} != -- ]] || shift
exec "$@"
EOF
cat > /usr/local/bin/omarchy-launch-browser <<'EOF'
#!/bin/bash
set -euo pipefail
args=()
for argument in "$@"; do
  [[ $argument != --private ]] || argument=--incognito
  args+=("$argument")
done
exec /usr/local/bin/cma-desktop-session --browser "${args[@]}"
EOF
chmod 755 /usr/local/bin/omarchy-launch-shell /usr/local/bin/uwsm-app /usr/local/bin/omarchy-launch-browser
cp /usr/share/omarchy/applications/omarchy-browser.desktop /usr/share/applications/
printf 'en_US.UTF-8 UTF-8\n' > /etc/locale.gen
locale-gen
# Preserve the caller's exact managed PATH through distro profile fragments.
# The owner's own .profile still runs afterwards and can intentionally change it.
python - <<'PY'
from pathlib import Path
p = Path('/etc/profile')
original = p.read_text()
prefix = 'cma_profile_path="${PATH:-/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games}"\n'
suffix = '\nPATH="$cma_profile_path"\nexport PATH\nunset cma_profile_path\n'
p.write_text(prefix + original + suffix)
PY
rm -f /tmp/omarchy.tar.gz /tmp/omarchy-keyring.pkg.tar.zst
rm -rf /var/cache/pacman/pkg/*
