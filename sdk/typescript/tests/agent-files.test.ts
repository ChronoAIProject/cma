import { describe, expect, it, vi } from 'vitest';
import { createClient, ProtocolError } from '../src/index.js';

describe('persistent-home file response bounds', () => {
  it('downloads files beyond the normal JSON limit while retaining that limit elsewhere', async () => {
    const bytes = new Uint8Array(9 * 1024 * 1024).fill(91);
    const fetch = vi.fn(async () => new Response(bytes));
    const client = createClient({ baseUrl: 'https://cma.example', fetch });
    const downloaded = await client.agentFiles.download('agent', 'cma/report.bin');
    expect(downloaded).toHaveLength(bytes.length);
    expect(downloaded.every((value) => value === 91)).toBe(true);
    await expect(client.agents.settings('agent')).rejects.toThrow(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(2);
  });
  it('rejects oversized declared and streamed downloads', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(null, { headers: { 'content-length': String(32 * 1024 * 1024 + 1) } }),
      )
      .mockResolvedValueOnce(
        new Response(
          new ReadableStream({
            start(controller) {
              controller.enqueue(new Uint8Array(32 * 1024 * 1024));
              controller.enqueue(new Uint8Array(1));
              controller.close();
            },
          }),
        ),
      );
    const client = createClient({ baseUrl: 'https://cma.example', fetch });
    await expect(client.agentFiles.download('agent', 'large.bin')).rejects.toThrow(
      '33554432-byte boundary',
    );
    await expect(client.agentFiles.download('agent', 'large.bin')).rejects.toThrow(
      '33554432-byte boundary',
    );
  });
  it('decodes bounded file JSON larger than ordinary JSON responses', async () => {
    const body = {
      path: 'cma/source',
      size_bytes: 9 * 1024 * 1024,
      sha256: 'a'.repeat(64),
      content_base64: 'YWFh'.repeat(3 * 1024 * 1024),
    };
    const fetch = vi.fn(async () => Response.json(body));
    const client = createClient({ baseUrl: 'https://cma.example', fetch });
    expect(
      await client.agentFiles.read('agent', body.path, { maxBytes: 32 * 1024 * 1024 }),
    ).toEqual(body);
  });
});
