import { expect, it } from 'vitest';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { taskPresentation } from './taskPresentation';
import { taskStatus } from './taskStatus';

const agent = { ...workspaceFixture().agents[0]!, taskStatus: 'completed' as const };

it('uses an unfinished image rotation even when the Sandbox still reports Active', () => {
  const sandbox = sandboxFixture();
  const rotating = {
    ...sandbox,
    lifecycle: {
      ...sandbox.lifecycle!,
      operation: {
        purpose: 'image_rotation' as const,
        phase: 'conversation_reopen',
        startedAt: sandbox.updatedAt,
      },
    },
  };
  expect(taskPresentation(agent, rotating, 'fresh')).toMatchObject({
    group: 'Updating environment',
    label: 'Completed',
    showRunningTime: false,
  });
  expect(taskStatus(agent, rotating)).toBe('starting');
  expect(taskStatus(agent)).toBe('completed');
});

it('respects a reported pause or revival instead of interpreting it as image rotation', () => {
  const sandbox = sandboxFixture('rotating');
  for (const purpose of ['pause', 'revive'] as const) {
    const presentation = taskPresentation(
      agent,
      {
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: { purpose, phase: 'pending', startedAt: sandbox.updatedAt },
        },
      },
      'fresh',
    );
    expect(presentation?.group).toBeUndefined();
    expect(presentation?.parentNotice).toBeUndefined();
  }
});

it.each(['retiring', 'retired'] as const)(
  'hides %s parents before using stale observations',
  (status) => {
    expect(taskPresentation(agent, sandboxFixture(status), 'stale')).toBeUndefined();
  },
);

it('leaves an unknown task outcome unknown when the environment is healthy', () => {
  expect(
    taskPresentation({ ...agent, taskStatus: undefined }, sandboxFixture(), 'fresh'),
  ).toMatchObject({
    label: 'Status unavailable',
    status: 'unknown',
    showRunningTime: false,
  });
});
