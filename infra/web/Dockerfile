# syntax=docker/dockerfile:1
FROM node:26-alpine AS build
WORKDIR /build
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.29-alpine AS runtime
# Pull security fixes published after the base image was cut. Without this the
# image ships known-fixed CVEs and the vulnerability gate fails, correctly.
USER root
RUN apk upgrade --no-cache
USER 101:101
COPY --from=build /build/dist /usr/share/nginx/html
EXPOSE 8080
# Health is asserted by the Kubernetes probes in infra/web/deployment.yaml.
