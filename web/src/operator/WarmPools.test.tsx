import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type WarmPoolList } from '@cma/client';
import { describe, expect, it } from 'vitest';
import { Operations } from './Operations';
import { WarmPools } from './WarmPools';
import { operatorClient, warmPool } from './testFixtures';

import { cell } from './warmPoolTestHelpers';

describe('native warm pool operations', () => {
  it('gates the view by platform configuration capability and revokes the form immediately', async () => {
    const client = operatorClient();
    const { rerender } = render(
      <Operations client={client} capabilities={['configure_platform']} />,
    );
    expect(await screen.findByRole('row', { name: /Warm pool/ })).toBeInTheDocument();
    expect(screen.getAllByText(warmPool.image).length).toBeGreaterThan(0);
    expect(screen.getByRole('tab', { name: 'Warm pools' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(client.admin.listProfiles).not.toHaveBeenCalled();
    rerender(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    expect(screen.queryByRole('tab', { name: 'Warm pools' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disable pool' })).not.toBeInTheDocument();
    await screen.findByText('Alex Chen', { exact: true });
  });

  it('reports observed desired/ready/pending counts and retains each original image', async () => {
    const client = operatorClient();
    client.admin.listWarmPools.mockResolvedValue({
      items: [warmPool, { ...warmPool, id: 'pool-2', observed: false, ready_replicas: 3 }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: 'registry.example/new-runtime@sha256:' + 'b'.repeat(64),
      available: true,
    });
    render(<WarmPools client={client} />);
    const [observed, waiting] = await screen.findAllByRole('row', { name: /Warm pool/ });
    expect(observed).toHaveTextContent(warmPool.image);
    expect(cell(observed!, 'Desired')).toHaveTextContent('3');
    expect(cell(observed!, 'Ready')).toHaveTextContent('2');
    expect(cell(observed!, 'Pending')).toHaveTextContent('1');
    expect(cell(waiting!, 'Ready')).toHaveTextContent('Unknown');
    expect(cell(waiting!, 'Pending')).toHaveTextContent('Unknown');
  });

  it('shows current-image pools by default and collapses disabled previous runtime records', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const oldImage = 'registry.example/previous-runtime@sha256:' + 'b'.repeat(64);
    client.admin.listWarmPools.mockResolvedValue({
      items: [
        warmPool,
        { ...warmPool, id: 'other-size', config_sha256: 'd'.repeat(64), cpu_millis: 2000 },
        {
          ...warmPool,
          id: 'previous',
          image: oldImage,
          target_replicas: 0,
          ready_replicas: 0,
          status: 'disabled',
        },
      ],
      default_cpu_millis: warmPool.cpu_millis,
      default_memory_mib: warmPool.memory_mib,
      default_config_sha256: warmPool.config_sha256,
      default_image: warmPool.image,
      available: true,
    });
    render(<WarmPools client={client} />);
    await screen.findByText('Default configuration');
    const summary = screen.getByText('Previous runtime pools (1)');
    expect(summary.closest('details')).not.toHaveAttribute('open');
    const cards = screen.getAllByRole('row', { name: /Warm pool/ });
    expect(cards[0]).toBeVisible();
    expect(cards[1]).toBeVisible();
    expect(cards[2]).not.toBeVisible();
    expect(screen.queryByText(/Previous runtime pools still/)).not.toBeInTheDocument();
    await user.click(summary);
    const previous = screen
      .getAllByRole('row', { name: /Warm pool/ })
      .find((row) => row.textContent?.includes(oldImage))!;
    expect(within(previous).queryByLabelText('Ready Sandbox target')).not.toBeInTheDocument();
    expect(
      within(previous).queryByRole('button', { name: 'Update target' }),
    ).not.toBeInTheDocument();
    expect(within(previous).getByRole('button', { name: 'Disable pool' })).toBeDisabled();
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
  });

  it('keeps previous capacity visible while draining and only permits disabling its target', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const previous = { ...warmPool, id: 'previous', image: 'registry.example/previous-runtime' };
    client.admin.listWarmPools.mockResolvedValue({
      items: [warmPool, previous],
      default_cpu_millis: warmPool.cpu_millis,
      default_memory_mib: warmPool.memory_mib,
      default_config_sha256: warmPool.config_sha256,
      default_image: warmPool.image,
      available: true,
    });
    client.admin.resizeWarmPool.mockResolvedValue({
      ...previous,
      revision: 2,
      target_replicas: 0,
      status: 'disabled',
    });
    render(<WarmPools client={client} />);
    await screen.findByText(/Previous runtime pools still have spare capacity/);
    const history = screen.getByText('Previous runtime pools (1)').closest('details')!;
    expect(history).toHaveAttribute('open');
    const card = within(history).getByRole('row', { name: /Warm pool/ });
    expect(within(card).queryByRole('button', { name: 'Update target' })).not.toBeInTheDocument();
    await user.click(within(card).getByRole('button', { name: 'Disable pool' }));
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Confirm disable' }));
    expect(client.admin.resizeWarmPool).toHaveBeenCalledWith('previous', {
      expected_revision: 1,
      target_replicas: 0,
    });
    await waitFor(() =>
      expect(within(card).getByRole('button', { name: 'Disable pool' })).toBeDisabled(),
    );
    expect(history).toHaveAttribute('open');
    expect(cell(card, 'Ready')).toHaveTextContent('2');
    expect(
      screen.getByText(/Previous runtime pools still have spare capacity/),
    ).toBeInTheDocument();
    expect(client.admin.configureWarmPool).not.toHaveBeenCalled();
  });

  it.each([
    { error: 'Capacity cleanup failed', observed: true, ready_replicas: 0 },
    { error: null, observed: false, ready_replicas: null },
  ])('does not conceal previous pool errors or unconfirmed capacity: %j', async (observation) => {
    const client = operatorClient();
    client.admin.listWarmPools.mockResolvedValue({
      items: [
        {
          ...warmPool,
          ...observation,
          image: 'registry.example/previous-runtime',
          target_replicas: 0,
        },
      ],
      default_cpu_millis: warmPool.cpu_millis,
      default_memory_mib: warmPool.memory_mib,
      default_image: warmPool.image,
      available: true,
    });
    render(<WarmPools client={client} />);
    await screen.findByText(/Previous runtime pools still have spare capacity/);
    expect(screen.getByText('Previous runtime pools (1)').closest('details')).toHaveAttribute(
      'open',
    );
    expect(screen.getByRole('row', { name: /Warm pool/ })).toBeVisible();
    if (observation.error)
      expect(screen.getByText(observation.error)).toHaveAttribute('role', 'alert');
    expect(
      screen.getByText(/No warm pools configured for the current runtime/),
    ).toBeInTheDocument();
    expect(screen.queryByText('Used for new default-size Agents')).not.toBeInTheDocument();
  });

  it('lists the seeded shared desktop pool on its own with a target to resize', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const desktopImage = 'registry.example/desktop-runtime@sha256:' + 'e'.repeat(64);
    const desktop = {
      ...warmPool,
      id: 'desktop',
      machine: true,
      image: desktopImage,
      config_sha256: 'e'.repeat(64),
      cpu_millis: 2000,
      memory_mib: 8192,
    };
    client.admin.listWarmPools.mockResolvedValue({
      items: [warmPool, desktop],
      default_cpu_millis: warmPool.cpu_millis,
      default_memory_mib: warmPool.memory_mib,
      default_config_sha256: warmPool.config_sha256,
      default_image: warmPool.image,
      machine_image: desktopImage,
      available: true,
    });
    client.admin.resizeWarmPool.mockResolvedValue({ ...desktop, revision: 2, target_replicas: 5 });
    render(<WarmPools client={client} />);
    const table = await screen.findByRole('table', { name: 'Shared desktop pool' });
    const desktopSection = screen.getByRole('region', { name: 'Shared desktop runtime' });
    const reference = within(desktopSection).getByText(/desktop-runtime · sha256:/);
    expect(within(desktopSection).getByText(desktopImage)).not.toBeVisible();
    await user.click(reference);
    expect(within(desktopSection).getByText(desktopImage)).toBeVisible();
    expect(
      within(table)
        .getAllByRole('columnheader')
        .map((column) => column.textContent),
    ).toEqual([
      'Size',
      'Status',
      'Desired',
      'Ready',
      'Provider target',
      'Pending',
      'Target',
      'Actions',
    ]);
    const row = within(table).getByRole('row', { name: 'Warm pool 2000 millicores / 8192 MiB' });
    expect(
      within(screen.getByRole('table', { name: 'Current runtime pools' })).getAllByRole('row', {
        name: /Warm pool/,
      }),
    ).toHaveLength(1);
    expect(screen.queryByText(/Previous runtime pools/)).not.toBeInTheDocument();
    // Only the current Agent runtime is configured from the board; this pool is seeded.
    expect(screen.getAllByText('Configure a warm pool')).toHaveLength(1);
    expect(within(row).getByRole('button', { name: 'Disable pool' })).toBeEnabled();
    fireEvent.change(within(row).getByLabelText('Ready Sandbox target'), {
      target: { value: '5' },
    });
    await user.click(within(row).getByRole('button', { name: 'Update target' }));
    expect(client.admin.resizeWarmPool).toHaveBeenCalledWith('desktop', {
      expected_revision: 1,
      target_replicas: 5,
    });
    await screen.findByText('Target saved.');
    const resized = within(screen.getByRole('table', { name: 'Shared desktop pool' })).getByRole(
      'row',
      { name: 'Warm pool 2000 millicores / 8192 MiB' },
    );
    expect(cell(resized, 'Desired')).toHaveTextContent('5');
    expect(client.admin.configureWarmPool).not.toHaveBeenCalled();
  });

  it.each([
    { machine_image: 'registry.example/desktop-runtime@sha256:' + 'f'.repeat(64) },
    { machine_image: null },
  ])('keeps a desktop pool for another image under previous runtime pools: %j', async (config) => {
    const client = operatorClient();
    const user = userEvent.setup();
    const oldImage = 'registry.example/desktop-runtime@sha256:' + 'e'.repeat(64);
    client.admin.listWarmPools.mockResolvedValue({
      items: [
        warmPool,
        {
          ...warmPool,
          id: 'old-desktop',
          machine: true,
          image: oldImage,
          target_replicas: 0,
          ready_replicas: 0,
          status: 'disabled',
        },
      ],
      default_cpu_millis: warmPool.cpu_millis,
      default_memory_mib: warmPool.memory_mib,
      default_config_sha256: warmPool.config_sha256,
      default_image: warmPool.image,
      available: true,
      ...config,
    });
    render(<WarmPools client={client} />);
    const summary = await screen.findByText('Previous runtime pools (1)');
    await user.click(summary);
    const previous = within(
      screen.getByRole('table', { name: 'Previous runtime pools' }),
    ).getByRole('row', { name: /Warm pool/ });
    expect(previous).toHaveTextContent(oldImage);
    expect(within(previous).queryByRole('button', { name: 'Update target' })).toBeNull();
    if (config.machine_image) {
      expect(
        await screen.findByRole('cell', { name: /No shared desktop pool is recorded/ }),
      ).toBeVisible();
    } else {
      expect(screen.queryByRole('table', { name: 'Shared desktop pool' })).toBeNull();
    }
  });

  it('stages resources while infrastructure is unavailable and replays an uncertain create exactly', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.listWarmPools.mockResolvedValue({
      items: [],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: warmPool.image,
      available: false,
    });
    client.admin.configureWarmPool.mockRejectedValueOnce(new TypeError('Disconnected'));
    render(<WarmPools client={client} />);
    await screen.findByText(/Warm pool capacity is currently unavailable/);
    await user.click(screen.getByText('Configure a warm pool'));
    expect(screen.getByLabelText('CPU (millicores)')).toHaveValue(1000);
    expect(screen.getByLabelText('Memory (MiB)')).toHaveValue(1024);
    fireEvent.change(screen.getByLabelText('CPU (millicores)'), { target: { value: '2000' } });
    fireEvent.change(screen.getByLabelText('Memory (MiB)'), { target: { value: '8192' } });
    fireEvent.change(screen.getByLabelText('Ready Sandbox target'), { target: { value: '4' } });
    await user.click(screen.getByRole('button', { name: 'Save pool target' }));
    expect(await screen.findByRole('button', { name: 'Retry same request' })).toBeEnabled();
    expect(screen.getByLabelText('CPU (millicores)')).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Retry same request' }));
    await screen.findByText(/Pool target saved/);
    expect(client.admin.configureWarmPool).toHaveBeenCalledTimes(2);
    const [first, second] = client.admin.configureWarmPool.mock.calls;
    expect(first).toEqual(second);
    expect(first?.[0]).toEqual({ cpu_millis: 2000, memory_mib: 8192, target_replicas: 4 });
    expect(first?.[1]?.idempotencyKey).toEqual(expect.any(String));
    expect(first?.[0]).not.toHaveProperty('profile');
  });

  it('disables only the recorded original pool and sends its current revision', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<WarmPools client={client} />);
    await user.click(await screen.findByRole('button', { name: 'Disable pool' }));
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Confirm disable' }));
    await waitFor(() =>
      expect(client.admin.resizeWarmPool).toHaveBeenCalledWith('pool-1', {
        expected_revision: 1,
        target_replicas: 0,
      }),
    );
    expect(await screen.findByRole('button', { name: 'Disable pool' })).toBeDisabled();
    expect(client.admin.configureWarmPool).not.toHaveBeenCalled();
  });

  it('requires a fresh read after a revision conflict, then submits the reviewed revision', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.resizeWarmPool.mockRejectedValueOnce(
      new ApiError(409, 'Pool target changed', 'warm_pool_conflict'),
    );
    render(<WarmPools client={client} />);
    const card = await screen.findByRole('row', { name: /Warm pool/ });
    fireEvent.change(within(card).getByLabelText('Ready Sandbox target'), {
      target: { value: '5' },
    });
    await user.click(within(card).getByRole('button', { name: 'Update target' }));
    await screen.findByText(/Read the current target before/);
    expect(within(card).getByRole('button', { name: 'Update target' })).toBeDisabled();
    expect(client.admin.resizeWarmPool).toHaveBeenCalledTimes(1);
    client.admin.listWarmPools.mockResolvedValue({
      items: [{ ...warmPool, revision: 7, target_replicas: 6 }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: warmPool.image,
      available: true,
    });
    await user.click(screen.getByRole('button', { name: 'Read current target' }));
    await waitFor(() =>
      expect(
        within(screen.getByRole('row', { name: /Warm pool/ })).getByLabelText(
          'Ready Sandbox target',
        ),
      ).toHaveValue(6),
    );
    fireEvent.change(
      within(screen.getByRole('row', { name: /Warm pool/ })).getByLabelText('Ready Sandbox target'),
      {
        target: { value: '8' },
      },
    );
    await user.click(screen.getByRole('button', { name: 'Update target' }));
    await waitFor(() =>
      expect(client.admin.resizeWarmPool).toHaveBeenLastCalledWith('pool-1', {
        expected_revision: 7,
        target_replicas: 8,
      }),
    );
  });

  it('keeps the original pool visible after an unconfirmed resize and blocks repeat changes until read', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.resizeWarmPool.mockRejectedValue(new TypeError('Disconnected'));
    render(<WarmPools client={client} />);
    await user.click(await screen.findByRole('button', { name: 'Disable pool' }));
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Confirm disable' }));
    await screen.findByText(/Read the current target before/);
    expect(screen.getByRole('row', { name: /Warm pool/ })).toHaveTextContent('Pending');
    expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled();
    client.admin.listWarmPools.mockRejectedValueOnce(new Error('Read unavailable'));
    await user.click(screen.getByRole('button', { name: 'Read current target' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Read current target' })).toBeEnabled(),
    );
    expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled();
    expect(client.admin.resizeWarmPool).toHaveBeenCalledTimes(1);
  });

  it('retains an edited target when another operator changes the recorded revision', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<WarmPools client={client} />);
    const card = await screen.findByRole('row', { name: /Warm pool/ });
    fireEvent.change(within(card).getByLabelText('Ready Sandbox target'), {
      target: { value: '9' },
    });
    client.admin.listWarmPools.mockResolvedValue({
      items: [{ ...warmPool, revision: 2, target_replicas: 5 }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: warmPool.image,
      available: true,
    });
    await user.click(screen.getByRole('button', { name: 'Refresh pools' }));
    await screen.findByText(/Your draft is retained/);
    expect(within(card).getByLabelText('Ready Sandbox target')).toHaveValue(9);
    expect(within(card).getByRole('button', { name: 'Update target' })).toBeDisabled();
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
  });

  it('does not replace a successful mutation with an inventory response started before it', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    let finish!: (value: WarmPoolList) => void;
    const old = {
      items: [warmPool],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: warmPool.image,
      available: true,
    };
    const current = { ...old, items: [{ ...warmPool, revision: 2, target_replicas: 0 }] };
    client.admin.listWarmPools
      .mockResolvedValueOnce(old)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      )
      .mockResolvedValue(current);
    render(<WarmPools client={client} />);
    await screen.findByRole('row', { name: /Warm pool/ });
    await user.click(screen.getByRole('button', { name: 'Refresh pools' }));
    await user.click(screen.getByRole('button', { name: 'Disable pool' }));
    expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Confirm disable' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled(),
    );
    await act(async () => {
      finish(old);
    });
    await waitFor(() => expect(client.admin.listWarmPools).toHaveBeenCalledTimes(3));
    expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled();
    expect(
      within(screen.getByRole('row', { name: /Warm pool/ })).getByLabelText('Ready Sandbox target'),
    ).toHaveValue(0);
  });
});
