# syntax=docker/dockerfile:1
FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
RUN cargo build --release --locked --bin chrono-bot

FROM alpine:3.22 AS runtime
# Pull security fixes published after the base image was cut. ca-certificates is
# required, not optional: the control plane makes outbound HTTPS calls to the
# identity provider, and an empty trust store fails every one of them.
RUN apk upgrade --no-cache \
    && apk add --no-cache ca-certificates \
    && adduser -D -u 10001 chrono
USER 10001:10001
COPY --from=build /build/target/release/chrono-bot /usr/local/bin/chrono-bot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/chrono-bot"]
# Health is asserted by the Kubernetes probes in infra/api/deployment.yaml.
