#!/bin/bash
# Build-time only: install signed packages and the pinned upstream desktop.
set -euo pipefail
ulimit -c 0
# Stable build metadata; package security/signature verification stays active.
export SOURCE_DATE_EPOCH=0
snapshot=$1
[[ $snapshot =~ ^[0-9]{4}/[0-9]{2}/[0-9]{2}$ ]]
# Every Arch and Omarchy package comes from the digest-pinned package bundle,
# mounted read-only. No package server is contacted.
bundle=/run/cma-omarchy-packages
test "$(cat "$bundle/SNAPSHOT")" = "$snapshot"
pacman-key --init
pacman-key --populate archlinux
cp "$bundle/omarchy/x86_64/omarchy-keyring-20251027-1-any.pkg.tar.zst" /tmp/omarchy-keyring.pkg.tar.zst
echo 'f4bc258879f493462736f9a072c4f4c7f753eb559f6b346f5bdf8ed816f75f66  /tmp/omarchy-keyring.pkg.tar.zst' | sha256sum -c -
# libalpm omits INSTALLDATE at epoch zero. Use a fixed nonzero package date;
# normalization below still writes zero after validating every installed record.
SOURCE_DATE_EPOCH=1 pacman -U --noconfirm /tmp/omarchy-keyring.pkg.tar.zst
pacman-key --populate omarchy
curl -fsSL https://api.github.com/repos/omacom/omarchy/tarball/237f1b4071a0bff2296c6d740ba2b9fd695ebfa0 -o /tmp/omarchy.tar.gz
echo '01fa4191939a61a804b6169b84c4fe3e29eab0d8ee973ff078172d85959b3647  /tmp/omarchy.tar.gz' | sha256sum -c -
mkdir -p /usr/share/omarchy /usr/share/cma-omarchy
# The upstream account helper embeds a shared OAuth client. Keep its executable
# out of the installed tree and remove its menu and refresh hooks below.
tar -xzf /tmp/omarchy.tar.gz -C /usr/share/omarchy --strip-components=1 \
    --exclude='*/bin/omarchy-install-chromium-google-account'
# Install from Omarchy's repository configuration with every package taken
# from the bundle. core, extra and multilib are the bundle's copies of the
# dated Arch Linux Archive, including the indexed aml that the archive itself
# serves with older bytes. Omarchy's own repository has no dated archive, so a
# local copy holds exactly the bundle's packages this desktop takes from it,
# each checked by sha256 beside its detached signature. pacman still verifies
# every signature against the keyrings.
omarchy_packages=/tmp/cma-omarchy-packages
mkdir -m 0755 "$omarchy_packages"
while read -r file package_sha256 signature_sha256; do
    cp "$bundle/omarchy/x86_64/$file" "$bundle/omarchy/x86_64/$file.sig" "$omarchy_packages/"
    printf '%s  %s\n' "$package_sha256" "$omarchy_packages/$file" \
        "$signature_sha256" "$omarchy_packages/$file.sig" | sha256sum -c -
done <<'PACKAGES'
quickshell-0.3.2.r7.g2b4501a-1-x86_64.pkg.tar.zst f9f813213597d1ee25e16860ed326e810857810460c09ee87d06bc3b655c2da1 1fba2e10aa1c48209588a5dcd875cb83b91bc27b938d16ccd5a6a7700833b936
ttf-jetbrains-mono-nerd-basic-3.5.1-2-any.pkg.tar.zst c574e6266139fd24f668301d39d9faab386d7041ad3952b3012a2480e09cff0c 525d05434bdb3db29e60323b4398ba605d43020c626d7382095d093d48f383e8
yaru-icon-theme-26.10.3-3-any.pkg.tar.zst 3efc53db41713dd7a660a6ecb0ff5fc130ff9b88feb1251b7bcc35aa54288b0a 934fc4fe04993bdf458f70b40a765b0bd246c93783e455ec585d41a24eced5a5
PACKAGES
repo-add -q "$omarchy_packages/omarchy.db.tar.gz" "$omarchy_packages"/*.pkg.tar.zst
sed "s|^Server = https://pkgs.omarchy.org/stable/\$arch\$|Server = file://$omarchy_packages|" \
    /usr/share/omarchy/default/pacman/pacman-stable.conf > /etc/pacman.conf
printf 'Server = file://%s/$repo/os/$arch\n' "$bundle" > /etc/pacman.d/mirrorlist
test "$(pacman-conf --repo-list | paste -sd ' ')" = 'omarchy core extra multilib'
test "$(pacman-conf --repo=omarchy Server)" = "file://$omarchy_packages"
for repository in core extra multilib; do
    test "$(pacman-conf --repo="$repository" Server)" = "file://$bundle/$repository/os/x86_64"
done
# Docker build already isolates this step; its seccomp profile denies Landlock.
# This flag affects only the build-time downloader, not package signatures.
SOURCE_DATE_EPOCH=1 pacman -Syu --disable-sandbox --noconfirm --needed \
    bash bubblewrap ca-certificates curl git python python-pillow python-numpy \
    ripgrep unzip xz zstd openssl dbus glibc libunwind icu krb5 gmp \
    hyprland quickshell foot chromium nautilus grim slurp wl-clipboard wtype \
    xorg-server-xvfb xorg-xdpyinfo xorg-xset xorg-xrandr xorg-xwininfo xorg-xprop xdotool \
    wayvnc tigervnc wlr-randr python-pip scrot labwc mesa libglvnd xorg-xwayland \
    qt6-imageformats qt6-multimedia qt6-svg qt6-wayland \
    noto-fonts noto-fonts-emoji ttf-jetbrains-mono-nerd-basic yaru-icon-theme \
    jq gum imagemagick inotify-tools libnotify procps-ng util-linux which \
    xdg-utils xdg-terminal-exec xdg-desktop-portal-hyprland python-gobject pipewire wireplumber
git -C /usr/share/omarchy apply /tmp/remove-google-account.patch
rm /tmp/remove-google-account.patch
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
bash /tmp/build-graphics.sh
# The sandbox's own installs use Omarchy's stable repositories.
cp /usr/share/omarchy/default/pacman/pacman-stable.conf /etc/pacman.conf
cp /usr/share/omarchy/default/pacman/mirrorlist-stable /etc/pacman.d/mirrorlist
rm -rf "$omarchy_packages"
pacman -Q > /usr/share/cma-omarchy/packages.txt
find /var/cache/pacman/pkg -type f -name '*.pkg.tar.*' ! -name '*.sig' -exec sha256sum {} + | sort -k2 > /usr/share/cma-omarchy/package-sha256.txt
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

# Finish cleanup before this signed package-install layer is committed.
gpgconf --homedir /etc/pacman.d/gnupg --kill gpg-agent
python /tmp/normalize-omarchy.py
