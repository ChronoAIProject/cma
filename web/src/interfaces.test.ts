import { describe, expect, it } from 'vitest';
import { interfaceCatalogue } from './interfaces';

describe('interfaceCatalogue', () => {
  it('lists the five Layer 1 interfaces', () => {
    expect(interfaceCatalogue).toHaveLength(5);
  });

  it('is ordered by ascending priority', () => {
    const priorities = interfaceCatalogue.map((i) => i.priority);
    expect(priorities).toEqual([...priorities].sort((a, b) => a - b));
  });

  it('has unique ids', () => {
    const ids = new Set(interfaceCatalogue.map((i) => i.id));
    expect(ids.size).toBe(interfaceCatalogue.length);
  });
});
