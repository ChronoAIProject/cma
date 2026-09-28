# Profiles

<!-- nav: 6 Profiles -->

`agentProfiles`, `workspaceProfiles` and `sandboxProfiles` share the draft and
revision workflow with distinct typed content. Drafts are editable; published
revisions are immutable. Instances and nested profile references pin a revision.

Agent content can set `isolated_agent: true` to request separate process, file
and network boundaries. Omission or `false` retains the existing default. The
SDK preserves the selected value when reading or saving a profile, including
when that configuration appears in a Bot Father setup review.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function publishAgentProfile(profileId: string, commandKey: string) {
  const document = await cma.agentProfiles.read(profileId);
  return cma.agentProfiles.publish(
    profileId,
    { draft_version: document.draft_version },
    { idempotencyKey: commandKey },
  );
}
```

`save` and `publish` use the observed `draft_version` for compare-and-swap. A
conflict requires rereading and reviewing the current draft. Saving does not
publish, and publishing does not modify previously provisioned environments.

`setVisibility` controls Marketplace eligibility. Public Sandbox Profile values
are secret-scanned at publication and when retained revisions become public.
A rejection identifies variable names only. Never copy rejected values into
logs; scanner unavailability is a retryable service failure, not permission to
publish unchecked content.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function inspectConfiguredDefaults() {
  const defaults = await cma.readDefaults();
  return cma.sandboxProfiles.readRevision(
    defaults.sandbox_profile.id,
    defaults.sandbox_profile.revision,
  );
}
```

`marketplace.agent`, `.workspace` and `.sandbox` provide `list`, `read` and `fork`.
Entry reads include immutable content and resolved nested revisions. Sandbox
entries include the trust digest for the reviewed commands. Fork an explicit
revision; do not replace the selected revision with the latest during a retry.
Retiring a profile prevents future selection without rewriting pinned instances.

`profileAuthoring` supplies deployment capacity bounds, safe service scope names,
bounded skill search, and public GitHub repository verification. All four
operations require the profile-management capability and accept ordinary
`OperationOptions`, including cancellation, without a mutation idempotency key.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function inspectProfileAuthoring(url: string, signal: AbortSignal) {
  const bounds = await cma.profileAuthoring.bounds({ signal });
  const services = await cma.profileAuthoring.services({ signal });
  const skills = await cma.profileAuthoring.searchSkills({ q: 'review', limit: 20 }, { signal });
  const repository = await cma.profileAuthoring.resolveRepository({ url }, { signal });
  return { bounds, services, skills, repository };
}
```

Skill search accepts at most 200 UTF-8 bytes and a limit from 1 through 50
(default 20). Pass `next` back as the opaque `cursor`; cursors are nonempty and
bounded at 2048 UTF-8 bytes, and neither search text nor cursors may contain
controls. Versions are canonical literal `major.minor` strings in newest-first
order. Keep those exact strings, including distinctions such as `2.10` and `2.1`.
Catalog descriptions allow up to 6144 UTF-8 bytes; profile draft text limits are unchanged.
An empty filtered page may still contain `next`; continue using that opaque cursor.
Only an absent `next` marks the end of the catalog.

Repository resolution is a read-only POST: it neither clones nor admits work.
Use its authoritative canonical URL and lowercase 40-digit commit identity.
Omitting `reference` retains default-branch semantics; do not replace that omission
with the returned branch or commit implicitly. Public repository validation and
dependency failures remain typed API problems for an explicit user decision.
