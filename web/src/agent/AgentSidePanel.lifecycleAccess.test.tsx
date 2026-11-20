import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { computerFixture } from './computerLifecycle.fixture';

it('renews shared history independently of paused runtime capabilities and withdraws it on denial', async () => {
  let permitted = true;
  const effective = vi.fn(async () =>
    Response.json({
      capabilities: permitted ? ['view_details', 'view_history'] : ['view_details'],
      revalidate_at: new Date(Date.now() + 50_000).toISOString(),
      policy_revision: 1,
    }),
  );
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (String(input).includes('/resource-access/')) return effective();
      return Response.json({
        access: 'shared',
        id: 'agt_1',
        title: 'Shared Agent',
        status: 'ready',
        created_at: '2026-11-01T00:00:00Z',
        updated_at: '2026-11-01T00:00:00Z',
      });
    },
  });
  const f = computerFixture({ resourceViews: sdk });
  const view = render(f.draw({ shared: true, panel: 'activity' }));
  await act(async () => {});
  expect(screen.getByText('retained-output', { exact: true })).toBeVisible();
  expect(effective).toHaveBeenCalledOnce();
  expect(f.files.environment).not.toHaveBeenCalled();
  permitted = false;
  fireEvent.focus(window);
  await act(async () => {});
  expect(screen.queryByText('retained-output', { exact: true })).toBeNull();
  expect(screen.getByText('Operation output access is not permitted.')).toBeVisible();
  expect(effective).toHaveBeenCalledTimes(2);
  expect(f.files.environment).not.toHaveBeenCalled();
  permitted = true;
  view.rerender(f.draw({ shared: true, panel: 'activity', viewerId: 'new-viewer' }));
  expect(screen.queryByText('retained-output', { exact: true })).toBeNull();
  await act(async () => {});
  expect(screen.queryByText('retained-output', { exact: true })).toBeNull();
});
