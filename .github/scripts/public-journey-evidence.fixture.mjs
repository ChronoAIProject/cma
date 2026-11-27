import { currentJourneyCases } from './public-journey-evidence.mjs';
import { connectionCampaignFixture } from './connection-campaign.fixture.mjs';

// Synthetic receipts exercise the auditor; they are never remote journey evidence.
export function journeyEvidenceFixture() {
  const journal = [];
  const run = 'synthetic-current-journeys';
  const own = (kind, id, subject) => {
    const row = { run, at: new Date().toISOString(), kind, id, subject };
    journal.push({ ...row, action: 'created' });
    if (kind !== 'agent-lifecycle') journal.push({ ...row, action: 'cleanup_complete' });
  };
  const command = (path) => ({ method: 'POST', path, bodyHash: 'a'.repeat(64), keyHash: 'b'.repeat(64) });
  const publication = (suffix, publisher, visitor) => {
    const image = { kind: 'managed_default' };
    const stats = (count) => ({ totals: { launches_total: count, agents_ready_total: count }, placements: [{ placement_id: `placement-${suffix}`, launches_total: count, agents_ready_total: count }] });
    for (const [kind, id, subject] of [
      ['triggers', `trigger-${suffix}`, publisher], ['launches', `launch-${suffix}`, visitor],
      ['sandboxes', `sandbox-${suffix}`, visitor], ['agents', `agent-${suffix}`, visitor],
    ]) own(kind, id, subject);
    return {
      publisher, observedPublisher: publisher, targetKind: 'inline', submittedProfileIds: [],
      submittedTargetHash: suffix === 'second' ? 'd'.repeat(64) : 'e'.repeat(64),
      publishedProfiles: Object.fromEntries(['sandbox', 'workspace', 'agent'].map((kind) => [kind, { id: `generated-${kind}-${suffix}`, revision: 1 }])),
      revisionProfiles: Object.fromEntries(['sandbox', 'workspace', 'agent'].map((kind) => [kind, { id: `generated-${kind}-${suffix}`, revision: 1 }])),
      authoringCommands: [command('/api/v1/triggers'), command(`/api/v1/triggers/trigger-${suffix}/placements`), command(`/api/v1/triggers/trigger-${suffix}/publish`)],
      requestedTargetHash: suffix === 'second' ? 'd'.repeat(64) : 'e'.repeat(64),
      publishedTargetHash: suffix === 'second' ? 'd'.repeat(64) : 'e'.repeat(64),
      revisionTargetHash: suffix === 'second' ? 'd'.repeat(64) : 'e'.repeat(64),
      expectedImage: image, observedImage: structuredClone(image), triggerId: `trigger-${suffix}`,
      publicId: `public-${suffix}`, placementId: `placement-${suffix}`, revision: 1, revisionDigest: 'f'.repeat(64),
      launch: { id: `launch-${suffix}`, triggerId: `trigger-${suffix}`, publicId: `public-${suffix}`,
        placementId: `placement-${suffix}`, revisionDigest: 'f'.repeat(64), sandboxId: `sandbox-${suffix}`, agentId: `agent-${suffix}`, owner: visitor },
      commands: [command(`/api/v1/triggers/public/public-${suffix}/launches`)], stats: { before: stats(0), after: stats(1) },
    };
  };
  const common = { schema: 'cma-public-journey-v1', execution: 'configured_remote_origin', scope: 'managed_default_inline', run };
  const j5 = { ...common, visitor: 'visitor', publications: [publication('first', 'owner', 'visitor'), publication('second', 'second-owner', 'visitor')] };
  const bootstrap = publication('managed', 'owner', 'owner');
  own('workspaces', 'workspace-inline', 'owner');
  const agents = ['one', 'two'].map((suffix) => {
    own('agents', `task-${suffix}`, 'owner');
    return { id: `task-${suffix}`, workspaceId: 'workspace-inline', sandboxId: bootstrap.launch.sandboxId, worktreeId: `worktree-${suffix}`, command: command('/api/v1/agents') };
  });
  own('agent-lifecycle', 'retry-operation', 'owner');
  own('agents', 'retry-agent', 'owner');
  journal.push({ run, at: new Date().toISOString(), kind: 'agent-lifecycle', id: 'retry-operation', subject: 'owner', action: 'lifecycle_observed', state: 'succeeded', agent_id: 'retry-agent' });
  const before = {
    isolatedHomes: true, installBeforeCredentials: true, softwarePresent: true,
    dirtyWorktreePresent: true, transcriptsPreserved: true, runtimeVersion: 'a'.repeat(64), imageReferenceHash: 'b'.repeat(64),
    homes: agents.map((agent, index) => ({ agentId: agent.id, homeDigest: (index ? 'c' : 'd').repeat(64), fileHash: 'e'.repeat(64) })),
  };
  const j6 = {
    ...common, owner: 'owner', bootstrap, sandboxId: bootstrap.launch.sandboxId, workspaceId: 'workspace-inline', agents,
    workspaceAgentIds: [...agents.map((agent) => agent.id), 'retry-agent'], expectedManagedImageHash: before.imageReferenceHash,
    before, after: ['restart_pausing', 'restart_reviving', 'rotate_image'].map((fault) => ({ fault, snapshot: { ...structuredClone(before), ...(fault === 'rotate_image' ? { runtimeVersion: 'f'.repeat(64), imageReferenceHash: 'f'.repeat(64) } : {}) } })),
    retry: { operationId: 'retry-operation', agentId: 'retry-agent', commands: [command('/api/v1/agents'), command('/api/v1/agents')] },
    deletedAgentId: agents[0].id, survivingAgentId: agents[1].id,
  };
  return { proofs: { [currentJourneyCases.j5]: j5, [currentJourneyCases.j6]: j6 }, journal };
}

export function campaignFixture(cases) {
  const { proofs, journal } = journeyEvidenceFixture();
  const connections = connectionCampaignFixture();
  return {
    results: { status: 'passed', cases: cases.map((title) => ({
      case: title, status: 'passed', durationMs: 1,
      ...(proofs[title] ? { proof: proofs[title] } : {}),
      ...(connections.proofs[title] ? { connectionProof: connections.proofs[title] } : {}),
    })) },
    journal: [...journal, ...connections.journal], connectionFixture: connections.fixture,
  };
}
