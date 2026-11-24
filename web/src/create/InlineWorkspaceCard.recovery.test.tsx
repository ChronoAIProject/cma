import '../test/connectedAccounts';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createRestWebClient } from '../api/rest';
import { restTransport, WIRE_WORKSPACE, WIRE_WORKSPACE_DETAIL } from '../test/rest';
import { InlineWorkspaceCard } from './InlineWorkspaceCard';

it.each(['lost response', 'created but detail read failed'])(
  'keeps Workspace inputs and request identity locked after %s and a later refusal',
  async (scenario) => {
    const user = userEvent.setup();
    const transport = restTransport();
    if (scenario === 'lost response') {
      transport.fetch.mockRejectedValueOnce(new TypeError('response lost'));
    } else {
      transport.reply(WIRE_WORKSPACE, 201);
      transport.reply({ status: 403, code: 'forbidden', detail: 'Read unavailable' }, 403);
    }
    const onCreated = vi.fn();
    const onClose = vi.fn();
    render(
      <InlineWorkspaceCard
        client={createRestWebClient(transport.sdk)}
        sandboxId="sandbox-1"
        onCreated={onCreated}
        onClose={onClose}
      />,
    );
    const name = () => screen.getByLabelText('Workspace name');
    fireEvent.change(name(), { target: { value: 'original workspace' } });
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    await screen.findByRole('button', { name: 'Retry creation' });
    expect(name()).toBeDisabled();
    expect(onCreated).not.toHaveBeenCalled();

    transport.reply({ status: 403, code: 'forbidden', detail: 'Creation access refused' }, 403);
    const previousCalls = transport.fetch.mock.calls.length;
    await user.click(screen.getByRole('button', { name: 'Retry creation' }));
    await waitFor(() => expect(transport.fetch).toHaveBeenCalledTimes(previousCalls + 1));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Creating' })).toBeNull());
    expect(name()).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Workspace source' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent(/original creation.*unresolved/i);
    expect(screen.getByRole('alert')).toHaveTextContent(/same request/i);
    await user.keyboard('{Escape}');
    expect(onClose).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();

    transport.reply(WIRE_WORKSPACE, 201);
    transport.reply(WIRE_WORKSPACE_DETAIL);
    await user.click(screen.getByRole('button', { name: 'Retry creation' }));
    await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: WIRE_WORKSPACE.id }));
    const writes = transport.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
    expect(writes).toHaveLength(3);
    expect(new Set(writes.map(([, init]) => init?.body)).size).toBe(1);
    expect(JSON.parse(String(writes[0]![1]?.body))).toEqual({
      source: { type: 'empty' },
      name: 'original workspace',
    });
    const keys = writes.map(([, init]) => new Headers(init?.headers).get('idempotency-key'));
    expect(keys[0]).toEqual(expect.any(String));
    expect(new Set(keys).size).toBe(1);
  },
);

it.each([
  {
    status: 422,
    code: 'invalid_request',
    message: 'Creation was refused. Review the selected profile and location before trying again.',
  },
  {
    status: 409,
    code: 'workspace_name_conflict',
    message: 'This workspace name is already in use. Choose another workspace name.',
  },
])(
  'unlocks Workspace creation after $code so corrected inputs use a new request',
  async ({ status, code, message }) => {
    const user = userEvent.setup();
    const transport = restTransport();
    transport.reply({ status, code, detail: 'Name refused' }, status);
    const onCreated = vi.fn();
    render(
      <InlineWorkspaceCard
        client={createRestWebClient(transport.sdk)}
        sandboxId="sandbox-1"
        onCreated={onCreated}
        onClose={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(message);
    expect(screen.getByLabelText('Workspace name')).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    fireEvent.change(screen.getByLabelText('Workspace name'), {
      target: { value: 'corrected workspace' },
    });
    transport.reply(WIRE_WORKSPACE, 201);
    transport.reply(WIRE_WORKSPACE_DETAIL);
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
    const writes = transport.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
    expect(writes).toHaveLength(2);
    expect(writes.map(([, init]) => JSON.parse(String(init?.body)) as unknown)).toEqual([
      { source: { type: 'empty' }, name: 'workspace' },
      { source: { type: 'empty' }, name: 'corrected workspace' },
    ]);
    const keys = writes.map(([, init]) => new Headers(init?.headers).get('idempotency-key'));
    expect(keys[0]).toEqual(expect.any(String));
    expect(keys[1]).toEqual(expect.any(String));
    expect(keys[0]).not.toBe(keys[1]);
  },
);

it('blocks an invalid Workspace repository and submits the corrected URL with its ref', async () => {
  const user = userEvent.setup();
  const transport = restTransport();
  const onCreated = vi.fn();
  render(
    <InlineWorkspaceCard
      client={createRestWebClient(transport.sdk)}
      sandboxId="sandbox-1"
      onCreated={onCreated}
      onClose={vi.fn()}
    />,
  );
  await user.click(screen.getByRole('button', { name: 'Workspace source' }));
  await user.click(screen.getByText('Clone a GitHub repository'));
  const repository = screen.getByLabelText('GitHub repository URL');
  const reference = screen.getByLabelText('Branch or ref (optional)');
  const create = screen.getByRole('button', { name: 'Create workspace' });
  expect(create).toBeDisabled();
  await user.type(repository, 'not-a-url');
  await user.click(reference);
  expect(repository).toHaveAttribute('aria-invalid', 'true');
  expect(repository).toHaveAccessibleDescription('Enter an HTTPS GitHub repository URL.');
  expect(create).toBeDisabled();
  await user.click(create);
  expect(transport.fetch).not.toHaveBeenCalled();

  // URL parsing normalizes an explicit default port, but the Workspace API
  // requires the literal github.com authority.
  await user.clear(repository);
  await user.type(repository, 'https://github.com:443/example/workspace');
  await user.click(reference);
  expect(repository).toHaveAttribute('aria-invalid', 'true');
  expect(create).toBeDisabled();
  await user.click(create);
  expect(transport.fetch).not.toHaveBeenCalled();

  const validUrl = 'https://github.com/example/workspace';
  await user.clear(repository);
  await user.type(repository, validUrl);
  await user.type(reference, 'feature/workspace');
  expect(repository).not.toHaveAttribute('aria-invalid');
  expect(create).toBeEnabled();
  transport.reply(WIRE_WORKSPACE, 201);
  transport.reply(WIRE_WORKSPACE_DETAIL);
  await user.click(create);
  await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
  const writes = transport.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(writes).toHaveLength(1);
  expect(JSON.parse(String(writes[0]![1]?.body))).toEqual({
    source: { type: 'git_repository', url: validUrl, reference: 'feature/workspace' },
    name: 'workspace',
  });
});
