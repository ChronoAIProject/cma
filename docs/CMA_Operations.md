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
