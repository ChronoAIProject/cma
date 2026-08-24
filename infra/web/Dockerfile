# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build/sdk/typescript
COPY sdk/typescript/package.json sdk/typescript/package-lock.json ./
RUN npm ci
# The web bundle consumes both SDK documentation trees. Keep sdk/ as one build
# input so a new guide cannot work locally and disappear from the image build.
COPY sdk/ /build/sdk/
RUN npm run build

WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
# Which build the status band names (docs/Web_Style.md section 6). Baked into
# the bundle here because that is the only moment the answer is knowable and
# cannot later disagree with the code. The publisher passes the release
# version; a plain `docker build` leaves the honest default.
ARG CHRONO_BUILD=dev
ENV CHRONO_BUILD=$CHRONO_BUILD
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.29-alpine AS runtime
# Pull security fixes published after the base image was cut. Without this the
# image ships known-fixed CVEs and the vulnerability gate fails, correctly.
USER root
RUN apk upgrade --no-cache
USER 101:101
COPY --from=build /build/web/dist /usr/share/nginx/html
# Same-origin proxy for the API and the terminal WebSocket, so the bundle needs
# no knowledge of where the control plane lives.
COPY web/nginx.conf /etc/nginx/conf.d/default.conf
# $connection_upgrade is what turns a proxied request into a WebSocket upgrade.
# Named to sort before default.conf so the map exists when the server uses it.
COPY web/nginx-upgrade.conf /etc/nginx/conf.d/00-upgrade.conf
EXPOSE 8080
# Health is asserted by the Kubernetes probes in infra/web/deployment.yaml.
