# syntax=docker/dockerfile:1
# Bot-pod image: the sandboxed agent runtime.
#
# This image is never scheduled by the control plane directly. It is handed to
# the sandbox provider as the image for a sandbox, with the supervisor as its
# entrypoint; the provider isolates it and exposes its port back to us.
#
# It therefore carries two things the control plane deliberately does not: the
# agent kernel itself, and a real pseudo-terminal to run it under.
FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
RUN cargo build --release --locked --bin layer4-runtime

FROM alpine:3.22 AS runtime
# ncurses-terminfo is required, not cosmetic: a full-screen agent interface
# cannot address the cursor without a terminfo entry for TERM, and degrades to
# an unreadable flat scroll.
RUN apk upgrade --no-cache \
    && apk add --no-cache \
        bash \
        ca-certificates \
        coreutils \
        git \
        ncurses \
        ncurses-terminfo \
        nodejs \
        npm \
    && npm install -g @openai/codex@0.147.0 \
    && npm cache clean --force \
    && adduser -D -u 10001 chrono
COPY --from=build /build/target/release/layer4-runtime /usr/local/bin/layer4-runtime
USER 10001:10001
ENV HOME=/home/chrono \
    CHRONO_RUNTIME_BIND=0.0.0.0:9090 \
    CHRONO_RUNTIME_COMMAND=codex \
    TERM=xterm-256color
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]
