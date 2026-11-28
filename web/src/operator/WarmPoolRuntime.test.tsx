import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { WarmPools } from './WarmPools';
import { operatorClient, warmPool } from './testFixtures';

const desktopImage = `registry.example/cma-runtime-omarchy@sha256:${'b'.repeat(64)}`;
const previousDesktopImage = `registry.example/cma-runtime-omarchy@sha256:${'c'.repeat(64)}`;

function renderPools(available: boolean, target: number, observed = true) {
  const client = operatorClient();
  const standard = { ...warmPool, id: 'standard', current_runtime: true };
  const desktop = {
    ...standard,
    id: 'desktop',
    image: desktopImage,
    config_sha256: 'b'.repeat(64),
    machine: true,
    target_replicas: target,
    ready_replicas: 0,
    observed,
  };
  client.admin.listWarmPools.mockResolvedValue({
    items: [
      standard,
      {
        ...standard,
        id: 'large',
        cpu_millis: 2000,
        config_sha256: 'e'.repeat(64),
      },
      { ...standard, id: 'old-standard', current_runtime: false, config_sha256: 'd'.repeat(64) },
      desktop,
      { ...desktop, id: 'old-desktop', current_runtime: false, image: previousDesktopImage },
    ],
    default_cpu_millis: standard.cpu_millis,
    default_memory_mib: standard.memory_mib,
    default_image: standard.image,
    default_config_sha256: standard.config_sha256,
    default_claim_pool_id: available ? standard.id : null,
    machine_image: desktop.image,
    machine_config_sha256: desktop.config_sha256,
    machine_claim_pool_id: available ? desktop.id : null,
    runtime_versions: [
      { image: standard.image, version: '1.2.3' },
      { image: desktop.image, version: '2.3.4' },
      { image: previousDesktopImage, version: '2.3.3' },
    ],
    available,
  });
  render(<WarmPools client={client} />);
}

describe('warm pool runtime sections', () => {
  it('keeps history and default claim authority with each runtime', async () => {
    const user = userEvent.setup();
    renderPools(true, 3);
    const standard = within(await screen.findByRole('region', { name: 'Standard runtime' }));
    const desktop = within(screen.getByRole('region', { name: 'Shared desktop runtime' }));

    expect(standard.getAllByText('cma-runtime · 1.2.3')).toHaveLength(2);
    expect(desktop.getByText('cma-runtime-omarchy · 2.3.4')).toBeVisible();
    expect(desktop.getByText(desktopImage)).not.toBeVisible();
    await user.click(desktop.getByText('cma-runtime-omarchy · 2.3.4'));
    expect(desktop.getByText(desktopImage)).toBeVisible();

    expect(standard.getAllByText('Used for new default-size Agents')).toHaveLength(1);
    expect(desktop.getAllByText('Used for new default-size Agents')).toHaveLength(1);
    expect(standard.getByText('For matching size')).toBeVisible();
    expect(standard.queryByText('No ready spare')).toBeNull();
    expect(desktop.getByText('No ready spare')).toBeVisible();

    const oldStandard = within(standard.getByRole('table', { name: 'Previous runtime pools' }));
    const oldDesktop = within(desktop.getByRole('table', { name: 'Previous runtime pools' }));
    expect(oldStandard.getByText('cma-runtime · 1.2.3')).toBeVisible();
    expect(oldDesktop.getByText('cma-runtime-omarchy · 2.3.3')).toBeVisible();
    expect(oldStandard.queryByRole('button', { name: 'Update target' })).toBeNull();
    expect(oldDesktop.queryByRole('button', { name: 'Update target' })).toBeNull();
  });

  it.each([
    [true, 0, true, 'Spare capacity disabled'],
    [true, 3, false, 'Ready capacity unknown'],
    [false, 3, true, 'Ready capacity unknown'],
  ])(
    'distinguishes disabled and unknown capacity (%s, %s, %s)',
    async (available, target, observed, text) => {
      renderPools(available, target, observed);
      const desktop = within(await screen.findByRole('region', { name: 'Shared desktop runtime' }));
      expect(desktop.getByText(new RegExp(text))).toBeVisible();
      expect(desktop.queryByText('No ready spare')).toBeNull();
    },
  );
});
