import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentFilesClient } from '../api/agentFiles';
import { MessageContent } from './MessageContent';
import { MessageMediaContext, messageFilePath } from './mediaSources';

const NativeURL = URL;
const create = vi.fn<(blob: Blob) => string>(() => 'blob:preview');
const revoke = vi.fn();
beforeEach(() => {
  create.mockClear();
  revoke.mockClear();
  vi.stubGlobal(
    'URL',
    Object.assign(class extends NativeURL {}, {
      createObjectURL: create,
      revokeObjectURL: revoke,
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

function files(download: AgentFilesClient['download']): AgentFilesClient {
  return { download, list: vi.fn(), read: vi.fn(), write: vi.fn(), remove: vi.fn() };
}
function draw(content: string, client: AgentFilesClient, agentId = 'agt_example') {
  return (
    <MessageMediaContext.Provider
      value={{ agentId, files: client, workingDirectory: '/home/cma/project' }}
    >
      <MessageContent content={content} />
    </MessageMediaContext.Provider>
  );
}

it('loads sandbox images with the authorized file client and releases their object URLs', async () => {
  const download = vi.fn().mockResolvedValue(new Blob(['image']));
  const view = render(draw('![Result](sandbox:/home/cma/project/chart.png)', files(download)));
  expect(await screen.findByRole('img', { name: 'Result' })).toHaveAttribute('src', 'blob:preview');
  expect(download).toHaveBeenCalledWith('agt_example', 'cma/project/chart.png', {
    signal: expect.any(AbortSignal),
  });
  expect(create.mock.calls[0]?.[0]).toHaveProperty('type', 'image/png');
  view.unmount();
  expect(revoke).toHaveBeenCalledWith('blob:preview');
  expect(download.mock.calls[0]?.[2]?.signal?.aborted).toBe(true);
});

it('renders remote images and audio/video links with native controls and no autoplay', () => {
  const { container } = render(
    <MessageContent
      content={
        '![Chart](https://example.test/chart.png)\n\n[Clip](https://example.test/movie.mp4?token=public)\n\n[Audio](https://example.test/sound.mp3)'
      }
    />,
  );
  expect(screen.getByRole('img', { name: 'Chart' })).toHaveAttribute(
    'referrerpolicy',
    'no-referrer',
  );
  expect(container.querySelector('video')).toHaveAttribute('controls');
  expect(container.querySelector('video')).toHaveAttribute(
    'src',
    'https://example.test/movie.mp4?token=public',
  );
  expect(container.querySelector('audio')).toHaveAttribute('controls');
  expect(container.querySelector('[autoplay]')).toBeNull();
  expect(create).not.toHaveBeenCalled();
});

it('uses the recorded working directory for relative links and shows retryable failures', async () => {
  const download = vi
    .fn()
    .mockRejectedValueOnce(new Error('Unavailable'))
    .mockResolvedValueOnce(new Blob(['video']));
  const { container } = render(draw('[Demo](./output/demo.webm)', files(download)));
  expect(await screen.findByText(/preview unavailable/)).toBeVisible();
  expect(download.mock.calls[0]?.[1]).toBe('cma/project/output/demo.webm');
  fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }));
  await waitFor(() =>
    expect(container.querySelector('video')).toHaveAttribute('src', 'blob:preview'),
  );
  expect(create.mock.calls[0]?.[0]).toHaveProperty('type', 'video/webm');
});

it('does not expose local previews without file access and cancels late reads on access loss', async () => {
  let resolve!: (blob: Blob) => void;
  const download = vi.fn<AgentFilesClient['download']>(
    () =>
      new Promise<Blob>((done) => {
        resolve = done;
      }),
  );
  const content = '![Private chart](/home/cma/project/chart.png)';
  const view = render(draw(content, files(download)));
  await waitFor(() => expect(download).toHaveBeenCalledOnce());
  view.rerender(<MessageContent content={content} />);
  await act(async () => {
    resolve(new Blob(['private']));
  });
  expect(create).not.toHaveBeenCalled();
  expect(screen.queryByRole('img')).toBeNull();
  expect(screen.getByText('Private chart')).toBeVisible();
  expect(download.mock.calls[0]?.[2]?.signal?.aborted).toBe(true);
});

it('waits until local media enters the viewport before reading files', async () => {
  let notify!: IntersectionObserverCallback;
  const disconnect = vi.fn();
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(callback: IntersectionObserverCallback) {
        notify = callback;
      }
      observe = vi.fn();
      disconnect = disconnect;
    },
  );
  const download = vi.fn().mockResolvedValue(new Blob(['image']));
  render(draw('![Lazy](/home/cma/chart.png)', files(download)));
  expect(download).not.toHaveBeenCalled();
  act(() =>
    notify([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver),
  );
  expect(await screen.findByRole('img', { name: 'Lazy' })).toHaveAttribute('src', 'blob:preview');
  expect(disconnect).toHaveBeenCalled();
});

it.each([
  'javascript:alert(1)',
  'data:image/svg+xml;base64,PHN2Zz4=',
  'file:///home/cma/a.png',
  '//foreign.test/a.png',
  '/etc/passwd',
  '/home/../../etc/passwd',
  '/home/%2e%2e/etc/passwd',
  '/home/cma/%00.png',
  '/home/cma/\\file.png',
])('refuses unsafe or out-of-home media paths: %s', (path) => {
  expect(messageFilePath(path, '/home/cma/project')).toBeUndefined();
});

it('never turns hostile raw HTML or an executable URL into a preview', () => {
  const download = vi.fn();
  const { container } = render(
    draw(
      '<video src="https://example.test/x.mp4" onerror="alert(1)"></video>\n\n![Unsafe](javascript:alert(1))',
      files(download),
    ),
  );
  expect(container.querySelector('video,iframe,script,img,[onerror]')).toBeNull();
  expect(download).not.toHaveBeenCalled();
});
