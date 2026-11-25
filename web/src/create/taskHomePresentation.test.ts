import { expect, it } from 'vitest';
import { navigationFixture, parkedRuntimeUpdateFixture } from '../test/hierarchy';
import type { NavigationState } from '../shell/navigation';
import type { AgentSummary } from '../api/types';
import { selectTaskHomePresentation, taskCrewCards } from './taskHomePresentation';

function crew(statuses: AgentSummary['status'][]): NavigationState {
  const item = navigationFixture()[0]!;
  const workspace = item.workspaces[0]!;
  return {
    status: 'ready',
    items: [
      {
        ...item,
        workspaces: [
          {
            ...workspace,
            agents: statuses.map((status, index) => ({
              ...workspace.agents[0]!,
              id: `agent-${index}`,
              status,
              taskActivityAt: `2026-11-23T10:0${index}:00Z`,
            })),
          },
        ],
      },
    ],
  };
}

it('keeps the existing task activity ordering and compares only the three visible cards', () => {
  const navigation = crew(['working', 'paused', 'paused', 'paused']);
  expect(taskCrewCards(navigation).map(({ agent }) => agent.id)).toEqual([
    'agent-3',
    'agent-2',
    'agent-1',
    'agent-0',
  ]);
  const result = selectTaskHomePresentation({ navigation });
  expect(result.crew.map(({ agent }) => agent.id)).toEqual(['agent-3', 'agent-2', 'agent-1']);
  expect(result.commonStatus).toBe('Paused');
});

it.each(['failed', 'waiting_approval'] as const)('retains individual %s attention', (status) => {
  expect(
    selectTaskHomePresentation({ navigation: crew([status, status]) }).commonStatus,
  ).toBeUndefined();
});

it('does not collapse different states, stale reads or parent runtime failures', () => {
  expect(
    selectTaskHomePresentation({ navigation: crew(['working', 'paused']) }).commonStatus,
  ).toBeUndefined();
  const navigation = crew(['working', 'working']);
  if (navigation.status !== 'ready') throw new Error('Expected fixture inventory');
  expect(
    selectTaskHomePresentation({ navigation: { ...navigation, refreshError: 'Stale' } })
      .commonStatus,
  ).toBeUndefined();
  expect(
    selectTaskHomePresentation({
      navigation: {
        ...navigation,
        items: navigation.items.map((item) => ({ ...item, sandbox: parkedRuntimeUpdateFixture() })),
      },
    }).commonStatus,
  ).toBeUndefined();
});

it.each([
  undefined,
  { status: 'loading' },
  { status: 'error', message: 'Unavailable' },
  { status: 'ready', items: [], refreshError: 'Stale' },
] as const)('never treats an unresolved inventory as a fresh empty account: %j', (navigation) => {
  const result = selectTaskHomePresentation({ navigation });
  expect(result.empty).toBe(false);
  expect(result.templates).toBe('menu');
});

it('expands one catalog only for a fresh empty personal editable Home', () => {
  const navigation: NavigationState = { status: 'ready', items: [] };
  expect(selectTaskHomePresentation({ navigation }).templates).toBe('expanded');
  for (const change of [{ scope: 'shared' }, { contextual: true }, { templateSelected: true }])
    expect(selectTaskHomePresentation({ navigation, ...change }).templates).toBe('menu');
  expect(selectTaskHomePresentation({ navigation, submitted: true }).templates).toBe('hidden');
  expect(
    selectTaskHomePresentation({ navigation: crew(['working']), scope: 'shared' }).crew,
  ).toEqual([]);
});
