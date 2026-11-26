import { afterEach, expect, it, vi } from 'vitest';
import { createNyxidAuthProvider } from './nyxid';

afterEach(() => vi.unstubAllGlobals());

it.each([undefined, false, true, 'true', 1, null])(
  'advertises binding diagnostics only for explicit true (%s)',
  async (binding_diagnostics) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              authenticated: true,
              subject: 'operator',
              capabilities: ['read_all_sandboxes'],
              binding_diagnostics,
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          ),
      ),
    );
    const state = await createNyxidAuthProvider().current();
    expect(state.status).toBe('authenticated');
    if (state.status === 'authenticated')
      expect(state.user.bindingDiagnostics).toBe(binding_diagnostics === true ? true : undefined);
  },
);
