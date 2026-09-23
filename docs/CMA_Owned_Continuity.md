# Owned Sandbox continuity

Control-plane deployment must preserve every retained Sandbox runtime and Agent
conversation. The runtime image default affects new allocations only. See
[CMA operations](CMA_Operations.md#delivery-authority) for deployment authority
and [CMAEG operations](CMAEG_Operations.md) for the separate Event Gateway.

Before deployment, record the cluster context, namespace, deployment identity,
API and web image digests, all live Sandbox runtime Pod UIDs, their image pins,
restart counts, and the Sandbox/Agent identifiers used for an authorized test.
Read current Agent status, conversation identity and durable sequence through
the public CMA API. An observation does not authorize stopping a user's work.

Roll out only the reviewed control-plane component images, preserving the
configured runtime default unless a separately reviewed change requires it.
Wait for readiness and verify the deployed digest and build identity. Compare
the recorded live runtime cohort, Sandbox hierarchy, Agent status, conversation
identity and previously observed durable history after rollout.

For an explicitly authorized test Bot/chat, use the CMA Bot operation and public
CMAEG ingress contract to demonstrate a continued turn through its existing
binding and conversation. Retain delivery receipts and verify full event context
can be read after gateway replacement. Do not send a test message to an
unapproved destination or create duplicate input to resolve an uncertain receipt.

The retired in-CMA Bridge fixture harness is no longer an executable rollout
procedure. Its Mongo allocation, callback, outbox and original-human credential
lanes have been removed. Existing history seals and provenance remain readable;
see [retired bridge compatibility](CMA_Operations.md#retired-bridge-compatibility).
