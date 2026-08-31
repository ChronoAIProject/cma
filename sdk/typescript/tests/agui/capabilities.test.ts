import { describe, expect, it, vi } from 'vitest';
import { resolveConfig } from '../../src/config.js';
import { ProtocolError } from '../../src/errors.js';
import { readAguiCapabilities, toAguiCapabilities } from '../../src/agui/capabilities.js';

describe('AG-UI capability discovery', () => {
  it('retains additive custom declarations including the live chat gate', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          protocol: 'ag-ui',
          features: {
            custom: {
              'cma:chat': { enabled: true },
              'future:feature': { mode: 'new' },
            },
          },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );

    const document = await readAguiCapabilities(
      resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
    );

    expect(document.features.custom['cma:chat']).toEqual({ enabled: true });
    expect(document.features.custom['future:feature']).toEqual({ mode: 'new' });
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/agui/capabilities',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('refuses malformed protocol and feature boundaries', () => {
    for (const value of [
      null,
      { protocol: 'other', features: { custom: {} } },
      { protocol: 'ag-ui', features: null },
      { protocol: 'ag-ui', features: { custom: [] } },
    ]) {
      expect(() => toAguiCapabilities(value)).toThrow(ProtocolError);
    }
  });
});
