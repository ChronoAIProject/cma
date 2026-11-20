#!/bin/sh
# The sandbox user's tools use its own trust. A privileged caller retains
# the distro's independent system configuration and never trusts user state.
case "${0##*/}" in
    pacman|pacman-key|pacman-conf) tool="/usr/bin/${0##*/}" ;;
    *) exit 2 ;;
esac
if [ "$(/usr/bin/id -u)" = 10001 ]; then
    exec "$tool" --config /usr/share/cma-omarchy/pacman-user.conf "$@"
fi
exec "$tool" "$@"
