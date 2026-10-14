# Live Agent skill installation

The Agent details Skills section uses the existing authenticated
`GET /api/v1/profile-authoring/skills` Ornn catalog. Catalog access is owner scoped;
the installation worker independently resolves the selected skill with the
Sandbox's delivered, owner-bound Ornn credential.

`GET /api/v1/agents/{id}/skills` returns:

```json
{
  "items": [{
    "id": "ornn-skill-guid",
    "name": "Research",
    "version": "1.0",
    "source": "ornn",
    "status": "installed"
  }],
  "pending": false,
  "can_install": true
}
```

`name`, `error`, and the top-level `unavailable_reason` are optional. Status is
`pending`, `installed`, or `failed`. `can_install` requires an idle, ready Agent
and a read-only observation of the exact bound runtime advertising
`agent_skill_install: true`. Missing, unreachable, or older runtime support
fails closed. Pending installation disables additional installs and conversation
input until the operation settles.

`POST /api/v1/agents/{id}/skills` accepts `{"id":"ornn-skill-guid","version":"1.0"}`
and requires `Idempotency-Key`. The HTTP 202 response has the same view shape;
clients poll GET for runtime confirmation. Repeating the key and exact body
replays the original response. A different body with that key is rejected.
Busy Agents, unsupported runtimes, and replacing an installed skill ID with a
different version are refused. Installation is additive and capped at 32 total
profile plus live skills. Failed rows are retained as evidence; this endpoint
does not remove or replace an installed skill or reset a failed runtime mutation.

Admission persists separate live-skill work inside the Agent aggregate, leaving
the immutable profile and initial material untouched. The existing worker lease
serializes work. Before runtime mutation, the worker resolves and durably pins
source, ID, exact version, name, and archive SHA-256. Lost responses replay that
same content identity without another catalog lookup. Runtime-confirmed receipt
publication marks the skill installed. Resolution failure marks only the skill
failed and leaves the Agent available.

The supervisor's fenced `install_agent_skill` operation installs one archive
into an operation-specific staging directory, using the existing checksum and
safe ZIP extraction checks. It publishes with atomic no-replace rename, retaining
all other skills, baseline links, settings, credentials, and Agent home files.
A durable per-skill receipt recovers a crash after publication but before journal
completion. Existing destinations and symlinked parents are refused. Skill
mutation authority is separate per skill; Agent-home removal permanently dominates
late installation/replay. Stop cancels pending confirmation and follows normal
lease-fenced cleanup. A retained-home revival reconnects the conversation and
reschedules a pending installation, which settles before revived message delivery
and final Active publication.

## Rollout and verification

Deploy the new Agent Manager first, then publish/select the new runtime image for
new Agents. The runtime status response uses an additive capability field; older
Agent Managers reject unknown status fields, so the runtime must not precede its
compatible Agent Manager. Retained runtimes remain intact and report installation
unavailable through the new UI. Do not replace a retained user's process or home
solely to enable this feature.

The runtime image build runs `live_skill_install.test.cjs` with pinned Codex
0.147.0 and the actual patched ACP bridge. The test uses a loopback Responses
fixture, keeps the same process and conversation, atomically publishes a new
skill, and verifies the next model request includes its unique skill name and
description. No provider credentials or model API calls are required. Rust tests
cover owner/idle admission, idempotency, durable material replay after lost
responses, retained conversation reattachment, filesystem preservation,
checksum failure, no-replace publication, parent symlink refusal, and home-cleanup
authority. Production installation still requires a smoke test against a new
runtime image after rollout.
