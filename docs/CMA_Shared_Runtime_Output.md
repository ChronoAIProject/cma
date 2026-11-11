# Shared retained runtime output

`CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED` is a strict boolean, default `false` in
code and the API ConfigMap. It changes in-process ownership of retained runtime
domain updates. Both native and retained-provider transports apply this policy
after renderer settings, so renderer preferences cannot override it.

Enabled attachments store each decoded domain effect in immutable shared
ownership. Copies made by startup/reopen and durable persistence retry paths
share that allocation. The durable recorder borrows the input during folding;
it allocates the resulting logs, assemblies and checkpoint state separately.
The flag-disabled representation retains the existing owned payload cloning.
Legacy output without a runtime receipt remains on its existing path.

Original stream position, payload fingerprint, prompt identity and generation
fences remain authoritative. Commit failure retains the original input for an
exact retry. Cloning, dropping or refusing an update does not acknowledge it;
the consumer sends the source ACK only after durable success. The persisted
checkpoint and runtime wire formats do not change.

This is the ownership prerequisite tracked by #1703, Part of #1472 and #1702.
It does not enforce an aggregate byte limit. Codec/decode scratch, prepared
startup effects, permission replay maps, assemblies and independent public
serialization still need the aggregate accounting described in #1702.

Release owners enable the flag through a reviewed API rollout. Existing
attachments keep their original representation until replaced; qualification
must identify the new attachments and exact source/configuration. Disable and
restart for rollback. No retained journal, checkpoint or source receipt is
deleted or rewritten as part of rollback.
