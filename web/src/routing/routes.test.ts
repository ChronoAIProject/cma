import { describe, expect, it } from 'vitest';
import { parseRoute, routePath } from './routes';

describe('the temporary route table', () => {
  it('recognizes only the retained home route', () => {
    expect(parseRoute('/')).toEqual({ kind: 'home' });
    expect(parseRoute('/removed')).toEqual({ kind: 'not_found', pathname: '/removed' });
  });

  it('preserves an unknown path for rendering and browser history', () => {
    expect(routePath({ kind: 'not_found', pathname: '/unknown' })).toBe('/unknown');
  });
});
