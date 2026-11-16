# ADM-077 private platform spend reads

The case distinguishes enabled metering with a known observation, disabled
metering, a failed accounting dependency, and enabled metering whose cloud
observation is missing. The actual public settings determine whether metering
is enabled: budgets must be enabled and the platform must configure a cloud
source. Removing that source disables platform metering even when other budget
controls remain enabled. The historical fixture key `enabled_without_cloud`
means a configured source with no observation. It must not mean missing cloud
configuration, which belongs to the disabled case.

The parent supplies real bounded `selectMode(name)` and `restoreMode()` methods
under its exclusive settings lease. `restoreMode` captures the actual initial
configuration when setup acquires custody. Mode-method return values never
decide PASS. The case independently captures initial settings and spend from
both AMs, then validates all selected modes through public APIs. The parent
retains mode-change evidence and owns the sole application's topology. A fake
response switch or an unbound factory is insufficient.

`expectedEnabled`, `expectedDisabled` and `expectedWithoutCloud` contain exact
prepared accounting inputs. Enabled data includes a real observation and a
positive committed value. The parent must choose real projection inputs and a
bounded case interval for which that expected value remains exact; a cloud
policy with zero run rate or zero maximum age is invalid. Each selected healthy
mode compares the complete response against these concrete expectations. The
case also derives the warning, stop and disposable levels independently from
the actual public policy using exact integer arithmetic, and joins the settings
revision and ledger period to that policy. Expected data cannot be a function
that returns the observed case result as an oracle.

Disabled metering omits levels, committed and ledger entirely. A missing cloud
observation has an actual ledger but no cloud entry or committed value; neither
null nor an invented zero is accepted. The dependency-unavailable mode targets
the actual accounting reader while preserving the installed service and
authentication path. Both AMs must return exactly HTTP 503 with
`platform_spend_unavailable`. This differs from a valid disabled HTTP 200 view.

Every mode also performs a named non-operator GET through each AM, requiring
HTTP 403 / `forbidden`. Refusals and unavailable responses must omit private
accounting fields even when those fields contain null, false or zero. Healthy
mode reads repeat after these denied requests to prove unchanged accounting.
All raw public responses remain in private observations, including levels,
committed values, ledger, timing, request IDs, both serving snapshots and
attribution. The shared request runner verifies actual AM ingress.

Mode intent is recorded before selection. One memoized finalizer restores the
parent's initial configuration and independently reads settings and spend from
both AMs, even when restoration or another read fails. A lost acknowledgement
stays failed without replaying selection or restoration. The original policy,
writer flags and ledger must remain intact; a new settings revision is allowed
when genuine restoration required a public configuration change, and that
revision must agree across both AMs and spend views. The time-projected
`committed` field may differ during restoration, while all durable accounting
must match the captured original. Parent cleanup still runs after contributor
failure and owns immutable records and database disposal.

The healthy sequence is bounded to 42 public GETs, four mode selections and one
restoration. It issues no public mutation directly. Six automatic recipes / 31
parent-required factories remain unchanged. Offline models verify only the
harness; actual feature acceptance still requires the sole parent's authentic
fixtures, three steady repetitions, complete serving evidence and disposal.
