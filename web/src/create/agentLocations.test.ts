import { expect, it } from 'vitest';
import { navigationFixture, sandboxFixture, workspaceFixture } from '../test/hierarchy';
import {
  NO_SANDBOX_PERMISSION,
  agentLocationOptions,
  locationId,
  locationScope,
} from './agentLocations';

it('maps each location to exactly one shape of the lifecycle request', () => {
  expect(locationScope({ kind: 'new_sandbox' })).toEqual({
    workspaceId: undefined,
    sandboxId: undefined,
  });
  expect(locationScope(undefined)).toEqual({ workspaceId: undefined, sandboxId: undefined });
  expect(locationScope({ kind: 'sandbox', sandboxId: 'sbx_1' })).toEqual({
    workspaceId: undefined,
    sandboxId: 'sbx_1',
  });
  expect(locationScope({ kind: 'workspace', workspaceId: 'wsp_1' })).toEqual({
    workspaceId: 'wsp_1',
    sandboxId: undefined,
  });
  expect(locationId({ kind: 'workspace', workspaceId: 'wsp_1', sandboxId: 'sbx_1' })).toBe(
    'workspace:wsp_1',
  );
});

it('leaves out deleted and deleting resources instead of disabling them', () => {
  const items = [
    ...navigationFixture().map((item) => ({
      ...item,
      workspaces: [
        ...item.workspaces,
        { ...workspaceFixture(), id: 'wsp_gone', name: 'gone', status: 'removing' as const },
      ],
    })),
    {
      sandbox: { ...sandboxFixture(), id: 'sbx_old', status: 'retiring' as const },
      workspaces: [],
    },
  ];
  expect(agentLocationOptions(items, 'owner', true, undefined).map((option) => option.id)).toEqual([
    'new-sandbox',
    'sandbox:sbx_1',
    'workspace:wsp_1',
  ]);
});

it('keeps an opened target the hierarchy does not list, without guessing its eligibility', () => {
  const options = agentLocationOptions([], 'owner', false, { kind: 'sandbox', sandboxId: 'sbx_9' });
  expect(options).toEqual([
    expect.objectContaining({ id: 'new-sandbox', reason: NO_SANDBOX_PERMISSION }),
    {
      id: 'sandbox:sbx_9',
      location: { kind: 'sandbox', sandboxId: 'sbx_9' },
      title: 'New workspace in this Sandbox',
    },
  ]);
  expect(
    agentLocationOptions([], 'owner', true, { kind: 'workspace', workspaceId: 'wsp_9' }, 'notes')[1]
      ?.title,
  ).toBe('notes');
});
