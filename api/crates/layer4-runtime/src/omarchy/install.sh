#!/bin/bash
# Build-time only: install signed packages and the pinned upstream desktop.
set -euo pipefail
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
pacman -Syu --noconfirm --needed \
    bash bubblewrap ca-certificates curl git python python-pillow python-numpy \
    ripgrep unzip xz zstd openssl dbus glibc libunwind icu krb5 gmp \
    hyprland quickshell foot chromium nautilus grim slurp wl-clipboard wtype \
    xorg-server-xvfb xorg-xdpyinfo xorg-xset xorg-xrandr xorg-xwininfo xdotool \
    x11vnc python-websockify scrot weston mesa libglvnd xorg-xwayland \
    qt6-imageformats qt6-multimedia qt6-svg qt6-wayland \
    noto-fonts noto-fonts-emoji ttf-jetbrains-mono-nerd-basic yaru-icon-theme \
    jq gum imagemagick inotify-tools libnotify procps-ng util-linux which \
    xdg-utils xdg-desktop-portal-hyprland python-gobject pipewire wireplumber
pacman -Q > /usr/share/cma-omarchy/packages.txt
find /var/cache/pacman/pkg -type f -name '*.pkg.tar.*' ! -name '*.sig' -exec sha256sum {} + > /usr/share/cma-omarchy/package-sha256.txt
Hyprland --version > /usr/share/cma-omarchy/hyprland-version.txt
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
chmod 755 /usr/local/bin/omarchy-launch-shell /usr/local/bin/uwsm-app
printf 'en_US.UTF-8 UTF-8\n' > /etc/locale.gen
locale-gen
rm -f /tmp/omarchy.tar.gz /tmp/omarchy-keyring.pkg.tar.zst
rm -rf /var/cache/pacman/pkg/*
