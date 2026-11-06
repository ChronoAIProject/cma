import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { FileExplorer } from './FileExplorer';
import { changedFiles } from '../fileChanges';

it('exposes nested changes before expansion, marks ancestors, and opens the authorized file', async () => {
  const file = {
    path: 'cma/project/src/app.js',
    name: 'app.js',
    kind: 'file' as const,
    size_bytes: 8,
    hidden: false,
  };
  const list = vi.fn(async (_agent: string, path: string) =>
    path === 'cma/project'
      ? [
          {
            path: 'cma/project/src',
            name: 'src',
            kind: 'directory' as const,
            size_bytes: 0,
            hidden: false,
          },
        ]
      : [file],
  );
  const read = vi.fn(async () => ({
    path: file.path,
    size_bytes: 8,
    sha256: 'a'.repeat(64),
    content_base64: btoa('contents'),
  }));
  const changes = changedFiles(
    [
      {
        id: 'edit',
        kind: 'edit',
        title: 'app.js',
        status: 'completed',
        output: { text: 'File changes: 1 +4 −2\nFile: "src/app.js" +4 −2', truncated: false },
      },
    ],
    '/home/cma/project',
  );
  render(
    <FileExplorer
      panel
      embedded
      initialDirectory="cma/project"
      changes={changes}
      client={{ list, read, download: vi.fn(), write: vi.fn(), remove: vi.fn() }}
      agentId="agent"
      readOnly={false}
      onClose={vi.fn()}
    />,
  );
  const changed = screen.getByRole('region', { name: 'Changed files' });
  expect(within(changed).getByText('src/app.js')).toBeVisible();
  expect(within(changed).getByText('+4')).toBeVisible();
  expect(within(changed).getByText('−2')).toBeVisible();
  expect(await screen.findByLabelText('Contains changed files')).toBeVisible();
  expect(screen.getByRole('button', { name: 'src' })).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(within(changed).getByRole('button'));
  expect(await screen.findByRole('textbox', { name: 'Contents of app.js' })).toHaveValue(
    'contents',
  );
  expect(read).toHaveBeenCalledWith('agent', file.path, expect.any(Object));
});

it('keeps unknown counts unknown when exact edits later touch the same file', () => {
  const changes = changedFiles([
    {
      id: 'one',
      kind: 'edit',
      title: 'a',
      status: 'completed',
      output: {
        text: 'File changes: 1 counts unavailable\nFile: "/home/a" counts unavailable',
        truncated: false,
      },
    },
    {
      id: 'two',
      kind: 'edit',
      title: 'a',
      status: 'completed',
      output: { text: 'File changes: 1 +2 −1\nFile: "/home/a" +2 −1', truncated: false },
    },
  ]);
  expect(changes.get('a')).toEqual({ path: 'a', added: undefined, removed: undefined });
});
