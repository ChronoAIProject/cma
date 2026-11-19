#!/bin/sh
set -eu

executable=$1
headers=$(readelf -lW "$executable")
dynamic=$(readelf -dW "$executable")
imports=$(readelf --dyn-syms -W "$executable")
if ! printf '%s\n' "$headers" | grep -qw GNU_RELRO ||
    ! printf '%s\n' "$dynamic" | grep -qw BIND_NOW ||
    ! printf '%s\n' "$dynamic" | grep -Eq '\(FLAGS_1\).*[[:space:]]PIE([[:space:]]|$)' ||
    ! printf '%s\n' "$imports" | grep -Eq 'UND[[:space:]]+__stack_chk_fail@' ||
    ! printf '%s\n' "$imports" | grep -Eq 'UND[[:space:]]+__[[:alnum:]_]+_chk@'; then
    printf 'OpenSSL CLI hardening check failed for %s\n' "$executable" >&2
    exit 1
fi
printf 'OpenSSL CLI retains full RELRO, PIE, stack protection and fortified imports: %s\n' "$executable"
