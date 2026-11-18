#!/bin/sh
# Keep platform tools independent of language and application inputs.
# The production root is read-only; user-space tools install under /home/cma.
# Python/archive tools serve those installers, and repository harnesses need ps/jq.
set -eu
apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
        bash \
        bubblewrap \
        ca-certificates \
        chromium=156.0.8078.79-1~deb13u1 \
        chromium-common=156.0.8078.79-1~deb13u1 \
        chromium-sandbox=156.0.8078.79-1~deb13u1 \
        curl \
        build-essential \
        git \
        gzip \
        jq \
        libdbus-1-3 \
        libgmp-dev \
        libgssapi-krb5-2 \
        libicu76 \
        libssl3t64 \
        libunwind8 \
        ncurses-term \
        procps \
        python3 \
        ripgrep \
        tar \
        unzip \
        xz-utils \
        zlib1g \
        zstd \
    && if [ "${RUNTIME_DESKTOP}" = "1" ]; then apt-get install -y --no-install-recommends chromium dbus-x11 fonts-liberation novnc scrot wmctrl x11vnc x11-xserver-utils x11-utils xdotool xvfb websockify xfce4-session xfce4-panel xfdesktop4 xfwm4 xfce4-settings xfce4-appfinder gvfs thunar xfce4-terminal adwaita-icon-theme librsvg2-common; else test "${RUNTIME_DESKTOP}" = "0"; fi \
    && git --version \
    && ps --version \
    && jq --version \
    && rm -rf /var/lib/apt/lists/* /var/log/apt/* /var/log/dpkg.log /var/log/alternatives.log /var/cache/ldconfig/aux-cache
