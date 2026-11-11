# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY sdk/typescript/package.json sdk/typescript/package-lock.json /build/sdk/typescript/
RUN npm ci --prefix /build/sdk/typescript --omit=dev --ignore-scripts
COPY web/ ./
COPY sdk/typescript/src/ /build/sdk/typescript/src/
# Public Event Gateway documentation shares the backend's canonical sources.
COPY docs/CMAEG_Integration.md docs/CMAEG_Architecture.md docs/CMAEG_Protocol.md /build/docs/
COPY api/crates/cmaeg/assets/docs/ /build/api/crates/cmaeg/assets/docs/
# Every image is a promotable artifact; local Vite builds remain explicitly local.
# The bundle and final OCI metadata receive identical immutable inputs.
ARG CHRONO_BUILD=dev
ARG CHRONO_BUILD_SHA=development
ENV CHRONO_BUILD=$CHRONO_BUILD \
    CHRONO_BUILD_SHA=$CHRONO_BUILD_SHA \
    CHRONO_BUILD_ARTIFACT=promotable
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.29-alpine AS runtime
ARG CHRONO_BUILD=dev
ARG CHRONO_BUILD_SHA=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"

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
COPY --from=build /build/web/dist /usr/share/nginx/html
# The retained-assets volume overlays html/assets in deployed Pods. Keep the
# current bundle outside that mount so an image-only rollout can always boot.
COPY --from=build /build/web/dist/assets /usr/share/nginx/current/assets
# Same-origin proxy for the API, so the bundle needs no knowledge of where the
# control plane lives.
# Includes route-specific 20 MiB attachment uploads, 43 MiB encoded Agent file
# writes (32 MiB decoded), and compression of immutable JavaScript/CSS assets.
COPY web/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 8080
# Health is asserted by the Kubernetes probes in infra/web/deployment.yaml.
