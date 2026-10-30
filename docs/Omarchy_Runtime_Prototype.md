# Omarchy runtime prototype

The explicit `omarchy` image target packages Omarchy 4.0.6's actual Hyprland
and Quickshell desktop on Arch Linux. It is experimental and is not the
default CMA Machine image. The normal Dockerfile target remains Debian/XFCE.

## Recorded result: 30 October 2026

The image builds, but its desktop verification fails. It is not ready for the
CMA UI or a production image switch. The isolated campaign is
[Actions run 41048037767](https://github.com/ChronoAIProject/cma/actions/runs/41048037767),
using source `711698404e353e316411905441e515fe5283f112` and Docker image ID
`sha256:b9aed95faf52a10b0b14adef17301489aa64d6b59e0de934ca98d7cf17a82253`.

All 34 CMA runtime contract checks pass, including nonroot/read-only operation,
empty-home initialization, credential and installation gates, native executable
identity, Agent shell paths, checkpoint fencing/recovery and persistent bootstrap
restart. The prototype verifies the actual Codex 0.165 helper layout in a
temporary copy of the older probe; the standard runtime gate is unchanged.

Both graphics configurations fail on the same built image:

- Without graphics devices, labwc supplies shared-memory rendering but cannot
  initialize DMA-BUF. Hyprland starts and aborts with `CBackend::create() failed!`.
  The pinned Aquamarine Wayland backend requires the Linux DMA-BUF protocol and
  a usable DRM allocator; shared-memory rendering alone is insufficient.
- With the disposable runner's `vgem` devices mapped into the nonroot container,
  Mesa initializes llvmpipe, but reports `EGL device has no render node`, followed
  by `Failed to create allocator`. Exposing these devices did not establish a
  working graphics path through the nested X11 transport.

The runtime refuses desktop readiness. Native Agent screen control, authenticated
framebuffer input, the Omarchy shell, browser-close survival and desktop-state
persistence are therefore **not verified**. The test harness stops before those
checks; a built archive is not evidence that they work.

The final vulnerability scan also fails: 1 critical and 41 high findings
(34 unique advisories) in bundled Python packages and Go dependencies/toolchains.
Trivy did not detect an Arch OS package inventory, so this scan does not provide
complete OS vulnerability coverage. No finding was suppressed to admit the image.

The [candidate archive](https://github.com/ChronoAIProject/cma/actions/runs/41048037767/artifacts/12974001594)
is approximately 1.73 GB compressed and contains the Docker-save archive,
checksum/identity metadata and evidence. It expires on 6 November 2026 at
19:36 Singapore time. The [separate evidence artifact](https://github.com/ChronoAIProject/cma/actions/runs/41048037767/artifacts/12973877070)
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

The prototype attempts to run Hyprland as a native Wayland compositor nested
in labwc, which renders into the existing Xvfb display. Acceptance compares no-device software rendering
with a virtual DRM device on a disposable runner; GPU-free operation has not
been established. The intended desktop uses the real Omarchy shell for the bar,
wallpaper and menus, with native Wayland Chromium and Foot. User and agent
pointer and keyboard events are intended to enter the same outer display,
streamed by the existing authenticated VNC transport. Those interaction paths
remain unverified while desktop startup fails.

The supervisor owns this whole process group. The container runs as UID 10001
with no added capabilities, a read-only root, no network during acceptance,
and persistent home storage. Hardware login/power services and systemd are not
started. CMA supplies startup and an application launcher in place of UWSM.
These are adaptations for a hosted desktop, not a complete stock Omarchy OS
installation. System package installation still belongs to image builds.

## Verification

The branch-only `Omarchy runtime prototype` workflow downloads and verifies an
exact successful CMA desktop artifact before building the candidate. It verifies
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
