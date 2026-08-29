# CMA Trigger smoke environment operator

This tool manages only the protected `ChronoAIProject/cma` GitHub environment
`cma-trigger-smoke`. Repository, environment, workflow, reviewer, account, and
NyxID catalog identities are compiled into the tool. It cannot target a
production environment or another repository.

The provisioning bearer is read from exactly one of
`CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN` or
`CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE`. The file form must be an
absolute, owner-only `0600` file outside the repository. GitHub REST always
passes through the canonical NyxID GitHub catalog ID and the explicit
staging-administration UserService in the config. The upstream GitHub OAuth
token is never returned to or handled by this tool.

The bootstrap state, handoff, custody directory, and plan output are also
absolute owner-only paths outside the repository. Directories must be `0700`;
state and handoff files must be regular, singly linked `0600` files. Secret
paths may not traverse symbolic-link ancestors, and the plan output may not
have an untrusted group/world-writable ancestor. Root- or current-user-owned
sticky boundaries are the only writable-ancestor exception. The plan output may
not alias the bootstrap state or its adjacent caller-authority lock. Lexical
aliases are checked before lock acquisition; real-path and device/inode identity
are checked again after the lock exists, covering filesystem-specific Unicode
and case folding. Every stateful remote operator mode shares that lock with the
bootstrap; `--self-test` and `--validate-config` do not read state. A stale
same-host lock is reported for reviewed exact-file removal and is never
reclaimed automatically.

An unresolved bootstrap `caller_exchange` or `resource_creation` marker fences
environment setup and dispatch. Read-only planning and exact
ephemeral-handoff cleanup remain available under the shared lock so a bootstrap
incident cannot strand long-lived secrets inside a crash-left handoff. Recover
only through the marker-specific bootstrap procedure in
`docs/CMA_Trigger_Operations.md`; the environment operator cannot clear, modify,
or bypass either marker for any authority-producing or authority-consuming
action. A non-null `resource_creation.recovery_rotation` means one exact
rotation is already in flight or ambiguous and a second automatic rotation is
forbidden.

## Plan

Validate the checked-in staging contract, then produce a ten-minute live plan:

```bash
node .github/tools/cma-trigger-smoke-environment/operator.mjs \
  --validate-config \
  .github/tools/cma-trigger-smoke-environment/config.example.json

node .github/tools/cma-trigger-smoke-environment/operator.mjs \
  --plan .github/tools/cma-trigger-smoke-environment/config.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json \
  --output /absolute/private/cma-trigger-smoke/environment-plan.json
```

Review the target identities, protections, inventory, authority provenance,
branch head, planned writes, and conflict list in the plan. Every mutating
command repeats the live preflight and requires the exact unexpired plan.

## Setup

Setup preprovisions one Kubernetes CA and a namespace-local observer client,
stores their custody under `kind-authority-v1`, and uploads only the stable Kind
secrets and static variables. The Rust helper locally encrypts plaintext with
the GitHub environment public key; plaintext enters the helper only on stdin.

```bash
export CMA_TRIGGER_SMOKE_ENV_CONFIRM=setup-reviewed-cma-trigger-staging
node .github/tools/cma-trigger-smoke-environment/operator.mjs \
  --setup .github/tools/cma-trigger-smoke-environment/config.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json \
  --approved-plan /absolute/private/cma-trigger-smoke/environment-plan.json \
  --custody /absolute/private/cma-trigger-smoke
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

Run `--plan` again after setup before staging a run.

## Stage Run

The stage-run command proves the handoff against the authoritative bootstrap
state, verifies its signed NyxID JWT, and requires one different personal
GitHub UserService with exactly `public_repo`, `read:user`, and `user:email`.
It uploads the exact handoff bytes as base64 and the state-bound UserService ID,
keeps `develop` pinned to the reviewed plan head through the secret writes,
passes that full commit as the workflow's required `expected_head_sha`,
dispatches once, identifies the exact run, and watches it. It never retries an
unresolved dispatch, and a completed run returns success only when its GitHub
conclusion is exactly `success`.

```bash
export CMA_TRIGGER_SMOKE_ENV_CONFIRM=dispatch-reviewed-cma-trigger-staging
node .github/tools/cma-trigger-smoke-environment/operator.mjs \
  --stage-run .github/tools/cma-trigger-smoke-environment/config.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json \
  --approved-plan /absolute/private/cma-trigger-smoke/environment-plan.json \
  --custody /absolute/private/cma-trigger-smoke \
  --handoff /absolute/private/cma-trigger-smoke/handoff.json
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

Normal exit, failure, `SIGHUP`, `SIGINT`, and `SIGTERM` all enter the same
cleanup path. Only the exact nonce-bound
`CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_<UPPERCASE_DISPATCH_NONCE>` secret is
deleted, and its absence is verified. Stable Kind custody and
`CMA_TRIGGER_SMOKE_SERVICE_ID` are retained. The workflow separately destroys
Kind and purges runner-local credentials. After destruction proves writer
quiescence and before the purge, the workflow separately settles and
absence-verifies only the current run's exact live logical objects in the two
dedicated staging buckets.

If the operator was killed before cleanup completed, produce a fresh plan and
run the bounded cleanup command. It removes no stable resource:

```bash
export CMA_TRIGGER_SMOKE_ENV_CONFIRM=cleanup-reviewed-cma-trigger-staging
node .github/tools/cma-trigger-smoke-environment/operator.mjs \
  --cleanup .github/tools/cma-trigger-smoke-environment/config.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json \
  --approved-plan /absolute/private/cma-trigger-smoke/environment-plan.json
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

## Retention boundary

The checked-in staging CMA hard-codes `CHRONO_TRIGGER_CLEANUP_MODE=dry_run`, so
its in-process retention worker has no provider deletion port. The terminal
staging harness separately destroys Kind, proves every writer is gone, and then
uses the state-bound storage owner to settle only the current run's exact live
logical context/workspace objects. It validates legacy or generation-fenced
delete acknowledgements and re-enumerates both buckets before purging custody.

Do not install a blanket provider lifecycle rule on either bucket. Pinned
`chrono-bucket` source revision
`4087662e2a65e3747b3203ba619eca7342089272` stores fenced live objects and
durable tombstones under reserved mutation-authority keys. Tombstones remain for
the bucket lifetime and must never be expired or rewritten by S3 lifecycle.

Publishable evidence hashes both bucket identities and the storage owner, counts
selected objects and observable bytes, records legacy and fenced deletion
acknowledgements, requires zero unrelated live objects, and proves zero current
run live objects after settlement. Hidden durable tombstones are reported as
retained security authority, not as deleted provider objects. Local Kind
destruction alone is only the no-writer precondition and cannot close this
receipt.

The final local cleanup receipt uses closed schema version 2 and embeds that
redacted settlement receipt. It embeds `null` only for a smoke result that did
not pass and therefore did not authorize provider deletion; passing evidence
cannot finalize from a null settlement.

An ambiguous secret PUT or unresolved workflow dispatch is a recovery-required
condition. Do not automatically replay it. Re-run the read-only plan, inspect
the live inventory or exact run, and explicitly approve the next action.
