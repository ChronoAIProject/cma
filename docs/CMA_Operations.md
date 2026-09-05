# CMA Operations

This runbook covers deployment-owned operations for the refactored CMA
control plane. User-facing flows must not perform any operation in this file.

## NyxID visitor registration

CMA uses two independently configured NyxID origins:

- `CHRONO_NYXID_BASE_URL` is the API and OAuth discovery origin used by the
  control-plane process.
- `CHRONO_NYXID_PUBLIC_URL` is the browser-facing registration origin. In
  provider mode it is required and must be one exact canonical HTTPS origin.
  Production uses `https://nyx.chrono-ai.fun`.

After deployment, verify that `GET /auth/signup-options` returns only the four
documented registration fields. A closed response with
`register_available: false` means CMA could not obtain usable NyxID public
configuration; inspect CMA warnings and NyxID health before advertising
registration.

## Default visitor role

Creating or changing a NyxID role is an operator-only action. CMA never calls
the role administration endpoint. Apply the role in staging first, verify its
exact stored grants and default status, and then repeat the same reviewed
operation in production.

The required request is:

```http
POST https://nyx-api.chrono-ai.fun/api/v1/admin/roles
Authorization: Bearer <operator-token>
Content-Type: application/json

{
  "name": "CMA visitor",
  "slug": "cma-visitor",
  "description": "Can start CMA agents from trigger buttons and use their own sandboxes",
  "permissions": [
    "cma:trigger:launch",
    "cma:sandbox:use",
    "cma:sandbox:delete"
  ],
  "is_default": true
}
```

Do not add `cma:sandbox:create`, profile-management, trigger-publishing, or
administrator permissions. A visitor is derived from capabilities, not the
role slug: it is a caller with `LaunchTriggers` and without `CreateSandbox`.
This keeps role names and provider policy out of CMA authorization decisions.

Visitor Launch charging occurs only after idempotency resolution proves that
a new Launch was created. An exact replay and a later reviewed Start do not
consume another daily unit. Exhaustion returns `resource_quota` with the next
UTC reset time. Sandbox-count enforcement belongs to the transactional sandbox
creation path so concurrent launches cannot both pass the one-sandbox limit.

The current shared limiter is process-local and production is pinned to one API
replica. Do not scale the API horizontally until visitor counters use a durable
atomic ledger; otherwise each replica grants an independent daily allowance.

## Agent log capacity

CMA validates an agent's complete durable log before accepting a mutation. To
keep that validation within a bounded MongoDB snapshot, normal log writes stop
at either 10,000 entries or 64 MiB of exact encoded BSON, whichever is reached
first. The permanent store error is `agent_log_capacity_exceeded`. Existing log
pages remain readable, and the owner can still stop and delete the agent; start
a new agent for further conversation.

Lifecycle publication has two internal reserves that normal conversation and
title writes cannot consume. A stop or startup-failure transition may use at
most another 64 entries or 512 KiB, and deletion may use a final 64 entries or
512 KiB. These staged ceilings ensure a full stop reserve cannot strand the
required deletion record. Each atomic append is separately limited to 64 log
entries so one transaction cannot materialise an unbounded command batch.

`retention.agent_log_days` is the maximum time logs are retained after their
agent becomes terminal; it does not increase these per-agent capacity limits.
Never delete individual `agent_log` rows to reclaim capacity. A missing row or
an entry above the aggregate high-water mark is treated as durable corruption.

After an ambiguous Mongo commit, CMA reconciles the complete authority snapshot
at the commit session's operation time. The request fails closed as unavailable
if reconciliation has not completed within 30 seconds. Because cancelling an
in-flight Mongo driver future is unsafe, that read-only check finishes in the
background; at most 16 such checks may remain pending per API process.
