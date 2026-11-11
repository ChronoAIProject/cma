#!/bin/bash
# Build the installed wlroots ABI with narrowly scoped VKMS primary-node support.
set -euo pipefail
build_dir=$(mktemp -d /tmp/cma-omarchy-graphics.XXXXXX)
trap 'rm -rf "$build_dir"' EXIT
test "$(pacman -Q wlroots0.20)" = 'wlroots0.20 0.20.2-1'
test "$(pacman -Q aquamarine)" = 'aquamarine 0.15.1-1'
pacman -Qq | sort > "$build_dir/packages-before"
pacman -S --disable-sandbox --noconfirm --needed gcc make meson ninja glslang vulkan-headers pkgconf patch cmake hyprwayland-scanner hyprland-protocols glaze
curl -fsSL https://gitlab.freedesktop.org/wlroots/wlroots/-/archive/0.20.2/wlroots-0.20.2.tar.gz -o "$build_dir/source.tar.gz"
echo "972c7ac44b17828f4702bfae7cd8347346a3fb5b2c1076cfa2c3fcedac5ec343  $build_dir/source.tar.gz" | sha256sum -c -
mkdir "$build_dir/source"
tar xzf "$build_dir/source.tar.gz" -C "$build_dir/source" --strip-components=1
patch --batch --fuzz=0 -d "$build_dir/source" -p1 < /tmp/wlroots-vkms.patch
meson setup "$build_dir/build" "$build_dir/source" --prefix=/usr --libdir=lib \
  --buildtype=release -Dstrip=true -Dexamples=false \
  -Drenderers=gles2,vulkan -Dbackends=drm,libinput,x11 -Dxwayland=enabled -Dsession=enabled
meson compile -C "$build_dir/build" -j 2
meson install -C "$build_dir/build" --destdir "$build_dir/installed" --no-rebuild
cp -a "$build_dir/installed/usr/lib/"libwlroots-0.20.so* /usr/lib/
ldconfig
labwc --version
{
  printf 'wlroots 0.20.2, source SHA-256: '
  sha256sum "$build_dir/source.tar.gz"
  sha256sum /tmp/wlroots-vkms.patch /usr/lib/libwlroots-0.20.so
  meson introspect --buildoptions "$build_dir/build"
} > /usr/share/cma-omarchy/patched-graphics.txt
# Upstream #415 defers rendering until xdg_surface.configure is acknowledged.
# Without it, nested Hyprland commits a buffer before the host permits one.
curl -fsSL https://github.com/hyprwm/aquamarine/archive/refs/tags/v0.15.1.tar.gz -o "$build_dir/aquamarine.tar.gz"
echo "2f9de98c0bd1b7b1b09c576e390a2fef436449762fb334163c414f0c300296f2  $build_dir/aquamarine.tar.gz" | sha256sum -c -
# The web commit-patch route throttles CI; the API returns the identical pinned bytes.
curl -fsSL --connect-timeout 15 --max-time 120 --retry 3 --retry-max-time 180 \
  -H 'Accept: application/vnd.github.patch' -H 'X-GitHub-Api-Version: 2022-11-28' \
  https://api.github.com/repos/hyprwm/aquamarine/commits/7bb8bdf4e8fedaf4dfae58512bc4c728671727df \
  -o "$build_dir/aquamarine.patch"
echo "542a3075643971d69862e7481446b9ec8f368338d1dcbc13ce26438a0e7e23cf  $build_dir/aquamarine.patch" | sha256sum -c -
mkdir "$build_dir/aquamarine"
tar xzf "$build_dir/aquamarine.tar.gz" -C "$build_dir/aquamarine" --strip-components=1
patch --batch --fuzz=0 -d "$build_dir/aquamarine" -p1 < "$build_dir/aquamarine.patch"
patch --batch --fuzz=0 -d "$build_dir/aquamarine" -p1 < /tmp/aquamarine-fullscreen.patch
patch --batch --fuzz=0 -d "$build_dir/aquamarine" -p1 < /tmp/aquamarine-shm.patch
cp /tmp/shm-allocator.hpp "$build_dir/aquamarine/src/backend/cma-shm-allocator.hpp"
cmake -S "$build_dir/aquamarine" -B "$build_dir/aquamarine-build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib
cmake --build "$build_dir/aquamarine-build" --target aquamarine --parallel 2
DESTDIR="$build_dir/aquamarine-installed" cmake --install "$build_dir/aquamarine-build"
cp -a "$build_dir/aquamarine-installed/usr/lib/"libaquamarine.so* /usr/lib/
cp -a "$build_dir/aquamarine-installed/usr/include/aquamarine/." /usr/include/aquamarine/
ldconfig
{
  printf '\nAquamarine 0.15.1 with upstream configure fix 7bb8bdf4e8fedaf4dfae58512bc4c728671727df\n'
  sha256sum "$build_dir/aquamarine.tar.gz" "$build_dir/aquamarine.patch" /tmp/aquamarine-fullscreen.patch /tmp/aquamarine-shm.patch /tmp/shm-allocator.hpp /usr/lib/libaquamarine.so
} >> /usr/share/cma-omarchy/patched-graphics.txt
bash /tmp/build-hyprland.sh "$build_dir"
# Keep the runtime's original packages; remove only newly added build tooling.
pacman -Qq | sort > "$build_dir/packages-after"
mapfile -t build_packages < <(comm -13 "$build_dir/packages-before" "$build_dir/packages-after")
if ((${#build_packages[@]})); then pacman -Rns --noconfirm "${build_packages[@]}"; fi
ldd /usr/bin/labwc /usr/bin/Hyprland | tee "$build_dir/labwc-libraries"
! grep -q 'not found' "$build_dir/labwc-libraries"
