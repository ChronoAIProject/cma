# syntax=docker/dockerfile:1
FROM rust:1.96.1-alpine AS journey-build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
ARG TARGETARCH
ARG CHRONO_BUILD_SHA
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-journey-probe-target-${TARGETARCH},target=/build/target,sharing=locked \
    test -n "${CHRONO_BUILD_SHA}" && \
    CMA_JOURNEY_BUILD_SHA="${CHRONO_BUILD_SHA}" cargo build --release --locked --bin cma-journey-probe && \
    mkdir /out && cp target/release/cma-journey-probe /out/

FROM alpine:3.22 AS journey-probe
ARG CHRONO_BUILD_SHA
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD_SHA}"
RUN apk upgrade --no-cache && apk add --no-cache ca-certificates && adduser -D -u 10001 chrono
COPY --from=journey-build /out/cma-journey-probe /usr/local/bin/cma-journey-probe
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/cma-journey-probe"]
