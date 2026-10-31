#!/bin/bash
# Pinned software renderer integration for device-free production microVMs.
set -euo pipefail
build_dir=$1
test "$(pacman -Q hyprland)" = 'hyprland 0.56.2-4'
curl -fsSL https://github.com/hyprwm/Hyprland/archive/refs/tags/v0.56.2.tar.gz -o "$build_dir/hyprland.tar.gz"
echo "ddeb92a654fa65d41ed2add3176de4fe54eab3752441c33ca99f5e598e0571e5  $build_dir/hyprland.tar.gz" | sha256sum -c -
mkdir "$build_dir/hyprland"
tar xzf "$build_dir/hyprland.tar.gz" -C "$build_dir/hyprland" --strip-components=1
patch --batch --fuzz=0 -d "$build_dir/hyprland" -p1 < /tmp/hyprland-shm.patch
cmake -S "$build_dir/hyprland" -B "$build_dir/hyprland-build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DNO_HYPRPM=ON \
  -DNO_SYSTEMD=ON -DNO_UWSM=ON
cmake --build "$build_dir/hyprland-build" --target Hyprland --parallel 2
install -m 0755 "$build_dir/hyprland-build/Hyprland" /usr/bin/Hyprland
strip /usr/bin/Hyprland
{
  printf '\nHyprland 0.56.2 with nested SHM software output\n'
  sha256sum "$build_dir/hyprland.tar.gz" /tmp/hyprland-shm.patch /usr/bin/Hyprland
} >> /usr/share/cma-omarchy/patched-graphics.txt
