#!/bin/bash
# Build the installed wlroots ABI with narrowly scoped VKMS primary-node support.
set -euo pipefail
build_dir=$(mktemp -d /tmp/cma-omarchy-graphics.XXXXXX)
trap 'rm -rf "$build_dir"' EXIT
test "$(pacman -Q wlroots0.20)" = 'wlroots0.20 0.20.2-1'
pacman -Qq | sort > "$build_dir/packages-before"
pacman -S --disable-sandbox --noconfirm --needed gcc make meson ninja glslang vulkan-headers pkgconf patch
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
# Keep the runtime's original packages; remove only newly added build tooling.
pacman -Qq | sort > "$build_dir/packages-after"
mapfile -t build_packages < <(comm -13 "$build_dir/packages-before" "$build_dir/packages-after")
if ((${#build_packages[@]})); then pacman -Rns --noconfirm "${build_packages[@]}"; fi
ldd /usr/bin/labwc | tee "$build_dir/labwc-libraries"
! grep -q 'not found' "$build_dir/labwc-libraries"
