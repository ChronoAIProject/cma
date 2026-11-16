# EG mutation custody rollout

Issues: #2196 (channel/provider mutations), #2197 (Telegram webhook mutations).
These fixes are for the EG release after E1. No production rollout is part of
this change.

## Telegram custody

A retained webhook lease identifies an unresolved operation. Its 60-second
expiry closes new dispatch by that claimant; it does not prove cancellation of
an accepted Telegram request. Set/delete timeouts, malformed responses and
process loss keep the exact lease, channel pointers and bot claim. Recovery,
cancellation and readback must not clear these obligations or replay the effect.
Only an exact live claimant may settle confirmed completion, a proven local
pre-send refusal, or an authoritative Telegram error envelope. Rate-limit
refusals preserve bounded retry information and permit a later authorized retry.
Generic HTTP errors are insufficient terminal evidence.

The stored lease format is unchanged, but old replicas can still expire and
replace it. Quiesce connection management mutations and recovery workers, finish
or account for existing calls, replace **all** writers and workers with this
reader/custody implementation, then resume. Mixed old/new writers do not provide
this guarantee. Reverting to expiry-based code while any custody remains can
reintroduce stale remote writes; retain the capable image and recover forward.
Do not edit deadlines/nonces or clear pointers as a recovery shortcut. A crashed
unknown effect may require provider-side proof that it can no longer execute;
Telegram readback and elapsed time alone do not provide that proof.

## Validation

Local barrier tests cover retained custody after cancelled activation and failed
delete, exact late completion, terminal rate-limit refusals and local pre-send
validation failures. The ignored `connections::platform_tests::native_tests`
module requires an owned authenticated disposable Mongo replica set via
`CMA_IM_TEST_MONGODB_URI`. It uses independently authenticated runtime clients,
real CAS writes and a loopback Telegram receiver whose accepted operations
survive HTTP timeouts. Tests cover delayed set/delete, lease expiry, cancellation,
bot claim retention, pre-dispatch expiry and known-unsent read failures. No test
calls Telegram or reads production state.
