import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { referenceClient } from '../test/references';
import { ScopeSettings } from './ScopeSettings';

it('requires a paused shared Sandbox and retains only the new grant through uncertain retry', async () => {
  const grant = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost grant response'))
    .mockResolvedValue(undefined);
  const client = agentClient({ grantSandboxServiceAccess: grant });
  const profiles = referenceClient();
  const draw = (paused: boolean) => (
    <CmaClientProvider client={client} profiles={profiles}>
      <ScopeSettings
        detail={{
          ...AGENT_DETAIL,
          sandbox: { ...AGENT_DETAIL.sandbox, status: paused ? 'paused' : 'active' },
        }}
      />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  expect(screen.getByRole('button', { name: 'Add services' })).toBeDisabled();
  expect(grant).not.toHaveBeenCalled();
  view.rerender(draw(true));
  await userEvent.click(screen.getByRole('button', { name: 'Add services' }));
  await screen.findByText('Lost grant response');
  expect(screen.getByRole('checkbox', { name: 'Chrono Storage' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry scope grant' }));
  await screen.findByText('Services granted. Resume the Sandbox to apply them to its Agent keys.');
  expect(grant.mock.calls[0]?.slice(0, 2)).toEqual(['sbx_1', ['storage']]);
  expect(grant.mock.calls[1]).toEqual(grant.mock.calls[0]);
  expect(screen.getByRole('checkbox', { name: 'storage Already granted' })).toBeDisabled();
});
