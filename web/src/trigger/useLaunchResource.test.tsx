import { readFileSync } from 'node:fs';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Launch, LaunchProgress } from '@chronoai/cma-sdk';
import { useLaunchResource } from './useLaunchResource';

const api = vi.hoisted(() => ({
  readLaunch: vi.fn(),
  readLaunchProgress: vi.fn(),
}));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

const FIXTURE = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch.json', 'utf8'),
) as Launch;
const PROGRESS_FIXTURE = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch_progress.json', 'utf8'),
) as LaunchProgress;

function launch(id: string, revision: number): Launch {
  return {
    ...FIXTURE,
    id,
    trigger_public_id: `trigger-${id}`,
    revision_number: revision,
  };
}

function progress(id: string): LaunchProgress {
  return { launch_id: id, events: [], next_cursor: 0 };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('Launch resource route identity', () => {
  it('projects an empty B state and accepts its lower revision after A', async () => {
    const a = launch('launch-a', 12);
    const b = launch('launch-b', 1);
    const bProgress = deferred<LaunchProgress>();
    api.readLaunchProgress.mockImplementation((id: string) =>
      id === a.id ? Promise.resolve(progress(a.id)) : bProgress.promise,
    );
    api.readLaunch.mockImplementation((id: string) => Promise.resolve(id === a.id ? a : b));
    const view = renderHook(({ id }) => useLaunchResource(id), {
      initialProps: { id: a.id },
    });

    await waitFor(() => expect(view.result.current.launch?.id).toBe(a.id));
    expect(view.result.current.lastUpdated).not.toBeNull();
    const aSignal = api.readLaunchProgress.mock.calls[0]?.[2] as AbortSignal;

    view.rerender({ id: b.id });
    expect(aSignal.aborted).toBe(true);
    expect(view.result.current).toMatchObject({
      launch: null,
      loading: true,
      failure: null,
      offline: false,
      lastUpdated: null,
      liveSummary: '',
    });

    await act(async () => {
      bProgress.resolve(progress(b.id));
    });
    await waitFor(() => expect(view.result.current.launch?.id).toBe(b.id));
    expect(view.result.current.launch?.revision_number).toBe(1);
  });

  it('keeps A cleared when B fails to load', async () => {
    const a = launch('launch-a', 8);
    const bProgress = deferred<LaunchProgress>();
    api.readLaunchProgress.mockImplementation((id: string) =>
      id === a.id ? Promise.resolve(progress(a.id)) : bProgress.promise,
    );
    api.readLaunch.mockResolvedValue(a);
    const view = renderHook(({ id }) => useLaunchResource(id), {
      initialProps: { id: a.id },
    });

    await waitFor(() => expect(view.result.current.launch?.id).toBe(a.id));
    view.rerender({ id: 'launch-b' });
    expect(view.result.current.launch).toBeNull();
    expect(view.result.current.failure).toBeNull();

    await act(async () => {
      bProgress.reject(new Error('Launch B unavailable'));
    });
    await waitFor(() => expect(view.result.current.failure).toBe('Launch B unavailable'));
    expect(view.result.current).toMatchObject({
      launch: null,
      loading: false,
      offline: false,
      lastUpdated: null,
      liveSummary: '',
    });
  });

  it('ignores a late A progress response after B is current', async () => {
    const aProgress = deferred<LaunchProgress>();
    const b = launch('launch-b', 2);
    api.readLaunchProgress.mockImplementation((id: string) =>
      id === 'launch-a' ? aProgress.promise : Promise.resolve(progress(b.id)),
    );
    api.readLaunch.mockResolvedValue(b);
    const view = renderHook(({ id }) => useLaunchResource(id), {
      initialProps: { id: 'launch-a' },
    });
    const aSignal = api.readLaunchProgress.mock.calls[0]?.[2] as AbortSignal;

    view.rerender({ id: b.id });
    await waitFor(() => expect(view.result.current.launch?.id).toBe(b.id));
    expect(aSignal.aborted).toBe(true);
    await act(async () => {
      aProgress.resolve(progress('launch-a'));
    });

    expect(view.result.current.launch?.id).toBe(b.id);
    expect(api.readLaunch).not.toHaveBeenCalledWith('launch-a', expect.anything());
  });
});

describe('Launch progress recovery', () => {
  it('replays an event cursor until its matching Launch snapshot succeeds', async () => {
    const initial = launch('launch-replay', 1);
    const updated = launch(initial.id, 2);
    let progressRead = 0;
    api.readLaunchProgress.mockImplementation((_id: string, after: number) => {
      progressRead += 1;
      if (progressRead === 1) return Promise.resolve(progress(initial.id));
      return Promise.resolve({
        ...PROGRESS_FIXTURE,
        launch_id: initial.id,
        events: after === 0 ? PROGRESS_FIXTURE.events : [],
        next_cursor: 1,
      });
    });
    api.readLaunch
      .mockResolvedValueOnce(initial)
      .mockRejectedValueOnce(new Error('snapshot temporarily unavailable'))
      .mockResolvedValueOnce(updated);
    const view = renderHook(() => useLaunchResource(initial.id));

    await waitFor(() => expect(view.result.current.launch?.revision_number).toBe(1));
    act(() => window.dispatchEvent(new Event('focus')));
    await waitFor(() =>
      expect(view.result.current.failure).toBe('snapshot temporarily unavailable'),
    );
    act(() => window.dispatchEvent(new Event('focus')));

    await waitFor(() => expect(view.result.current.launch?.revision_number).toBe(2));
    expect(api.readLaunchProgress.mock.calls.slice(0, 3).map((call) => call[1])).toEqual([0, 0, 0]);
  });

  it('ignores a superseded poll even when its transport resolves after abort', async () => {
    const current = launch('launch-overlap', 1);
    const first = deferred<LaunchProgress>();
    const second = deferred<LaunchProgress>();
    api.readLaunchProgress.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    api.readLaunch.mockResolvedValue(current);
    const view = renderHook(() => useLaunchResource(current.id));
    const firstSignal = api.readLaunchProgress.mock.calls[0]?.[2] as AbortSignal;

    act(() => window.dispatchEvent(new Event('focus')));
    expect(firstSignal.aborted).toBe(true);
    await act(async () => first.resolve({ ...progress(current.id), next_cursor: 1 }));
    expect(api.readLaunch).not.toHaveBeenCalled();

    await act(async () => second.resolve(progress(current.id)));
    await waitFor(() => expect(view.result.current.launch?.id).toBe(current.id));
    expect(api.readLaunch).toHaveBeenCalledOnce();
  });
});
