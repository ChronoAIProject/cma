# S2 — EG fixed two, 15 November 08:30 SGT

Use [common setup and safety rules](deployment-runbooks.md). Require signed S1
PASS, AM2/PDB healthy, final EG image and schema/retained-record compatibility,
K09–K15 including secure early owned drain, local provider/recovery gates,
survivor capacity, no HPA/concurrent rollout and qualified <120s cutover.
Name all enabled provider canaries, reconciliation owner and24-hour monitor
before GO. Sleep30 alone does not close K13; never use the withdrawn EG exec
hook or invent `--replica-drain`.

The six commands below each require fresh plan/server dry-run → exact single-use
grant → apply/readback, as described in common setup. The image variables are
read from completed `release.json` as in S1. Verify AM2 before and after the EG
cutover. Keep canonical Service/Gateway/HTTPRoute/NEG identities unchanged.

```bash
python3 -B "$scale_tools/observe.py" check --service am --count 2 --image "$scale_am_image"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_image"
```

1. Pause legacyEG1; verify original one remains serving. Recovery before old
stop is branch A below.

```bash
plan_scale s2-pause s2-pause
apply_scale s2-pause "${s2_pause_approval:?exact pause grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 \
  --image "$scale_eg_image" --paused-template
```

2. Prepare inactive modeON template: actual image, Pod-IP origin8090, native
preStop sleep30, grace760, readiness/topology. Preserve guaranteed2Gi and all
unrelated env/resources. Wait observed generation and original one; branch A
still applies. The final image must implement the separately qualified secure
early drain without reintroducing a shell hook.

```bash
plan_scale s2-prepare s2-prepare
apply_scale s2-prepare "${s2_prepare_approval:?exact inactive-template grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 \
  --image "$scale_eg_image" --paused-template
```

3. Start the independently observed outage interval at actual old backend
withdrawal. Stop legacy1→0, then prove old bot sockets, partition/effect writers
and all legacy authority ended or are safely fenced. Pod disappearance alone
is insufficient. Post-stop failure uses branch C.

```bash
plan_scale s2-stop s2-stop
apply_scale s2-stop "${s2_stop_approval:?exact stop and outage grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 0 --image "$scale_eg_image"
```

4. After actual zero/authority cessation, unpause0; verify controller-created
candidateRS0, all olderRS0 and observed generation. No hand-created RS.

```bash
plan_scale s2-unpause s2-unpause
apply_scale s2-unpause "${s2_unpause_approval:?exact unpause-zero grant}"
```

5. Start0→2. Actual successful user service must restore strictly below120s.
Record both-ready separately; prove each pod serves through canonical backends,
distinct hosts, appropriate ownership and reconnect/provider budgets. Failed
start uses branch C; only healthy two with survivor capacity may use branch B.

```bash
plan_scale s2-start s2-start
apply_scale s2-start "${s2_start_approval:?exact start-two grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 2 --image "$scale_eg_image"
python3 -B "$scale_tools/observe.py" check --service am --count 2 --image "$scale_am_image"
```

6. Install EG PDB minAvailable1/AlwaysAllow with exact EG selector. If an
identical approved PDB exists, retain it after readback; differing spec is HOLD.
Retain both services' PDBs through recovery.

```bash
plan_scale s2-pdb s2-pdb
apply_scale s2-pdb "${s2_pdb_approval:?exact PDB-create grant}"
k get pdb cma-event-gateway -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
```

Declare S/E only after bothEG owners/backends/membership stabilize and prior
terminations finish. Run the common30-minute soak and per-route acceptance.
For every enabled provider reconcile accepted=terminal+reconciled+unresolved.
Unknown/Partial remains HOLD; never resend blindly. No missing/truncated/duplicate
reply, receipt/custody loss or stale commit may pass. K10 queue p95≤2s/max≤10s
and≥99.9% receipt/reply still apply. Reconciled failure remains failure. Record
24-hour monitor owner, scope, start/end and escalation. Lead signs S2 before S3.

## Recovery A — original EG one, before stop

Omit `--prepare-plan` if preparation never applied. The controller refuses a
changed/replaced original process, post-stop state or restoration of an EG
exec hook. A withdrawn original template needs a separately approved safe
replacement, not an override of this rejection.

```bash
plan_scale s2-abort-before-stop s2-abort-before-stop \
  --pause-plan "$scale_work/s2-pause.plan.json" \
  --prepare-plan "$scale_work/s2-prepare.plan.json"
apply_scale s2-abort-before-stop "${s2_abort_approval:?exact pre-stop abort grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_image"
```

## Recovery B — healthy modeON two to one

Refresh `survivor_capacity.eg`, ≤120s old and minimum safe count≤1. Both owners
must be healthy with no prior termination. Keep modeON, compatible image,
sealed-record floors and PDB; AM remains2. Wait reconnect/actual process/socket
exit and provider reconciliation, then fresh30-minute recovery soak.

```bash
plan_scale s2-one s2-one
apply_scale s2-one "${s2_one_approval:?exact healthy-one recovery grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_image"
python3 -B "$scale_tools/observe.py" check --service am --count 2 --image "$scale_am_image"
```

## Recovery C — failed start/unhealthy, compatible all-stop restart

This branch needs separately approved outage/recovery authority before stop.
Forward fix is the default. An older image is usable only with explicit exact
retained-writer/record compatibility qualification. Quiesce0, prove actual
zero plus bot/socket/effect authority cessation, install the qualified recovery
image while paused0, unpause0, verify controllerRS0, restart compatible modeON1.
Unsafe takeover stays quiesced; never clear a claim or reset a record floor.

```bash
plan_scale s2-quiesce s2-quiesce
apply_scale s2-quiesce "${s2_quiesce_approval:?exact emergency all-stop grant}"
python3 -B "$scale_tools/observe.py" check --service eg --count 0 --image "$scale_eg_image"
plan_scale s2-recovery-image s2-recovery-image
apply_scale s2-recovery-image "${s2_recovery_image_approval:?qualified compatible image grant}"
plan_scale s2-unpause s2-recovery-unpause
apply_scale s2-recovery-unpause "${s2_recovery_unpause_approval:?exact recovery unpause grant}"
# Wait actual controller-owned compatible RS0; next plan enforces it.
plan_scale s2-recover-one s2-recover-one
apply_scale s2-recover-one "${s2_recover_one_approval:?exact compatible restart grant}"
scale_eg_recovery_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["recovery"]["eg"]["image"])' "$scale_release")
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_recovery_image"
```

Recheck actual canonical service, custody/provider reconciliation and AM2;
start fresh30-minute recovery soak. Report any120s breach and unresolved
outcomes. If a recovery write is uncertain, stop and reconcile before planning
another; do not apply a generic deployment undo or bulk delete.
