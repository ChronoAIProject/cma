"""Application and runtime members admitted to a coordinated release."""

APP_TARGETS = frozenset({"am", "eg", "web"})
RUNTIME_TARGETS = frozenset({"runtime", "runtime-omarchy"})
TARGETS = APP_TARGETS | RUNTIME_TARGETS
