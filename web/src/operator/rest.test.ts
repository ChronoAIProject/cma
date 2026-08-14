import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  activityQuery,
  createRestOperatorStore,
  toActivityEntry,
  toAdminSession,
  toInventory,
  toPerson,
  toProviderReport,
} from './rest';

/**
 * The operator store that talks to the control plane.
 *
 * What these pin is the boundary: a payload becomes the model without anything
 * being invented, a failure arrives as a sentence fit to show somebody, the
 * request carries the session cookie — and, above all, that a provider nobody
 * could ask never turns into a provider running nothing.
 */

function respond(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as unknown as Response;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('reading people off the wire', () => {
  it('keeps the name, the address and the identifier apart', () => {
    const person = toPerson({
      subject: '76fe9d91',
      display_name: 'Ada Lovelace',
      email: 'ada@example.test',
      first_seen: '2026-08-01T09:14:22Z',
      last_seen: '2026-08-14T11:02:05Z',
    });
    expect(person.subject).toBe('76fe9d91');
    expect(person.displayName).toBe('Ada Lovelace');
    expect(person.email).toBe('ada@example.test');
    expect(person.firstSeen).toBe(Date.parse('2026-08-01T09:14:22Z'));
  });

  it('refuses somebody with no identifier', () => {
    // A row nothing can be keyed on is a rendering bug waiting to happen.
    expect(() => toPerson({ display_name: 'Nameless' })).toThrow(/identifier/i);
  });

  it('leaves details the provider withheld absent', () => {
    const person = toPerson({ subject: '76fe9d91' });
    expect(person.displayName).toBeUndefined();
    expect(person.email).toBeUndefined();
    expect(person.lastSeen).toBeUndefined();
  });
});

describe('reading the record off the wire', () => {
  it('keeps what was recorded, including the parameters that were kept', () => {
    const entry = toActivityEntry({
      id: 'actv_1',
      at: '2026-08-14T11:02:05Z',
      actor: '76fe9d91',
      action: 'session_terminate',
      target: { kind: 'session', id: 'sesn_1' },
      outcome: 'succeeded',
      detail: { runtime: 'example-runtime', limit: 100 },
    });
    expect(entry.actor).toBe('76fe9d91');
    expect(entry.action).toBe('session_terminate');
    expect(entry.target).toEqual({ kind: 'session', id: 'sesn_1' });
    expect(entry.detail).toEqual([
      { key: 'runtime', value: 'example-runtime' },
      { key: 'limit', value: '100' },
    ]);
  });

  it('keeps an unattributed entry unattributed', () => {
    // A refused caller is recorded precisely because they were refused, and
    // attributing them to somebody would be worse than recording an absence.
    const entry = toActivityEntry({ id: 'actv_2', action: 'session_list', outcome: 'denied' });
    expect(entry.actor).toBeUndefined();
    expect(entry.target).toBeUndefined();
    expect(entry.detail).toEqual([]);
  });

  it('leaves an unreadable timestamp absent rather than rendering a bad date', () => {
    expect(
      toActivityEntry({ id: 'actv_3', at: 'not a date', outcome: 'failed' }).at,
    ).toBeUndefined();
  });
});

describe('reading the inventory off the wire', () => {
  it('reads a provider that answered', () => {
    const report = toProviderReport({
      kernel: 'example-runtime',
      observable: true,
      runtimes: [{ runtime: 'sbx_1', session: 'sesn_1', owner: '76fe9d91', status: 'ready' }],
    });
    expect(report.observable).toBe(true);
    if (report.observable) {
      expect(report.runtimes[0]?.runtime).toBe('sbx_1');
      expect(report.runtimes[0]?.unaccounted).toBe(false);
    }
  });

  it('marks a runtime nothing accounts for', () => {
    const report = toProviderReport({
      kernel: 'example-runtime',
      observable: true,
      runtimes: [{ runtime: 'sbx_stray', session: 'sesn_forgotten' }],
    });
    if (report.observable) {
      expect(report.runtimes[0]?.unaccounted).toBe(true);
    }
  });

  it('never turns a provider that could not be asked into one running nothing', () => {
    // The single most important assertion in this file. An empty list here
    // would draw an idle namespace and an operator would act on it.
    const report = toProviderReport({
      kernel: 'example-runtime',
      observable: false,
      reason: 'cannot enumerate',
    });
    expect(report.observable).toBe(false);
    if (!report.observable) {
      expect(report.reason).toBe('cannot enumerate');
    }
  });

  it('treats a provider that claims to be observable with no list as unobservable', () => {
    // Degrading into "we could not tell" is the safe direction; degrading into
    // "nothing is running" is the one that misleads.
    expect(toProviderReport({ kernel: 'x', observable: true }).observable).toBe(false);
  });

  it('withholds the total while any provider is unobservable', () => {
    const inventory = toInventory({
      running: 3,
      fully_observed: false,
      providers: [
        { kernel: 'a', observable: true, runtimes: [] },
        { kernel: 'b', observable: false, reason: 'unreachable' },
      ],
    });
    expect(inventory.running).toBeUndefined();
    expect(inventory.fullyObserved).toBe(false);
    expect(inventory.providers).toHaveLength(2);
  });

  it('reports a genuine zero as a zero', () => {
    // An answer of nothing is an answer, and must read differently from not
    // having asked.
    const inventory = toInventory({
      running: 0,
      fully_observed: true,
      providers: [{ kernel: 'a', observable: true, runtimes: [] }],
    });
    expect(inventory.running).toBe(0);
    expect(inventory.fullyObserved).toBe(true);
  });
});

describe('reading a session off the wire', () => {
  it('carries the owner and the name to show them by', () => {
    const session = toAdminSession({
      id: 'sesn_1',
      owner: '76fe9d91',
      owner_label: 'Ada Lovelace',
      status: 'running',
      kernel: 'example-runtime',
      created_at: '2026-08-14T09:00:00Z',
    });
    expect(session.owner).toBe('76fe9d91');
    expect(session.ownerLabel).toBe('Ada Lovelace');
    expect(session.status).toBe('running');
  });

  it('keeps a state it has never heard of rather than refusing the row', () => {
    // An operator console that would not draw an unfamiliar state would hide
    // the session most worth looking at.
    expect(toAdminSession({ id: 'sesn_2', status: 'hibernating' }).status).toBe('hibernating');
  });

  it('refuses a session with no identifier', () => {
    expect(() => toAdminSession({ owner: '76fe9d91' })).toThrow(/identifier/i);
  });
});

describe('talking to the control plane', () => {
  it('sends the session cookie', async () => {
    const fetchMock = vi.fn().mockResolvedValue(respond([]));
    vi.stubGlobal('fetch', fetchMock);

    await createRestOperatorStore().people();

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/admin/users',
      expect.objectContaining({ credentials: 'include' }),
    );
  });

  it('asks the control plane to narrow the record by person', async () => {
    // Server-side rather than in the component: the record is served with a
    // limit, so filtering a page down to one person shows a handful of rows
    // where asking for that person shows a page of theirs.
    const fetchMock = vi.fn().mockResolvedValue(respond([]));
    vi.stubGlobal('fetch', fetchMock);

    await createRestOperatorStore().activity({ actor: '76fe9d91', limit: 100 });

    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/admin/activity?actor=76fe9d91&limit=100');
  });

  it('omits what was not asked for', () => {
    expect(activityQuery({})).toBe('');
    expect(activityQuery({ actor: '' })).toBe('');
    expect(activityQuery({ actor: 'a b' })).toBe('?actor=a+b');
  });

  it('reports the control plane’s own explanation of a refusal', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          respond(
            { detail: 'This operation requires an operator capability this caller does not hold.' },
            403,
          ),
        ),
    );
    await expect(createRestOperatorStore().people()).rejects.toThrow(/operator capability/);
  });

  it('reports an unreachable control plane rather than hanging', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    await expect(createRestOperatorStore().sessions()).rejects.toThrow(/cannot reach/i);
  });

  it('refuses a list that is not a list', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(respond({ oops: true })));
    await expect(createRestOperatorStore().sessions()).rejects.toThrow(/was not a list/);
  });

  it('acts on a session through its own address, encoded', async () => {
    const fetchMock = vi.fn().mockResolvedValue(respond(null, 204));
    vi.stubGlobal('fetch', fetchMock);

    const store = createRestOperatorStore();
    await store.stopSession('sesn 1');
    await store.terminateSession('sesn_2');

    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/admin/sessions/sesn%201/stop');
    expect((fetchMock.mock.calls[0]?.[1] as RequestInit).method).toBe('POST');
    expect(fetchMock.mock.calls[1]?.[0]).toBe('/api/v1/admin/sessions/sesn_2/terminate');
  });
});
