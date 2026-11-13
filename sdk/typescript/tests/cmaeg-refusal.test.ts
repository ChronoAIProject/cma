import { describe, expect, it } from 'vitest';
import { ProblemSchema, UpstreamRefusalSchema } from '../src/cmaeg/types.js';

describe('CMAEG provider refusal metadata', () => {
  const legacy = { code: 'provider_rejected', status: 502, title: 'Provider rejected' };

  it('preserves legacy absence and the additive nullable extension', () => {
    expect(ProblemSchema(legacy)).toEqual(legacy);
    expect(ProblemSchema({ ...legacy, upstream: null })).toEqual({ ...legacy, upstream: null });
    for (const code of ['media_source_refused', 'unrecognized']) {
      const upstream = { operation: 'responses', status: 403, code };
      expect(UpstreamRefusalSchema(upstream)).toEqual(upstream);
      expect(ProblemSchema({ ...legacy, upstream })).toEqual({ ...legacy, upstream });
    }
  });

  it('rejects malformed provider metadata', () => {
    for (const upstream of [{}, { operation: 'responses', status: '403', code: 'unrecognized' }]) {
      expect(() => ProblemSchema({ ...legacy, upstream })).toThrow();
    }
  });
});
