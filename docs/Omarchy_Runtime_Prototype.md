# Omarchy runtime prototype

The explicit `omarchy` image target packages Omarchy 4.0.6's actual Hyprland
and Quickshell desktop on Arch Linux. It is experimental and is not the
default CMA Machine image. The normal Dockerfile target remains Debian/XFCE.

## Recorded result: 31 October 2026

The image builds, but its desktop verification fails. It is not ready for the
CMA UI or a production image switch. The isolated campaign is
[Actions run 41100886329](https://github.com/ChronoAIProject/cma/actions/runs/41100886329),
using source `3f7bef51e0676abef0fb6e51e701b037bcb0d249` and Docker image ID
`sha256:832a39526c7c00e385bdeebc66cb92b943d22bac88ae914d98601092714b1130`.

All 34 CMA runtime contract checks pass, including nonroot/read-only operation,
empty-home initialization, credential and installation gates, native executable
identity, Agent shell paths, checkpoint fencing/recovery and persistent bootstrap
restart. The prototype verifies the actual Codex 0.165 helper layout in a
temporary copy of the older probe; the standard runtime gate is unchanged.

Neither desktop configuration passed on this image:

- Without graphics devices, the VNC viewer connected but startup could not find
  its window in the X11 display used by native tools. Startup stopped before
  Hyprland launched. The installed viewer uses FLTK 1.3; a preliminary theory
  that it had selected Wayland was incorrect.
- The VKMS test stopped before creating its container: driver discovery assumed
  a `/sys/class/drm/card*/device/driver` link ending in `vkms`. Linux 7 uses a
  faux VKMS device and does not expose that link as expected. This run provides
  no evidence about rendering with VKMS.

The subsequent integration selects X11 explicitly and uses DRM's version ioctl
to identify the actual driver on both the test host and inside the container.
Run `41177946179` proves VKMS discovery succeeds, then shows wlroots rejecting
its primary node because it expects a render node. The next integration rebuilds
the pinned wlroots 0.20.2 library with a narrow exception for a primary node
whose actual DRM driver is `vkms`; other primary nodes remain refused. It also
records the X11 window tree before failed startup is cleaned up.

The runtime refuses desktop readiness. Native Agent screen control, authenticated
framebuffer input, the Omarchy shell, browser-close survival and desktop-state
persistence are therefore **not verified**. The test harness stops before those
checks; a built archive is not evidence that they work.

The configured Trivy scan passes with zero findings. Build-only pip and its
dependencies were removed after installing the pinned Websockify wheel, and
Gum/captree were rebuilt with pinned patched Go tools and dependencies. The scan
checks HIGH/CRITICAL findings with fixes available; it does not report unfixed
advisories or lower severities.
Trivy did not detect an Arch OS package inventory, so this scan does not provide
complete OS vulnerability coverage. No finding was suppressed to admit the image.

The [candidate archive](https://github.com/ChronoAIProject/cma/actions/runs/41100886329/artifacts/12997689773)
is approximately 1.72 GB compressed and contains the Docker-save archive,
checksum/identity metadata and evidence, retained for seven days. The
[separate evidence artifact](https://github.com/ChronoAIProject/cma/actions/runs/41100886329/artifacts/12997993526)
contains the small logs and verification reports without the image download.
Admission is `blocked` and `productionValidated` is `false`.

A functioning DRM/DMA-BUF rendering path and a compatible Wayland display
transport require further integration and acceptance. The results do not prove
that every virtual GPU configuration is unsupported. No production Kata Sandbox,
existing Agent, default image or user desktop was changed in this campaign.

The prototype preserves CMA runtime contract 3, the existing supervisor,
Codex 0.165.0 and its patched ACP bridge from a verified desktop CI artifact.
The build records the base artifact provenance, pinned Omarchy source,
installed package versions and package archive hashes. Arch/Omarchy package
repositories remain rolling inputs; the resulting image ID identifies the
exact built candidate. Rebuilding later may select different signed packages.

## Display integration

The prototype runs Hyprland as a native Wayland compositor nested in a headless
labwc output. Labwc uses Mesa's software renderer with an accessible DRM device
when present. WayVNC captures that output; a local TigerVNC viewer on Xvfb lets
the existing native Agent shell/image tools see and control the same desktop.
The owner connects to WayVNC through CMA's existing authenticated desktop relay.
Both clients share the same Wayland seat. The intended desktop uses the real
Omarchy shell for the bar, wallpaper and menus, with native Wayland Chromium and
Foot. These interaction paths remain unverified while startup fails.

Acceptance compares startup without graphics devices with a VKMS primary node
mapped into the nonroot container on a disposable runner. VKMS is a virtual
graphics device, not a physical GPU; device-free operation has not been
established. The pinned Aquamarine Wayland backend requires DMA-BUF and a usable
DRM allocator. The earlier X11-hosted labwc/vgem attempts failed allocator
initialization; this is why the host compositor now renders headlessly.

The supervisor owns this whole process group. The container runs as UID 10001
with no added capabilities, a read-only root, no network during acceptance,
and persistent home storage. Hardware login/power services and systemd are not
started. CMA supplies startup and an application launcher in place of UWSM.
These are adaptations for a hosted desktop, not a complete stock Omarchy OS
installation. System package installation still belongs to image builds.

## Verification

The branch-only `Omarchy runtime prototype` workflow uses the retained candidate
above as a **runtime binary donor only**, because the original successful
Debian desktop export expired. It pins the outer artifact SHA-256, source,
workflow and image ID, verifies its archive checksum and all 34 runtime checks,
and retains the original successful base provenance and executable hashes.
The donor's blocked desktop admission remains explicit in `binary-donor.json`;
it is not admitted for deployment. The resulting new image must pass its own
checks. The workflow verifies
the CMA runtime contract and then attempts desktop startup. If startup succeeds,
the remaining checks require actual Omarchy processes, native Codex shell/image
tool execution, a native Wayland Chromium window, authenticated framebuffer and
user input, browser-close survival, and retained browser state across restart.
Model replies use a local deterministic fixture; no customer agent or live
model is used. Logs, source identity and the vulnerability scan accompany the
image archive; screenshots and interaction evidence are produced only when
startup permits those checks. Package versions and hashes are embedded in the
image under `/usr/share/cma-omarchy`. Failed checks remain failing workflow gates.
The diagnostic image archive is retained even after a failed desktop check,
with `admission: blocked` in its metadata; its presence is not a readiness claim.

A passing Docker campaign does not establish deployment compatibility with
the production Kata runtime or browser integration through CMA's public API.
That requires a separate disposable CMA Sandbox using the candidate's pinned
registry digest. Existing Agents, Sandbox images and production defaults must
remain untouched during prototype verification.
