#!/bin/sh
set -eu

# The temporary directory can survive a container restart.
rm -f /tmp/cma-web-draining
