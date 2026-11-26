import { execFileSync } from 'node:child_process';
import type { publishCurrentTrigger } from './publisher-current';
import { publicationProof } from './journey-proof';

export function auditSyntheticPublications(
  publications: Awaited<ReturnType<typeof publishCurrentTrigger>>[],
) {
  const run = 'synthetic-compiled-publishers';
  const visitor = 'synthetic-visitor';
  const journal: Record<string, unknown>[] = [];
  const proofs = publications.map((published, index) => {
    const { document, revision, placement } = published;
    const launch = {
      id: `launch-${index}`,
      public_id: document.public_id!,
      trigger_id: document.id,
      revision_digest: revision.digest,
      placement_id: placement.id,
      state: 'ready',
      revision: revision.revision,
      event_head: 1,
      first_message: '',
      resolution: {
        sandbox: { state: 'created', id: `sandbox-${index}` },
        workspace: { state: 'created', id: `workspace-${index}` },
        agent: { state: 'created', id: `agent-${index}` },
      },
    };
    for (const [kind, id, subject] of [
      ['triggers', document.id, published.subject],
      ['launches', launch.id, visitor],
      ['sandboxes', launch.resolution.sandbox.id, visitor],
      ['agents', launch.resolution.agent.id, visitor],
    ])
      journal.push({ run, action: 'created', kind, id, subject });
    const stats = (count: number) => ({
      totals: { launches_total: count, agents_ready_total: count },
      placements: [
        { placement_id: placement.id, launches_total: count, agents_ready_total: count },
      ],
    });
    return publicationProof(
      published,
      launch,
      { owner: visitor },
      [
        {
          method: 'POST',
          path: `/api/v2/triggers/public/${document.public_id}/launches`,
          bodyHash: 'a'.repeat(64),
          key: `synthetic-key-${index}`,
        },
      ],
      stats(0),
      stats(1),
    );
  });
  // Only publication comes from the compiled app; launch/count facts are synthetic.
  const proof = {
    schema: 'cma-public-journey-v1',
    execution: 'configured_remote_origin',
    scope: 'current_image_inline',
    run,
    visitor,
    publications: proofs,
  };
  const auditor = new URL('../../.github/scripts/public-journey-evidence.mjs', import.meta.url);
  execFileSync(
    process.execPath,
    [
      '--input-type=module',
      '-e',
      `
    import { readFileSync } from 'node:fs';
    import { currentJourneyCases, validatePublicJourneyProof } from ${JSON.stringify(auditor.href)};
    const { proof, journal } = JSON.parse(readFileSync(0, 'utf8'));
    validatePublicJourneyProof(currentJourneyCases.j5, proof, journal);
  `,
    ],
    { input: JSON.stringify({ proof, journal }), timeout: 10000 },
  );
}
