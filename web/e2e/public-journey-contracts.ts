import assert from 'node:assert/strict';
import type { TriggerInlineTarget } from '@cma/client';
import { identifier, object } from './environment';

export function assertInlineTarget(value: unknown, expected: TriggerInlineTarget) {
  const target = object(value, 'current inline target');
  assert.deepEqual(target.inline, expected, 'Published inline configuration must match selection');
  generatedProfileReferences(target);
}

export function submittedProfileIds(value: unknown) {
  const target = object(value);
  return ['sandbox', 'workspace', 'agent'].flatMap((kind) => {
    const profile = target[`${kind}_profile`];
    return profile && object(profile).id ? [object(profile).id] : [];
  });
}

export function assertInlineRequest(value: unknown, expected: TriggerInlineTarget) {
  const target = object(value);
  assert.deepEqual(target.inline, expected, 'Submitted inline configuration must match selection');
  assert.deepEqual(
    submittedProfileIds(target),
    [],
    'Current authoring cannot select historical profiles',
  );
}

export function generatedProfileReferences(value: unknown) {
  const target = object(value);
  return Object.fromEntries(
    ['sandbox', 'workspace', 'agent'].map((kind) => {
      const reference = object(target[`${kind}_profile`], 'materialized profile reference');
      identifier(reference.id, 'generated profile ID');
      assert.ok(Number.isSafeInteger(reference.revision) && Number(reference.revision) > 0);
      return [kind, { id: reference.id, revision: reference.revision }];
    }),
  );
}

export function assertPublisherIdentity(value: unknown, subject: string) {
  const identity = object(value);
  assert.equal(identity.authenticated, true);
  assert.equal(identity.visitor, false);
  assert.equal(identity.subject, subject);
}

export function assertPublishedTarget(
  value: unknown,
  revisionValue: unknown,
  target: TriggerInlineTarget,
) {
  const document = object(value);
  const revision = object(revisionValue);
  identifier(document.id, 'published Trigger');
  identifier(document.public_id, 'published public ID');
  assert.equal(document.availability, 'published');
  assert.equal(document.revision_count, 1);
  assert.equal(revision.revision, 1);
  assert.ok(typeof revision.digest === 'string' && revision.digest.length > 0);
  assertInlineTarget(object(document.draft).target, target);
  assertInlineTarget(object(revision.content).target, target);
  assert.deepEqual(
    generatedProfileReferences(object(document.draft).target),
    generatedProfileReferences(object(revision.content).target),
    'Published generated references must match the immutable revision',
  );
}

export function assertPublishedLaunch(
  value: unknown,
  sandboxValue: unknown,
  expected: {
    triggerId: string;
    publicId: string;
    placementId: string;
    digest: string;
    subject: string;
  },
) {
  const launch = object(value);
  const sandbox = object(sandboxValue);
  assert.equal(launch.trigger_id, expected.triggerId);
  assert.equal(launch.public_id, expected.publicId);
  assert.equal(launch.placement_id, expected.placementId);
  assert.equal(launch.revision_digest, expected.digest);
  const resolved = object(object(launch.resolution).sandbox);
  assert.equal(resolved.state, 'created');
  assert.equal(sandbox.id, resolved.id);
  assert.equal(sandbox.owner, expected.subject, 'New work belongs to the visiting identity');
}

export function assertSingleLaunchDelta(beforeValue: unknown, afterValue: unknown, id: string) {
  const before = object(beforeValue);
  const after = object(afterValue);
  const placement = (value: Record<string, unknown>) => {
    assert.ok(Array.isArray(value.placements));
    const matches = value.placements
      .map((row: unknown) => object(row))
      .filter((row: Record<string, unknown>) => row.placement_id === id);
    assert.equal(matches.length, 1);
    return matches[0]!;
  };
  for (const [start, end] of [
    [object(before.totals), object(after.totals)],
    [placement(before), placement(after)],
  ]) {
    for (const key of ['launches_total', 'agents_ready_total']) {
      assert.ok(Number.isSafeInteger(start![key]) && Number(start![key]) >= 0);
      assert.equal(end![key], Number(start![key]) + 1, `Exactly one ${key} per explicit intent`);
    }
  }
}

export function assertContextualTask(value: unknown, workspaceId: string, message: string) {
  const body = object(value);
  assert.equal(body.name, 'New task');
  assert.equal(body.workspace_id, workspaceId);
  assert.equal(body.initial_message, message);
  for (const key of Object.keys(body))
    assert.ok(
      ['name', 'workspace_id', 'initial_message', 'title_mode', 'machine_included'].includes(key),
      'Contextual creation must use the current inline task contract',
    );
}
