# Triggers

<!-- nav: 8 Triggers -->

A Trigger brings a visitor to an Agent of their own. The publisher selects exact
public Sandbox, Workspace and Agent Profile revisions and the reuse policy.
`triggers` and `triggerAuthoring` reference the same owner-facing helper.

Saving a draft does not alter a published Trigger. Publication uses the observed
`draft_version`; keep the command key and body stable until the outcome is known.
Placements have stable ids and separately validated HTTPS source URLs. The
owner's `embed` descriptor supplies Markdown, HTML, JavaScript and React snippets.

```tsx
import { CmaTriggerButton } from '@chronoai/cma-sdk/react';

export function ResearchButton() {
  return (
    <CmaTriggerButton
      triggerId="public-trigger-id"
      placementId="placement-id"
      label="open_in_cma"
      theme="light"
      baseUrl="https://cma.example.com"
    />
  );
}
```

The React entry renders an anchor and badge image with no fetch, effects or DOM
dependency, so it supports server rendering. It requires the optional React peer.
`triggerId` is the published public id, not the private authoring document id.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const preview = await cma.launches.preview('public-trigger-id');
console.log(preview.title, preview.availability);
```

Preview suppresses configured bearer credentials and cookies. Previewing,
navigation, badge fetches and an OAuth callback never admit work. Only an
explicit visitor action should call `createOrResume` with a retained command key.
When review is required, show the returned trust disclosures and connection
requirements before calling `start` with the reviewed digest/acknowledgements.

`repeatChoiceFrom(error)` extracts the existing Agent from the named repeat-choice
problem. Present that choice to the visitor; do not silently create or resume.
Service connections use `createConnection` and `readConnection` without exposing
credentials. Launch `read`, `requirements`, `events`, `stream` and filtered `list`
provide recorded progress. Cancellation does not prove in-flight resource work
has finished; do not infer safe cleanup from a successful cancel alone.
