# Sourced by Backend's ordinary Bash step shell, without wrapping the gate.
# Descendant shells inherit the marker; each new Actions step starts afresh.
if [[ "${CMA_RESOURCE_SAMPLER_STARTED:-}" != 1 ]]; then
  export CMA_RESOURCE_SAMPLER_STARTED=1
  # No wait, traps, shell-option changes, or status propagation from telemetry.
  # Missing Python / a failed sampler must leave the actual command untouched.
  python3 "${BASH_SOURCE[0]%/*}/backend-resource-sampler.py" "$$" </dev/null 2>/dev/null &
fi
:
