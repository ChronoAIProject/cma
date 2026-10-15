# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY sdk/typescript/package.json sdk/typescript/package-lock.json /build/sdk/typescript/
RUN npm ci --prefix /build/sdk/typescript --omit=dev --ignore-scripts
COPY web/ ./
COPY sdk/typescript/src/ /build/sdk/typescript/src/
# Which build the status band names (docs/Web_Style.md section 6). Baked into
# the bundle here because that is the only moment the answer is knowable and
# cannot later disagree with the code. The publisher passes the release
# version; a plain `docker build` leaves the honest default.
ARG CHRONO_BUILD=dev
ENV CHRONO_BUILD=$CHRONO_BUILD
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
COPY --from=build /build/web/dist /usr/share/nginx/html
# Same-origin proxy for the API, so the bundle needs no knowledge of where the
# control plane lives.
# Includes the route-specific 20 MiB streaming attachment upload limit and
# compression of public immutable JavaScript/CSS assets.
COPY web/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 8080
# Health is asserted by the Kubernetes probes in infra/web/deployment.yaml.
