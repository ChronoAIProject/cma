#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
umask 077
if [[ ! -f private/tls.key ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 30 \
    -keyout private/tls.key -out private/tls.crt -subj '/CN=cma-scale.test' \
    -addext 'subjectAltName=DNS:localhost,DNS:cma-scale.test,DNS:am.cma-scale.test,DNS:eg.cma-scale.test,DNS:cma-scale-control-plane,IP:127.0.0.1' \
    > evidence/tls-generation.log 2>&1
fi
chmod 644 private/tls.crt
scripts/k create secret tls routing-tls --cert private/tls.crt --key private/tls.key --dry-run=client -o json | scripts/k apply -f -
