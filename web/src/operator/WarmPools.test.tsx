import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type WarmPoolList } from '@chronoai/cma-sdk';
import { describe, expect, it } from 'vitest';
import { Operations } from './Operations';
import { WarmPools } from './WarmPools';
import { operatorClient, warmPool } from './testFixtures';

describe('native warm pool operations', () => {
  it('gates the view by platform configuration capability and revokes the form immediately', async () => {
    const client = operatorClient();
    const { rerender } = render(
      <Operations client={client} capabilities={['configure_platform']} />,
    );
    expect(await screen.findByRole('article')).toHaveTextContent(warmPool.image);
    expect(screen.getByRole('tab', { name: 'Warm pools' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(client.admin.listProfiles).not.toHaveBeenCalled();
    rerender(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    expect(screen.queryByRole('tab', { name: 'Warm pools' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disable pool' })).not.toBeInTheDocument();
    await screen.findByText('Owner Alex Chen');
  });

  it('reports observed desired/ready/pending counts and retains each original image', async () => {
    const client = operatorClient();
    client.admin.listWarmPools.mockResolvedValue({
      items: [warmPool, { ...warmPool, id: 'pool-2', observed: false, ready_replicas: 3 }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_image: 'registry.example/new-runtime@sha256:' + 'b'.repeat(64),
      available: true,
    });
    render(<WarmPools client={client} />);
    const [observed, waiting] = await screen.findAllByRole('article');
    expect(observed).toHaveTextContent(warmPool.image);
    expect(within(observed!).getByText('Desired').nextElementSibling).toHaveTextContent('3');
    expect(within(observed!).getByText('Ready').nextElementSibling).toHaveTextContent('2');
    expect(within(observed!).getByText('Pending').nextElementSibling).toHaveTextContent('1');
    expect(within(waiting!).getAllByText('Unknown')).toHaveLength(2);
  });

  it('stages resources while infrastructure is unavailable and replays an uncertain create exactly', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.listWarmPools.mockResolvedValue({
      items: [],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
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
    const card = await screen.findByRole('article');
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
      default_image: warmPool.image,
      available: true,
    });
    await user.click(within(card).getByRole('button', { name: 'Read current target' }));
    await waitFor(() =>
      expect(
        within(screen.getByRole('article')).getByLabelText('Ready Sandbox target'),
      ).toHaveValue(6),
    );
    fireEvent.change(within(screen.getByRole('article')).getByLabelText('Ready Sandbox target'), {
      target: { value: '8' },
    });
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
    await screen.findByText(/Read the current target before/);
    expect(screen.getByRole('article')).toHaveTextContent(warmPool.image);
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
    const card = await screen.findByRole('article');
    fireEvent.change(within(card).getByLabelText('Ready Sandbox target'), {
      target: { value: '9' },
    });
    client.admin.listWarmPools.mockResolvedValue({
      items: [{ ...warmPool, revision: 2, target_replicas: 5 }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
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
    await screen.findByRole('article');
    await user.click(screen.getByRole('button', { name: 'Refresh pools' }));
    await user.click(screen.getByRole('button', { name: 'Disable pool' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled(),
    );
    await act(async () => {
      finish(old);
    });
    await waitFor(() => expect(client.admin.listWarmPools).toHaveBeenCalledTimes(3));
    expect(screen.getByRole('button', { name: 'Disable pool' })).toBeDisabled();
    expect(within(screen.getByRole('article')).getByLabelText('Ready Sandbox target')).toHaveValue(
      0,
    );
  });
});
