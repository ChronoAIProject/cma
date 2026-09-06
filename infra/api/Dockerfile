# syntax=docker/dockerfile:1
FROM alpine:3.22 AS secret-scanner
ARG TARGETARCH
RUN apk add --no-cache ca-certificates curl \
    && case "${TARGETARCH}" in \
      amd64) archive=x64; checksum=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb ;; \
      arm64) archive=arm64; checksum=e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080 ;; \
      *) exit 1 ;; \
    esac \
    && curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
      "https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_${archive}.tar.gz" \
      --output /tmp/scanner.tar.gz \
    && printf '%s  /tmp/scanner.tar.gz\n' "${checksum}" | sha256sum -c - \
    && tar -xzf /tmp/scanner.tar.gz -C /usr/local/bin gitleaks \
    && test "$(/usr/local/bin/gitleaks version)" = '8.30.1'

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
COPY --from=secret-scanner /usr/local/bin/gitleaks /usr/local/bin/gitleaks
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cma"]
# Health is asserted by the Kubernetes probes in infra/api/deployment.yaml.
