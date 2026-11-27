# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,id=cma-web-npm,target=/root/.npm,sharing=locked \
    npm ci --prefer-offline --no-fund
COPY web/ ./
# Public Event Gateway documentation shares the backend's canonical sources.
COPY api/crates/cmaeg/assets/docs/ /build/api/crates/cmaeg/assets/docs/
# Every image is a promotable artifact; local Vite builds remain explicitly local.
# The bundle and final OCI metadata receive identical immutable inputs.
ARG CHRONO_BUILD=dev
ARG CHRONO_BUILD_SHA=development
ARG CHRONO_WEB_REPLICA_RECOVERY_ENABLED=false
ARG VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=false
ENV CHRONO_BUILD=$CHRONO_BUILD \
    CHRONO_BUILD_SHA=$CHRONO_BUILD_SHA \
    CHRONO_BUILD_ARTIFACT=promotable \
    CHRONO_WEB_REPLICA_RECOVERY_ENABLED=$CHRONO_WEB_REPLICA_RECOVERY_ENABLED \
    VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=$VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED
RUN npm run build \
    && mkdir -p /build/image/current \
    && mv dist /build/image/html \
    && cp -al /build/image/html/assets /build/image/current/assets

FROM nginxinc/nginx-unprivileged:1.29-alpine-slim AS runtime
ARG CHRONO_BUILD=dev
ARG CHRONO_BUILD_SHA=development
ARG CHRONO_WEB_REPLICA_RECOVERY_ENABLED=false
LABEL org.opencontainers.image.title="CMA Web" \
      org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}" \
      io.chronoai.web.replica-recovery-enabled="${CHRONO_WEB_REPLICA_RECOVERY_ENABLED}"

# Pull security fixes published after the base image was cut. Without this the
# image ships known-fixed CVEs and the vulnerability gate fails, correctly.
# CI and release builds change this value on every attempt so Docker cannot
# reuse an OS refresh layer after the package repository publishes a fix.
ARG OS_SECURITY_REFRESH=local
USER root
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apk upgrade --no-cache
USER 101:101
# Includes the bounded pre-module /browser-telemetry.js diagnostic bootstrap.
# The retained-assets volume overlays html/assets in deployed Pods. Keep the
# current bundle outside that mount so an image-only rollout can always boot.
# Both paths are real directories. Their files share inodes in this single COPY
# layer, so the archive contains each asset's bytes once and both paths remain
# usable when the retained-assets directory is covered by a mount.
COPY --from=build /build/image/ /usr/share/nginx/
# Same-origin proxy for the API, so the bundle needs no knowledge of where the
# control plane lives.
# Includes route-specific 20 MiB attachment uploads, 43 MiB encoded Agent file
# writes (32 MiB decoded), and compression of immutable JavaScript/CSS assets.
# nginx.conf pins an 8 KiB request-line buffer; public login URLs stay <=4 KiB.
COPY web/nginx.conf /etc/nginx/conf.d/default.conf
COPY --chmod=0755 web/scripts/web-clear-drain.sh /docker-entrypoint.d/40-clear-web-drain.sh
COPY --chmod=0755 web/scripts/web-prestop.sh /usr/local/bin/cma-web-prestop
EXPOSE 8080
# Operators configure health probes outside this repository.
