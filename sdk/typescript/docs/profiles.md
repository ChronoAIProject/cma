# Profiles

<!-- nav: 6 Profiles -->

`agentProfiles`, `workspaceProfiles` and `sandboxProfiles` share the draft and
revision workflow with distinct typed content. Drafts are editable; published
revisions are immutable. Instances and nested profile references pin a revision.

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
