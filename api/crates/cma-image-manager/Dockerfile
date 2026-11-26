# syntax=docker/dockerfile:1
FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
ARG TARGETARCH
ARG CHRONO_BUILD_SHA=development
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-image-manager-target-${TARGETARCH},target=/build/target,sharing=locked \
    CHRONO_BUILD_SHA="${CHRONO_BUILD_SHA}" cargo build --release --locked --bin cma-image-manager && mkdir /out && cp target/release/cma-image-manager /out/
FROM alpine:3.22
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
ARG OS_SECURITY_REFRESH=local
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"
RUN test -n "${OS_SECURITY_REFRESH}" && apk upgrade --no-cache && apk add --no-cache ca-certificates && adduser -D -u 10001 chrono
COPY --from=build /out/cma-image-manager /usr/local/bin/cma-image-manager
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cma-image-manager"]
