import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { OwnerAgentHeader } from './OwnerAgentHeader';
it('preserves owner, unsupported-client and busy reasons for the restored menu actions', () => {
  const client = agentClient({
    readAgentPreferences: vi.fn(),
    updateAgentPreferences: vi.fn(),
    readAgentSkills: vi.fn(),
    installAgentSkill: vi.fn(),
  });
  const draw = (readOnly: boolean, busy: boolean, actionsBusy = false, supported = true) => (
    <CmaClientProvider client={supported ? client : agentClient()}>
      <OwnerAgentHeader
        detail={AGENT_DETAIL}
        readOnly={readOnly}
        busy={busy}
        actionsBusy={actionsBusy}
        title="Task"
        status="ready"
        breadcrumb={[]}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(draw(false, true));
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveAttribute(
    'aria-disabled',
    'false',
  );
  expect(screen.getByRole('menuitem', { name: /^Update system prompt/ })).toHaveAccessibleName(
    expect.stringContaining('Wait until'),
  );
  view.rerender(draw(false, false, true));
  expect(screen.getByRole('menuitem', { name: /^Rename/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  expect(screen.getByRole('menuitem', { name: /^Update skills/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  view.rerender(draw(true, false));
  expect(screen.getByRole('menuitem', { name: /^Update skills/ })).toHaveAccessibleName(
    expect.stringContaining('Only the task owner'),
  );
  view.rerender(draw(false, false, false, false));
  expect(screen.getByRole('menuitem', { name: /^Update system prompt/ })).toHaveAccessibleName(
    expect.stringContaining('unsupported by this client'),
  );
});
