# CMA Image Manager API

Local implementation, 2026-10-12; cloud execution and deployment remain to be verified. Independent service, root `/v1`, NyxID resource
`cma-image-manager`. No Agent or Sandbox database access.

## Resources and operations

An image is an owned catalog entry; a version is an immutable digest and build
provenance; a build is a durable asynchronous operation. Registry credentials are
deployment secrets. Registry:
`asia-northeast1-docker.pkg.dev/chronoai-501608/chronoai-registry`.

| Method | Path | Purpose |
|---|---|---|
| POST / GET | `/v1/images` | Create/list accessible catalog entries |
| GET / PATCH / DELETE | `/v1/images/{id}` | Read/edit/retire an image |
| POST / GET | `/v1/images/{id}/builds` | Submit/list builds |
| GET | `/v1/builds/{id}` | Build status and result |
| GET | `/v1/builds/{id}/logs` | Bounded build logs |
| GET | `/v1/images/{id}/versions` | List immutable ready versions |
| GET | `/v1/images/{id}/versions/{version}` | Digest and provenance |
| PUT / DELETE | `/v1/images/{id}/shares/{subject}` | Grant/revoke use |
| POST | `/v1/images/{id}/resolve` | Authorize and resolve a version |

Create accepts `{name, description}`. Builds identify approved source, immutable
commit, Dockerfile and context paths confined to the source root. Registry
destination and build identity are service configuration, not user parameters.
Cloud Build jobs execute outside the API process; a recoverable dispatcher uses durable submission identity.
Repeated idempotency keys return the original operation; unknown submission is
reconciled before retry. Success requires verified digest/provenance for the
expected destination. Only successful versions resolve to `@sha256:...`.

Sharing grants use, not build/edit/share/delete rights or direct Registry IAM.
Every request checks service permission plus resource ownership/share. Resolve authenticates the actual caller; an owner string is insufficient. Per-Agent image selection in Agent Manager is a separate API extension. Retiring blocks new use, retaining blobs and
metadata for running Agents/recovery. Physical deletion requires verified absence
of retained references, failing closed if dependencies cannot be checked.

Permissions: `cma-im:images:read`, `cma-im:images:create`,
`cma-im:images:update`, `cma-im:images:delete`, `cma-im:images:share`,
`cma-im:images:use`, `cma-im:builds:create`, `cma-im:builds:read`, `cma-im:admin`.
Each service has its own viewer/user/admin roles. Runtime/migrator identities are
separate. Error responses and build logs must not expose service credentials.


The current build input is `{repository,commit,dockerfile,context}`: a public
GitHub HTTPS URL and full lowercase commit SHA, with optional Dockerfile/context
(default `Dockerfile` and `.`). Private repository credentials and source uploads
are not accepted in this version. The owner supplies `Idempotency-Key` on create
and build; edits, retirement and share changes supply the numeric current revision
in `If-Match`. `GET /v1/permissions` returns this service's role catalog and current
permissions. All list responses are `{items,next_cursor}` with optional `after`.

Cloud Build uses a separately configured Google build identity and pinned Docker
builder image. No live build is asserted by local protocol tests. Configure IAM,
builder digest and registry access before activation. Build logs are bounded to
200 entries and include a `truncated` flag. Retirement is logical: this release
does not delete registry blobs or claim to implement cross-service reference GC.
