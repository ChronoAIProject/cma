import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { ExportInventoryPage, inventoryExport } from './ExportInventoryPage';

const row = {
  agent_id: 'agt_retained',
  title: 'Original title',
  status: 'deleted' as const,
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-01T00:00:00Z',
  creation_origin: 'trigger' as const,
};
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it('exports only documented safe creation fields even if an object includes private references', () => {
  const value = {
    ...row,
    owner: 'private-owner',
    details: { trigger_id: 'private-trigger' },
    workspace_id: 'private-parent',
  };
  const encoded = inventoryExport([value]);
  expect(JSON.parse(encoded)).toEqual({ agents: [row] });
  expect(encoded).not.toContain('private');
});

it('reads fresh authorized data before downloading instead of exporting the displayed cache', async () => {
  const createObjectURL = vi.fn((blob: Blob) => {
    expect(blob).toBeInstanceOf(Blob);
    return 'blob:fixture';
  });
  vi.stubGlobal(
    'URL',
    class extends URL {
      static override createObjectURL = createObjectURL;
      static override revokeObjectURL = vi.fn();
    },
  );
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    expect(String(input)).toContain('/api/v1/agents/provenance');
    return Response.json({ items: [{ ...row, title: 'Current title' }] });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  render(
    <ExportInventoryPage
      client={client}
      options={{ organization: 'org', creationOrigin: 'trigger', cursor: 'agt_before', limit: 50 }}
      disabled={false}
    />,
  );
  await userEvent.setup().click(screen.getByRole('button', { name: 'Export this page' }));
  await screen.findByText('Page exported');
  expect(fetch).toHaveBeenCalledOnce();
  expect(String(fetch.mock.calls[0]?.[0])).toContain('creation_origin=trigger');
  expect(createObjectURL).toHaveBeenCalledOnce();
  const blob = createObjectURL.mock.calls[0]?.[0] as Blob;
  const text = await new Promise<string>((resolve) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.readAsText(blob);
  });
  expect(text).toContain('Current title');
});

it('does not download a late export after the scope changes or access is refused', async () => {
  const createObjectURL = vi.fn();
  vi.stubGlobal(
    'URL',
    class extends URL {
      static override createObjectURL = createObjectURL;
    },
  );
  let finish: ((response: Response) => void) | undefined;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  });
  const view = render(
    <ExportInventoryPage client={client} options={{ organization: 'org' }} disabled={false} />,
  );
  await userEvent.setup().click(screen.getByRole('button', { name: 'Export this page' }));
  view.rerender(
    <ExportInventoryPage
      client={client}
      options={{ organization: 'another-org' }}
      disabled={false}
    />,
  );
  await act(async () => finish?.(Response.json({ items: [row] })));
  expect(createObjectURL).not.toHaveBeenCalled();
  await userEvent.setup().click(screen.getByRole('button', { name: 'Export this page' }));
  await act(async () =>
    finish?.(
      Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 }),
    ),
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('Access removed');
  expect(createObjectURL).not.toHaveBeenCalled();
});
