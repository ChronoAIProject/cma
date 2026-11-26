# syntax=docker/dockerfile:1
# Optional local-preview callers supply their qualified base through this argument.
# The public default contains no private registry identity.
ARG CMAEG_LOCAL_PREVIEW_BASE=alpine:3.22

FROM rust:1.96.1-alpine AS chef
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/rust-toolchain.toml ./
RUN cargo install --locked --version 0.1.78 cargo-chef

FROM chef AS planner
COPY api/ ./
RUN cargo chef prepare --recipe-path /recipe.json

FROM chef AS dependencies
COPY --from=planner /recipe.json /recipe.json
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    cargo chef cook --release --locked --bin cmaeg --recipe-path /recipe.json \
    && cargo clean --release --workspace --locked

FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache binutils musl-dev python3
WORKDIR /build
COPY api/ ./
ARG TARGETARCH
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
# Validate restored workspace objects against the copied sources before Cargo
# sees their fingerprints. Keep the registry dependency stage independent.
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-api-target-${TARGETARCH},target=/build/target,sharing=locked \
    --mount=type=bind,from=dependencies,source=/build/target,target=/dependency-target \
    export CARGO_INCREMENTAL=1 \
    && export RUSTC_WRAPPER=/build/build_support/release_rustc.py \
    && python3 build_support/workspace_cache.py prepare cmaeg --dependency-target /dependency-target \
    && cargo build --release --locked --bin cmaeg \
      --verbose --config profile.release.package.cmaeg.codegen-units=256 \
    && mkdir /out \
    && cp /build/target/release/cmaeg /out/cmaeg \
    && python3 build_support/workspace_cache.py finish cmaeg
# Strip only the shipped ELF, leaving Cargo cache artifacts intact.
RUN --network=none strip --strip-all /out/cmaeg

# Qualify the actual linked allocator. Static musl ELF metadata alone does not
# exclude a cross-linker's replacement malloc retaining freed small blocks.
RUN --network=none /out/cmaeg --allocator-self-test
# Exercise the musl binary's sandbox (CI tests use glibc): gaps such as the
# sched_setscheduler stub or a missing close_range fail the image build here.
RUN --network=none <<'SANDBOX_CHECK'
set -eu
case "${TARGETARCH}" in
  amd64) /out/cmaeg --media-sandbox-check ;;
  *) set +e; /out/cmaeg --media-sandbox-check; status=$?; set -e; test "${status}" -eq 125 ;;
esac
SANDBOX_CHECK

# Minimal LGPL-only FFmpeg for the sandboxed media transformer. Only the
# components the fixed argv in media/transform/argv.rs needs are compiled.
# Literal pins; see private maintainer guidance "FFmpeg in the gateway image".
FROM alpine:3.22 AS ffmpeg-build
RUN apk add --no-cache build-base nasm pkgconf curl ca-certificates
WORKDIR /src/ffmpeg
RUN <<'FETCH_FFMPEG'
set -eu
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
  'https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz' --output /tmp/ffmpeg.tar.xz
printf '%s\n' '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e  /tmp/ffmpeg.tar.xz' | sha256sum -c -
tar -xJf /tmp/ffmpeg.tar.xz --strip-components=1 -C /src/ffmpeg
rm /tmp/ffmpeg.tar.xz
FETCH_FFMPEG
# `trim` and `atrim` serve accurate `-ss` and output `-t`; `null`, `anull`,
# `format` and `aformat` serve the graphs FFmpeg builds; every output goes
# to stdout, so `image2pipe` and `pcm_s16le` (`-f s16le`) are the only muxers.
RUN --network=none <<'BUILD_FFMPEG'
set -eu
./configure --prefix=/opt/ffmpeg --enable-static --disable-shared \
  --extra-ldflags=-static --pkg-config-flags=--static \
  --disable-autodetect --disable-network --disable-everything --disable-doc \
  --disable-debug --disable-ffplay --enable-ffmpeg --enable-ffprobe \
  --enable-swscale --enable-swresample --enable-protocol=file,pipe \
  --enable-demuxer=mov,matroska,ogg,mp3,wav,flac,aac,gif,amr \
  --enable-parser=h264,hevc,vp8,vp9,mpeg4video,aac,opus,vorbis,flac,mpegaudio,gif \
  --enable-decoder=h264,hevc,vp8,vp9,mpeg4,aac,mp3float,opus,vorbis,flac,pcm_s16le,pcm_f32le,amrnb,amrwb,gif,mjpeg \
  --enable-encoder=mjpeg,pcm_s16le --enable-muxer=image2pipe,pcm_s16le \
  --enable-filter=scale,format,aresample,aformat,trim,atrim,null,anull
make -j"$(nproc)"
make install
strip /opt/ffmpeg/bin/ffmpeg /opt/ffmpeg/bin/ffprobe
for binary in ffmpeg ffprobe; do
  if readelf -d "/opt/ffmpeg/bin/${binary}" | grep -q NEEDED; then
    echo "${binary} is not fully static" >&2
    exit 1
  fi
done
# The notice wraps across lines, so it is joined before matching.
if ! /opt/ffmpeg/bin/ffmpeg -hide_banner -L | tr '\n' ' ' | grep -q 'GNU Lesser General Public License'; then
  echo 'FFmpeg does not report the LGPL' >&2
  exit 1
fi
if /opt/ffmpeg/bin/ffmpeg -hide_banner -buildconf | grep -Eq -- '--enable-(gpl|nonfree|version3)'; then
  echo 'FFmpeg must be built LGPL-2.1-or-later only' >&2
  exit 1
fi
mkdir -p /opt/ffmpeg/share/cmaeg
cp COPYING.LGPLv2.1 /opt/ffmpeg/share/cmaeg/LICENSE
/opt/ffmpeg/bin/ffmpeg -hide_banner -buildconf > /opt/ffmpeg/share/cmaeg/configure.txt
printf '%s\n' 'FFmpeg 9.0.2, LGPL-2.1-or-later' \
  'source=https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz' \
  'archive.sha256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e' \
  > /opt/ffmpeg/share/cmaeg/source.txt
sha256sum /opt/ffmpeg/bin/ffmpeg /opt/ffmpeg/bin/ffprobe > /opt/ffmpeg/share/cmaeg/binaries.sha256
BUILD_FFMPEG

# Every committed fixture through the real transformer path under the real
# sandbox: an image whose FFmpeg cannot serve the fixed argv never builds.
# COPY leaves /opt/ffmpeg/bin root-owned and unwritable, as the gateway
# requires. The report is printed before a failure fails the build.
FROM alpine:3.22 AS media-self-test
ARG TARGETARCH
RUN adduser -D -u 10001 chrono && install -d -o 10001 -g 10001 -m 0700 /var/cmaeg/media
COPY --from=build /out/cmaeg /usr/local/bin/cmaeg
COPY --from=ffmpeg-build /opt/ffmpeg/bin/ffmpeg /opt/ffmpeg/bin/ffprobe /opt/ffmpeg/bin/
COPY api/crates/cmaeg/tests/fixtures/media/ /opt/cmaeg-media-fixtures/
USER 10001:10001
RUN --network=none <<'MEDIA_SELF_TEST'
set -eu
case "${TARGETARCH}" in
  amd64)
    status=0
    CMAEG_MEDIA_FFMPEG_DIR=/opt/ffmpeg/bin /usr/local/bin/cmaeg \
      --media-self-test /opt/cmaeg-media-fixtures > /tmp/media-self-test.txt || status=$?
    cat /tmp/media-self-test.txt
    test "${status}" -eq 0
    ;;
  *)
    echo 'media transformer is linux/amd64 only; self-test not applicable' > /tmp/media-self-test.txt
    cat /tmp/media-self-test.txt
    ;;
esac
MEDIA_SELF_TEST

# Explicit local preview packaging only. The named local-binary context holds
# a verified static Linux amd64 cmaeg executable. No compilation or emulated
# RUN occurs here. The pinned production base retains CA/Codex/Gitleaks; it
# does not include FFmpeg. Keep CMAEG_MEDIA_FFMPEG_DIR empty for this target.
# Standard builds still use the final runtime target below with all its gates.
FROM ${CMAEG_LOCAL_PREVIEW_BASE} AS local-preview
ARG CHRONO_BUILD=local-preview
ARG CHRONO_BUILD_SHA
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}" \
      dev.chrono.cmaeg.packaging="local-preview-no-transformer" \
      org.opencontainers.image.base.digest="sha256:ca748e49dbc69553bb7cfbf6456fe0ab0a13dc4357f9aa6c517865cdb090987c"
COPY --from=local-binary --chmod=0755 /cmaeg /usr/local/bin/cmaeg

FROM alpine:3.22 AS runtime
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
LABEL org.opencontainers.image.title="CMA Event Gateway" \
      org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"
# Pull security fixes published after the base image was cut. ca-certificates is
# required, not optional: the control plane makes outbound HTTPS calls to the
# identity provider, and an empty trust store fails every one of them. CI and
# release builds change this value on every attempt so Docker cannot reuse an
# OS refresh layer after the package repository publishes a fix.
ARG OS_SECURITY_REFRESH=local
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apk upgrade --no-cache \
    && apk add --no-cache ca-certificates \
    && adduser -D -u 10001 chrono \
    && install -d -m 0755 /var/cmaeg/media
USER 10001:10001
COPY --from=build /out/cmaeg /usr/local/bin/cmaeg
COPY --from=ffmpeg-build /opt/ffmpeg/bin/ffmpeg /opt/ffmpeg/bin/ffprobe /opt/ffmpeg/bin/
COPY --from=ffmpeg-build /opt/ffmpeg/share/cmaeg /usr/local/share/ffmpeg
# Forces the media-self-test stage: BuildKit skips stages nothing depends on.
COPY --from=media-self-test /tmp/media-self-test.txt /usr/local/share/cmaeg/media-self-test.txt
EXPOSE 8090
EXPOSE 9091
# CMAEG_METRICS_LISTEN defaults to 0.0.0.0:9091; /metrics is served separately
# from the public management, ingress and tools listener.
# Runtime contract: CMAEG_PUBLIC_URL, CMAEG_CMA_BASE_URL and CMAEG_NYXID_BASE_URL
# are required origins. Inject CMAEG_MASTER_KEYS and CMAEG_CMA_SERVICE_TOKEN
# from Kubernetes Secrets; startup refuses missing or invalid values before binding.
# CMAEG_CMA_CLIENT_ID and CMAEG_CMA_CLIENT_SECRET enable automatic outbound CMA
# token renewal; the service token then remains the separate inbound rekey bearer.
# CMAEG_TOOLS_NYXID_SLUG defaults to cmaeg. Channel cache settings default to
# CMAEG_CHANNEL_CACHE_TTL_MS=60000, CMAEG_CHANNEL_NEGATIVE_TTL_MS=10000,
# CMAEG_CHANNEL_CACHE_MAX=50000, CMAEG_RESTORE_RATE_PER_MINUTE=10 and
# CMAEG_RESTORE_TIMEOUT_MS=800. /readyz reports local readiness and CMA reachability.
# Relay callbacks use CMAEG_JWKS_TTL_MS=600000, CMAEG_INGRESS_MAX_BODY=1048576
# and CMAEG_INGRESS_INFLIGHT=256. POST /v1/callbacks/{channel_id} verifies callbacks.
# Turns use CMAEG_ACCEPT_BUDGET_MS=20000, CMAEG_TURN_TIMEOUT_MS=600000,
# CMAEG_BUSY_RETRY_MAX_MS=15000 and CMAEG_DRAIN_SECONDS=600. During shutdown,
# /readyz returns 503 draining while tracked turns finish within the drain deadline.
# Agent tools: /v1/events/{event_ref}/{replies,typing,context},
# /v1/events/{event_ref}/attachments/{index}, /v1/messages/{message_ref}/update
# and stateless POST /mcp (2025-06-18), which also lists readEventAttachment
# when CMAEG_MEDIA_ENABLED=true.
# Providers retain event_context on the Responses POST and serve authenticated
# GET at the configured conversation path plus /events/{event_id}.
# Delivery uses CMAEG_NYXID_REPLY_TIMEOUT_MS=5000, CMAEG_ACK_TEXT="Working on it…",
# CMAEG_HEARTBEAT_MS=60000 and CMAEG_FAILURE_TEXT for short failure notices.
# Eight workers pace each relay route at three seconds and drain queued sends.
# Custom ingress: POST /v1/channels/{channel_id}/events (256 KiB), optional
# mode=stream SSE, and signed HTTPS result webhooks with DNS pinning per attempt.
# Inbound media: CMAEG_MEDIA_TOOL_PROVIDERS=cma_codex lists the providers whose profile bindings install the gateway MCP tools.
# CMAEG_MEDIA_ENABLED=false keeps inbound media refused as attachment_unsupported.
# Inbound media (off by default): CMAEG_MEDIA_MAX_BYTES=20971520, CMAEG_MEDIA_INFLIGHT_BYTES=67108864, CMAEG_MEDIA_RESPONSE_BYTES=25165824, CMAEG_MEDIA_MAX_JOBS=6, CMAEG_MEDIA_CACHE_BYTES=33554432, CMAEG_MEDIA_RATE_PER_MINUTE=60, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR=268435456 and CMAEG_MEDIA_LINK_TTL_SECONDS=120.
# Speech-to-text spend: CMAEG_STT_MAX_SECONDS=600, CMAEG_STT_MAX_BYTES=20971520, CMAEG_STT_RATE_PER_MINUTE=20, CMAEG_STT_CHANNEL_SECONDS_PER_HOUR=3600 and CMAEG_STT_SECONDS_PER_HOUR=36000, validated at startup even while speech-to-text is off.
# Media turn preparation: CMAEG_MEDIA_PREPARE_BUDGET_MS=15000 (below CMAEG_TURN_TIMEOUT_MS) and CMAEG_STT_EAGER_MAX_SECONDS=300 (0 disables eager audio transcripts).
# Speech-to-text (off while CMAEG_STT_URL is empty): CMAEG_STT_MODEL, CMAEG_STT_FORMATS, CMAEG_STT_TIMEOUT_MS; inject CMAEG_STT_API_KEY from the cmaeg-stt Secret.
# Media transformer (off while CMAEG_MEDIA_FFMPEG_DIR is empty; linux/amd64 only): CMAEG_MEDIA_FFMPEG_DIR names a root-owned directory holding ffmpeg and ffprobe, CMAEG_MEDIA_VIDEO_FRAMES=6 keyframes per video, and job inputs live on the memory emptyDir at /var/cmaeg/media.
# CMAEG_MEDIA_FFMPEG_DIR=/opt/ffmpeg/bin enables the sandboxed media transformer (linux/amd64 only; see Operations). The FFmpeg licence and source record are in /usr/local/share/ffmpeg.
# See private maintainer guidance.
ENTRYPOINT ["/usr/local/bin/cmaeg"]
# Operators configure health probes outside this repository.
