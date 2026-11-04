import { describe, expect, it } from 'vitest';
import { ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

describe('retired Sandbox inventory filter', () => {
  it('keeps history by default and carries the explicit filter across scopes and pages', async () => {
    const test = transport();
    test.answer({ items: [] });
    await test.client.sandboxes.list();
    expect(new URL(test.calls.at(-1)!.url).searchParams.has('exclude_retired')).toBe(false);
    for (const scope of [{}, { organization: 'org' }, { shared: true }]) {
      await test.client.sandboxes.listAccessible({
        ...scope,
        excludeRetired: true,
        cursor: 'next',
        limit: 1,
      });
      const query = new URL(test.calls.at(-1)!.url).searchParams;
      expect(query.get('exclude_retired')).toBe('true');
      expect(query.get('cursor')).toBe('next');
      expect(query.get('limit')).toBe('1');
    }
    await test.client.sandboxes.list({ excludeRetired: true, status: 'retired' });
    expect(new URL(test.calls.at(-1)!.url).searchParams.get('status')).toBe('retired');
    await test.client.sandboxes.list({ excludeRetired: false });
    expect(new URL(test.calls.at(-1)!.url).searchParams.has('exclude_retired')).toBe(false);
  });

  it('refuses a malformed filter before making a request', async () => {
    const test = transport();
    expect(() => test.client.sandboxes.list({ excludeRetired: 'true' as never })).toThrow(
      ValidationError,
    );
    expect(() => test.client.sandboxes.listAccessible({ excludeRetired: 1 as never })).toThrow(
      ValidationError,
    );
    expect(test.calls).toHaveLength(0);
  });
});
