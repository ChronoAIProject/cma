import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, TransportError } from '../../api-client/errors';
import type { ServiceCatalogClient, ServiceCatalogEntry } from '../client';
import { ServicePicker } from './ServicePicker';

const service = { id: 'api-github', name: 'GitHub' };
const sentinel = '<html>private-catalog-diagnostic</html>';
function picker(catalog: ServiceCatalogClient, onChange = vi.fn()) {
  return <ServicePicker catalog={catalog} value={[]} onChange={onChange} disabled={false} />;
}

it.each([
  [new ApiError(401, sentinel), /Sign in again/, false],
  [
    new ApiError(403, sentinel),
    /permission to use your own Sandboxes or manage your own profiles/,
    false,
  ],
  [new ApiError(503, sentinel, 'profile_services_unavailable'), /temporarily unavailable/, true],
  [new ProtocolError(sentinel), /response could not be verified/, true],
  [new TransportError(sentinel), /could not be reached/, true],
] as const)('explains %s without exposing upstream diagnostics', async (error, message, retry) => {
  const onChange = vi.fn();
  render(picker({ listServices: vi.fn().mockRejectedValue(error) }, onChange));
  expect(await screen.findByRole('alert')).toHaveTextContent(message);
  expect(document.body).not.toHaveTextContent('private-catalog-diagnostic');
  expect(screen.getByRole('textbox', { name: 'Search services' })).toBeDisabled();
  expect(screen.queryByText('No services available')).not.toBeInTheDocument();
  expect(Boolean(screen.queryByRole('button', { name: 'Try again' }))).toBe(retry);
  expect(onChange).not.toHaveBeenCalled();
});

it('recovers a failed catalog with an explicit read and never changes scope on retry', async () => {
  const catalog = {
    listServices: vi
      .fn()
      .mockRejectedValueOnce(new ApiError(503, sentinel))
      .mockResolvedValue([service]),
  };
  const onChange = vi.fn();
  render(picker(catalog, onChange));
  await screen.findByRole('alert');
  await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('checkbox', { name: 'GitHub' })).toBeEnabled();
  await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  expect(screen.getByRole('textbox', { name: 'Search services' })).toBeEnabled();
  expect(catalog.listServices).toHaveBeenCalledTimes(2);
  expect(onChange).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('checkbox', { name: 'GitHub' }));
  expect(onChange).toHaveBeenCalledWith(['api-github']);
});

it('shows an empty catalog even when required services remain visible', async () => {
  render(
    <ServicePicker
      catalog={{ listServices: vi.fn().mockResolvedValue([]) }}
      value={[]}
      disabled={false}
      onChange={vi.fn()}
      fixed={[{ id: 'ornn-api', reason: 'Required' }]}
    />,
  );
  expect(await screen.findByText('No services available')).toBeVisible();
  expect(screen.getByRole('checkbox', { name: 'ornn-api Required' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'ornn-api Required' })).toBeDisabled();
});

it('aborts old catalogs and hides their rows while the next owner catalog loads', async () => {
  let finish!: (entries: readonly ServiceCatalogEntry[]) => void;
  const first = { listServices: vi.fn().mockResolvedValue([service]) };
  const second: ServiceCatalogClient = {
    listServices: vi.fn(
      () =>
        new Promise<readonly ServiceCatalogEntry[]>((resolve) => {
          finish = resolve;
        }),
    ),
  };
  const { rerender } = render(picker(first));
  await screen.findByRole('checkbox', { name: 'GitHub' });
  rerender(picker(second));
  expect(screen.queryByRole('checkbox', { name: 'GitHub' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Search services' })).toBeDisabled();
  await act(async () => finish([{ id: 'second-owner', name: 'Second owner service' }]));
  expect(screen.getByRole('checkbox', { name: 'Second owner service' })).toBeVisible();
});

it('ignores a late catalog response after its owner changes or the picker unmounts', async () => {
  let finish!: (entries: readonly ServiceCatalogEntry[]) => void;
  const first = {
    listServices: vi.fn<ServiceCatalogClient['listServices']>(
      () =>
        new Promise<readonly ServiceCatalogEntry[]>((resolve) => {
          finish = resolve;
        }),
    ),
  };
  const second = { listServices: vi.fn().mockResolvedValue([]) };
  const { rerender, unmount } = render(picker(first));
  rerender(picker(second));
  expect(first.listServices.mock.calls[0]?.[0]?.signal?.aborted).toBe(true);
  await screen.findByText('No services available');
  await act(async () => finish([service]));
  expect(screen.queryByRole('checkbox', { name: 'GitHub' })).not.toBeInTheDocument();
  unmount();
});

it('honors Retry-After without automatically reading or changing scope', async () => {
  vi.useFakeTimers();
  try {
    const catalog = {
      listServices: vi
        .fn()
        .mockRejectedValue(new ApiError(429, sentinel, undefined, undefined, 3000)),
    };
    const onChange = vi.fn();
    render(picker(catalog, onChange));
    await act(async () => {});
    const retry = screen.getByRole('button', { name: 'Try again' });
    expect(retry).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('Too many catalog requests');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2999);
    });
    expect(retry).toBeDisabled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(retry).toBeEnabled();
    expect(catalog.listServices).toHaveBeenCalledOnce();
    expect(onChange).not.toHaveBeenCalled();
  } finally {
    vi.useRealTimers();
  }
});
