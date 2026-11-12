import { describe, expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { harness, item, responseId } from './responses-v2-fixtures.js';

const page = {
  object: 'cma.response.output_page',
  response_id: responseId,
  data: [item],
  has_more: true,
  next_after: 'indexed:8',
  through: 12000,
};

describe('bounded response output pages', () => {
  it('preserves the opaque cursor and requests one page without submitting input', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(Response.json(page));
    expect(await t.client.responsesV2.readOutputPage(responseId, { after: 'indexed:7' })).toEqual(
      page,
    );
    expect(t.fetch).toHaveBeenCalledTimes(1);
    const [url, init] = t.fetch.mock.calls[0]!;
    expect(new URL(String(url)).searchParams.get('after')).toBe('indexed:7');
    expect(init?.method).toBe('GET');
    expect(init?.body).toBeUndefined();
  });

  it.each([
    { response_id: 'resp_foreign' },
    { next_after: null },
    { has_more: false },
    { next_after: 'indexed:7' },
    { next_after: '' },
    { data: [] },
  ])('refuses a misleading page or continuation: %j', async (changed) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(Response.json({ ...page, ...changed }));
    await expect(
      t.client.responsesV2.readOutputPage(responseId, { after: 'indexed:7' }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(t.fetch).toHaveBeenCalledTimes(1);
  });
});
