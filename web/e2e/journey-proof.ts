import { createHash } from 'node:crypto';
import type { TriggerInlineTarget, TriggerStatistics } from '@cma/client';
import { object, required, type Hooks } from './environment';
import type { Command, Launch } from './harness';
import { generatedProfileReferences, submittedProfileIds } from './public-journey-contracts';
import type { publishCurrentTrigger } from './publisher-current';

export const hashText = (value: string) => createHash('sha256').update(value).digest('hex');
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, entry]) => [key, canonical(entry)]),
    );
  return value;
}
export const hashValue = (value: unknown) => hashText(JSON.stringify(canonical(value)));
export function commandProof(row: Command) {
  return {
    method: row.method,
    path: row.path,
    bodyHash: row.bodyHash,
    keyHash: hashText(required(row.key, 'journey command idempotency key')),
  };
}
function imageProof(target: TriggerInlineTarget) {
  if (!target.image) return { kind: 'managed_default' };
  return {
    kind: 'catalogue',
    id: target.image.id,
    version: required(target.image.version, 'published image version'),
    referenceHash: hashText(required(target.image.reference, 'published image reference')),
  };
}
export function publicationProof(
  published: Awaited<ReturnType<typeof publishCurrentTrigger>>,
  launch: Launch,
  sandbox: unknown,
  commands: Command[],
  before: TriggerStatistics,
  after: TriggerStatistics,
) {
  const { document, revision, placement, target, subject } = published;
  return {
    publisher: subject,
    observedPublisher: published.observedPublisher,
    targetKind: 'inline',
    submittedProfileIds: submittedProfileIds(published.requestTarget),
    submittedTargetHash: hashValue(object(published.requestTarget).inline),
    publishedProfiles: generatedProfileReferences(document.draft.target),
    revisionProfiles: generatedProfileReferences(revision.content.target),
    authoringCommands: published.authoringCommands.map(commandProof),
    requestedTargetHash: hashValue(target),
    publishedTargetHash: hashValue(document.draft.target.inline),
    revisionTargetHash: hashValue(revision.content.target.inline),
    expectedImage: imageProof(target),
    observedImage: imageProof(revision.content.target.inline!),
    triggerId: document.id,
    publicId: document.public_id,
    placementId: placement.id,
    revision: revision.revision,
    revisionDigest: revision.digest,
    launch: {
      id: launch.id,
      triggerId: launch.trigger_id,
      publicId: launch.public_id,
      placementId: launch.placement_id,
      revisionDigest: launch.revision_digest,
      sandboxId: launch.resolution.sandbox.id,
      agentId: launch.resolution.agent.id,
      owner: object(sandbox).owner,
    },
    commands: commands.map(commandProof),
    stats: { before, after },
  };
}
export function workCommands(commands: Command[]) {
  return commands.filter((row) =>
    /\/(launches|start|revive|input|agui\/run|responses|agents)$/.test(row.path),
  );
}

export function runtimeProof(value: Awaited<ReturnType<Hooks['verifyRuntime']>>) {
  return {
    isolatedHomes: value.isolatedHomes,
    installBeforeCredentials: value.installBeforeCredentials,
    softwarePresent: value.softwarePresent,
    dirtyWorktreePresent: value.dirtyWorktreePresent,
    transcriptsPreserved: value.transcriptsPreserved,
    runtimeVersion: hashText(value.runtimeVersion),
    imageReferenceHash: hashText(value.imageReference),
    homes: value.homes,
  };
}
