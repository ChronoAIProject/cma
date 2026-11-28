import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type BindingHoldOverview } from '@cma/client';
import { BindingHolds } from './BindingHolds';
import { Operations } from './Operations';
import { heldBindings } from './bindingHolds.test-support';
import { operatorClient } from './testFixtures';

afterEach(() => vi.useRealTimers());

const principal = { principal: 'operator', cacheKey: 'operator:epoch-1' };

it('groups the same Sandbox without adding distinct reporter attempts or exposing owner IDs', async () => {
  const client = operatorClient();
  client.admin.bindingHolds.mockResolvedValue(heldBindings());
  render(<BindingHolds client={client} />);
  expect(await screen.findByText('1 Sandbox with deferred bindings')).toBeVisible();
  expect(screen.getAllByRole('heading', { name: 'Sandbox sbx_observed' })).toHaveLength(1);
  expect(screen.getByText('Alex Chen')).toBeVisible();
  expect(screen.getByText('Name unavailable')).toBeVisible();
  expect(screen.queryByText('private-owner-id')).not.toBeInTheDocument();
  expect(screen.getByText('Concurrent change')).toBeVisible();
  expect(screen.getByText('Retrying at observation time')).toBeVisible();
  expect(screen.getByText('Not recorded')).toBeVisible();
  expect(screen.getByText(/1 sampled reporters were unavailable/)).toBeVisible();
  expect(screen.getByText(/counts only Sandboxes whose runtime binding has been deferred/)).toBeVisible();
  expect(client.admin.retireSandbox).not.toHaveBeenCalled();
  expect(client.admin.bindingHolds).toHaveBeenCalledTimes(1);
});

it('reports an empty partial observation without declaring a recovered or healthy deployment', async () => {
  const client = operatorClient();
  client.admin.bindingHolds.mockResolvedValue({
    ...heldBindings(),
    observed_sandboxes: 0,
    reports: [],
    unavailable_reporters: 3,
  });
  render(<BindingHolds client={client} />);
  expect(await screen.findByText('0 Sandboxes with deferred bindings')).toBeVisible();
  expect(screen.getByText('No held bindings were observed in the returned samples.')).toBeVisible();
  expect(screen.getByText(/Incomplete observation/)).toBeVisible();
  expect(screen.queryByText(/all.*healthy|recovered successfully/i)).not.toBeInTheDocument();
});

it.each([undefined, false])(
  'issues no diagnostic request when advertisement is %s',
  async (bindingDiagnostics) => {
    const client = operatorClient();
    render(
      <Operations
        client={client}
        capabilities={['read_all_sandboxes']}
        principal={principal}
        bindingDiagnostics={bindingDiagnostics}
      />,
    );
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole('heading', { name: 'Operations' })).toBeVisible();
    expect(
      screen.queryByRole('region', { name: 'Deferred runtime bindings' }),
    ).not.toBeInTheDocument();
    expect(client.admin.bindingHolds).not.toHaveBeenCalled();
  },
);

it('does not fetch the feature for an unscoped identity or a caller without cross-owner reads', async () => {
  const client = operatorClient();
  const { rerender } = render(
    <Operations client={client} capabilities={['read_all_sandboxes']} bindingDiagnostics />,
  );
  await screen.findByText('Research Sandbox');
  rerender(
    <Operations
      client={client}
      capabilities={['read_activity_log']}
      principal={principal}
      bindingDiagnostics
    />,
  );
  expect(client.admin.bindingHolds).not.toHaveBeenCalled();
});

it('drops previous rows during refresh and clears them after a rejected read without showing raw errors', async () => {
  const client = operatorClient();
  client.admin.bindingHolds.mockResolvedValueOnce(heldBindings());
  render(<BindingHolds client={client} />);
  await screen.findByText('1 Sandbox with deferred bindings');
  let reject!: (cause: unknown) => void;
  client.admin.bindingHolds.mockImplementationOnce(
    () =>
      new Promise((_, fail) => {
        reject = fail;
      }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Refresh binding observations' }));
  expect(screen.queryByText('Alex Chen')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Refresh binding observations' })).toBeDisabled();
  await act(async () => reject(new ApiError(503, 'private raw service failure')));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Runtime binding observations are unavailable.',
  );
  expect(screen.queryByText('private raw service failure')).not.toBeInTheDocument();
  expect(screen.queryByText('0 Sandboxes with deferred bindings')).not.toBeInTheDocument();
});

it('preserves Retry-After without polling or allowing an early explicit refresh', async () => {
  vi.useFakeTimers();
  const client = operatorClient();
  client.admin.bindingHolds.mockRejectedValueOnce(
    new ApiError(503, 'unavailable', undefined, undefined, 2_000),
  );
  render(<BindingHolds client={client} />);
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByRole('button', { name: 'Refresh binding observations' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh binding observations' }));
  await act(async () => vi.advanceTimersByTimeAsync(1_999));
  expect(client.admin.bindingHolds).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Refresh binding observations' })).toBeDisabled();
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByRole('button', { name: 'Refresh binding observations' })).toBeEnabled();
  expect(client.admin.bindingHolds).toHaveBeenCalledTimes(1);
  client.admin.bindingHolds.mockResolvedValueOnce(heldBindings());
  fireEvent.click(screen.getByRole('button', { name: 'Refresh binding observations' }));
  await act(async () => {
    await Promise.resolve();
  });
  expect(client.admin.bindingHolds).toHaveBeenCalledTimes(2);
});

it.each([401, 403])('requires renewed identity after authority refusal %s', async (status) => {
  const client = operatorClient();
  client.admin.bindingHolds.mockRejectedValueOnce(new ApiError(status, 'private authority detail'));
  render(<BindingHolds client={client} />);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Current permission could not be confirmed.',
  );
  expect(screen.getByRole('button', { name: 'Refresh binding observations' })).toBeDisabled();
  expect(screen.queryByText('private authority detail')).not.toBeInTheDocument();
});

it.each([
  { principal: 'new-operator', cacheKey: 'new:epoch-2' },
  { principal: 'operator', cacheKey: 'operator:epoch-2' },
])(
  'rejects a late response after the authenticated generation becomes $cacheKey',
  async (nextPrincipal) => {
    const client = operatorClient();
    let resolve!: (value: BindingHoldOverview) => void;
    client.admin.bindingHolds.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    client.admin.bindingHolds.mockResolvedValueOnce({
      ...heldBindings(),
      observed_sandboxes: 0,
      reports: [],
      unavailable_reporters: 3,
    });
    const { rerender } = render(
      <Operations
        client={client}
        capabilities={['read_all_sandboxes']}
        principal={principal}
        bindingDiagnostics
      />,
    );
    await waitFor(() => expect(client.admin.bindingHolds).toHaveBeenCalledTimes(1));
    const signal = client.admin.bindingHolds.mock.calls[0]![0]!.signal!;
    rerender(
      <Operations
        client={client}
        capabilities={['read_all_sandboxes']}
        principal={nextPrincipal}
        bindingDiagnostics
      />,
    );
    expect(signal.aborted).toBe(true);
    await screen.findByText('0 Sandboxes with deferred bindings');
    await act(async () => resolve(heldBindings()));
    const panel = screen.getByRole('region', { name: 'Deferred runtime bindings' });
    expect(within(panel).queryByText('Alex Chen')).not.toBeInTheDocument();
    expect(within(panel).queryByText('1 Sandbox with deferred bindings')).not.toBeInTheDocument();
  },
);

it('clears protected rows when the client changes and rejects the previous client response', async () => {
  const previous = operatorClient();
  const next = operatorClient();
  let resolve!: (value: BindingHoldOverview) => void;
  previous.admin.bindingHolds.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  next.admin.bindingHolds.mockResolvedValueOnce({
    ...heldBindings(),
    observed_sandboxes: 0,
    reports: [],
    unavailable_reporters: 3,
  });
  const { rerender } = render(<BindingHolds client={previous} />);
  await waitFor(() => expect(previous.admin.bindingHolds).toHaveBeenCalledTimes(1));
  const signal = previous.admin.bindingHolds.mock.calls[0]![0]!.signal!;
  rerender(<BindingHolds client={next} />);
  expect(signal.aborted).toBe(true);
  await screen.findByText('0 Sandboxes with deferred bindings');
  await act(async () => resolve(heldBindings()));
  expect(screen.queryByText('Alex Chen')).not.toBeInTheDocument();
  expect(screen.queryByText('1 Sandbox with deferred bindings')).not.toBeInTheDocument();
});

it.each(['capability', 'advertisement', 'route'])(
  'clears protected rows and cancels reads on %s loss',
  async (change) => {
    const client = operatorClient();
    let resolve!: (value: BindingHoldOverview) => void;
    client.admin.bindingHolds.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const props = {
      client,
      capabilities: ['read_all_sandboxes'] as const,
      principal,
      bindingDiagnostics: true,
    };
    const { rerender, unmount } = render(<Operations {...props} />);
    await waitFor(() => expect(client.admin.bindingHolds).toHaveBeenCalledTimes(1));
    const signal = client.admin.bindingHolds.mock.calls[0]![0]!.signal!;
    if (change === 'route') unmount();
    else
      rerender(
        <Operations
          {...props}
          capabilities={change === 'capability' ? [] : props.capabilities}
          bindingDiagnostics={change !== 'advertisement'}
        />,
      );
    expect(signal.aborted).toBe(true);
    await act(async () => resolve(heldBindings()));
    expect(
      screen.queryByRole('region', { name: 'Deferred runtime bindings' }),
    ).not.toBeInTheDocument();
  },
);
