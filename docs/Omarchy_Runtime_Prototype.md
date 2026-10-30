# Omarchy runtime prototype

The explicit `omarchy` image target packages Omarchy 4.0.6's actual Hyprland
and Quickshell desktop on Arch Linux. It is experimental and is not the
default CMA Machine image. The normal Dockerfile target remains Debian/XFCE.

The prototype preserves CMA runtime contract 3, the existing supervisor,
Codex 0.165.0 and its patched ACP bridge from a verified desktop CI artifact.
The build records the base artifact provenance, pinned Omarchy source,
installed package versions and package archive hashes. Arch/Omarchy package
repositories remain rolling inputs; the resulting image ID identifies the
exact built candidate. Rebuilding later may select different signed packages.

## Display integration

Hyprland runs as a native Wayland compositor nested in labwc, which renders
into the existing Xvfb display. Acceptance compares no-device software rendering
with a virtual DRM device on a disposable runner; GPU-free operation has not
been established. The real Omarchy shell owns the bar, wallpaper and menus.
Chromium and Foot use native Wayland. User and agent pointer and keyboard
events enter the same outer display; the existing authenticated VNC transport
streams those pixels. There is no replacement computer-use service.

The supervisor owns this whole process group. The container runs as UID 10001
with no added capabilities, a read-only root, no network during acceptance,
and persistent home storage. Hardware login/power services and systemd are not
started. CMA supplies startup and an application launcher in place of UWSM.
These are adaptations for a hosted desktop, not a complete stock Omarchy OS
installation. System package installation still belongs to image builds.

## Verification

The branch-only `Omarchy runtime prototype` workflow downloads and verifies an
exact successful CMA desktop artifact before building the candidate. It tests
desktop startup, actual Omarchy processes, native Codex shell/image tool
execution, a native Wayland Chromium window, authenticated framebuffer and
user input, browser-close survival, and retained browser state across restart.
Model replies use a local deterministic fixture; no customer agent or live
model is used. Screenshots, package manifests, source identity and a vulnerability
scan accompany the image archive. Failed checks remain failing workflow gates.
The diagnostic image archive is retained even after a failed desktop check,
with `admission: blocked` in its metadata; its presence is not a readiness claim.

A passing Docker campaign does not establish deployment compatibility with
the production Kata runtime or browser integration through CMA's public API.
That requires a separate disposable CMA Sandbox using the candidate's pinned
registry digest. Existing Agents, Sandbox images and production defaults must
remain untouched during prototype verification.
