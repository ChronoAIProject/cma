import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { NewSessionDialog } from './NewSessionDialog';
import { DEFAULT_OPTIONS } from './options';

/**
 * The settings decide what an agent may do unattended, so the cases that
 * matter are the defaults and the moment someone selects the permissive mode.
 */

function renderDialog(onStart = vi.fn(), onCancel = vi.fn()) {
  render(<NewSessionDialog open onCancel={onCancel} onStart={onStart} />);
  return { onStart, onCancel };
}

describe('the new session dialog', () => {
  it('starts with the cautious defaults', async () => {
    // Pressing Start without touching anything must never produce a session
    // that acts without asking.
    const user = userEvent.setup();
    const { onStart } = renderDialog();

    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart).toHaveBeenCalledWith('codex', DEFAULT_OPTIONS);
    expect(DEFAULT_OPTIONS.permission).toBe('guarded');
  });

  it('offers the second kernel but does not let it be chosen', async () => {
    // Listed so the product's shape is visible; disabled because nothing
    // behind it works yet, and offering it would be a lie.
    renderDialog();
    const option = screen.getByRole('option', { name: /claude code/i }) as HTMLOptionElement;
    expect(option.disabled).toBe(true);
  });

  it('warns when full access is selected', async () => {
    // Stated plainly rather than hidden behind another click: what someone
    // needs here is to know what it means.
    const user = userEvent.setup();
    renderDialog();

    await user.selectOptions(screen.getByLabelText(/permissions/i), 'unrestricted');

    expect(screen.getByRole('alert')).toHaveTextContent(/without asking/i);
  });

  it('does not warn for the guarded modes', async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.selectOptions(screen.getByLabelText(/permissions/i), 'autonomous');

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('carries the chosen settings through', async () => {
    const user = userEvent.setup();
    const { onStart } = renderDialog();

    await user.selectOptions(screen.getByLabelText(/thinking effort/i), 'xhigh');
    await user.selectOptions(screen.getByLabelText(/permissions/i), 'unrestricted');
    await user.selectOptions(screen.getByLabelText(/model access/i), 'own');
    await user.type(screen.getByPlaceholderText(/default for this deployment/i), 'a-model');
    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart).toHaveBeenCalledWith('codex', {
      model: 'a-model',
      reasoning_effort: 'xhigh',
      permission: 'unrestricted',
      credential: 'own',
    });
  });

  it('leaves the model unset when the field is empty', async () => {
    // An empty string would ask the upstream for a model with no name; absent
    // means "the deployment's default", which is a different thing.
    const user = userEvent.setup();
    const { onStart } = renderDialog();

    await user.type(screen.getByPlaceholderText(/default for this deployment/i), '   ');
    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart.mock.calls[0]?.[1]).not.toHaveProperty('model', '   ');
    expect(onStart.mock.calls[0]?.[1].model).toBeUndefined();
  });

  it('closes on Escape without starting anything', async () => {
    const user = userEvent.setup();
    const { onStart, onCancel } = renderDialog();

    await user.keyboard('{Escape}');

    expect(onCancel).toHaveBeenCalled();
    expect(onStart).not.toHaveBeenCalled();
  });

  it('renders nothing when closed', () => {
    render(<NewSessionDialog open={false} onCancel={vi.fn()} onStart={vi.fn()} />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
