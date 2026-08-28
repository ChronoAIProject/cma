# syntax=docker/dockerfile:1
FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
RUN CHRONO_BUILD="${CHRONO_BUILD}" \
    CHRONO_BUILD_SHA="${CHRONO_BUILD_SHA}" \
    cargo build --release --locked --bin cma

FROM alpine:3.22 AS runtime
# Pull security fixes published after the base image was cut. ca-certificates is
# required, not optional: the control plane makes outbound HTTPS calls to the
# identity provider, and an empty trust store fails every one of them. CI and
# release builds change this value on every attempt so Docker cannot reuse an
# OS refresh layer after the package repository publishes a fix.
ARG OS_SECURITY_REFRESH=local
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apk upgrade --no-cache \
    && apk add --no-cache ca-certificates \
    && adduser -D -u 10001 chrono
USER 10001:10001
COPY --from=build /build/target/release/cma /usr/local/bin/cma
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cma"]
# Health is asserted by the Kubernetes probes in infra/api/deployment.yaml.
