import { describe, expect, it, vi } from 'vitest';
import { testClient } from '../test/client';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { loadNavigation } from './navigation';

describe('navigation hierarchy loading', () => {
  it('follows Sandbox pages and expands public Sandbox and Workspace detail projections', async () => {
    const sandbox = sandboxFixture();
    const workspace = workspaceFixture();
    const listSandboxes = vi
      .fn()
      .mockResolvedValueOnce({ items: [sandbox], next: 'next-page' })
      .mockResolvedValueOnce({ items: [] });
    const readSandbox = vi.fn().mockResolvedValue(sandbox);
    const readWorkspace = vi.fn().mockResolvedValue(workspace);
    const client = testClient({ listSandboxes, readSandbox, readWorkspace });

    await expect(loadNavigation(client, new AbortController().signal)).resolves.toEqual([
      { sandbox, workspaces: [workspace] },
    ]);
    expect(listSandboxes).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ cursor: 'next-page' }),
    );
    expect(readSandbox).toHaveBeenCalledWith('sbx_1', expect.any(Object));
    expect(readWorkspace).toHaveBeenCalledWith('wsp_1', expect.any(Object));
  });

  it('rejects a non-advancing public page cursor', async () => {
    const listSandboxes = vi.fn().mockResolvedValue({ items: [], next: 'same' });
    await expect(
      loadNavigation(testClient({ listSandboxes }), new AbortController().signal),
    ).rejects.toThrow(/did not advance/i);
  });
});
