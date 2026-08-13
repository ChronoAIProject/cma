import { describe, expect, it } from 'vitest';
import { createMockProfileStore } from './mock';
import { emptyDraft } from './types';

function draft(name: string) {
  return { ...emptyDraft(), name };
}

describe('the in-memory profile store', () => {
  it('stores and lists a profile', async () => {
    const store = createMockProfileStore();
    const created = await store.create(draft('build tools'));

    expect(created.id).toMatch(/^envp_/);
    expect(created.name).toBe('build tools');
    expect(await store.list()).toHaveLength(1);
  });

  it('adds the mandatory entries to a caller who omitted them', async () => {
    // The guarantee the server makes, mirrored here so a regression in the
    // editor cannot pass unnoticed.
    const store = createMockProfileStore();
    const created = await store.create(draft('bare'));

    expect(created.packages.map((p) => p.name)).toContain('nyxid');
    expect(created.skills).toContain('nyxid');
    expect(created.skills).toContain('ornn-agent-manual-cli');
    expect(created.mandatory.skills).toContain('ornn-agent-manual-cli');
  });

  it('puts the mandatory entries back when a caller strips them out', async () => {
    const store = createMockProfileStore();
    const created = await store.create({
      ...draft('mine'),
      skills: ['chosen'],
      packages: [{ name: 'ripgrep' }],
    });

    const stripped = await store.update(created.id, {
      ...draft('mine'),
      skills: [],
      packages: [],
    });
    expect(stripped.skills).toContain('nyxid');
    expect(stripped.packages.map((p) => p.name)).toContain('nyxid');
  });

  it('keeps a caller pin of a mandatory package rather than duplicating it', async () => {
    const store = createMockProfileStore();
    const created = await store.create({
      ...draft('pinned'),
      packages: [{ name: 'nyxid', version: '0.10.0' }],
    });

    const nyxid = created.packages.filter((p) => p.name === 'nyxid');
    expect(nyxid).toHaveLength(1);
    expect(nyxid[0]?.version).toBe('0.10.0');
  });

  it('replaces contents on update but keeps identity and creation time', async () => {
    const store = createMockProfileStore();
    const created = await store.create({
      ...draft('first'),
      variables: [{ key: 'ONE', value: '1' }],
    });

    const updated = await store.update(created.id, {
      ...draft('second'),
      variables: [{ key: 'TWO', value: '2' }],
    });
    expect(updated.id).toBe(created.id);
    expect(updated.createdAt).toBe(created.createdAt);
    expect(updated.name).toBe('second');
    expect(updated.variables).toEqual([{ key: 'TWO', value: '2' }]);
  });

  it('removes a profile', async () => {
    const store = createMockProfileStore();
    const created = await store.create(draft('mine'));

    await store.remove(created.id);
    expect(await store.list()).toHaveLength(0);
  });

  it('reports an update or removal of something that is gone', async () => {
    const store = createMockProfileStore();
    await expect(store.update('envp_missing', draft('x'))).rejects.toThrow(/no longer exists/i);
    await expect(store.remove('envp_missing')).rejects.toThrow(/no longer exists/i);
  });

  it('lists the skills a user may select, and what is mandatory', async () => {
    const { skills, mandatory } = await createMockProfileStore().catalogue();
    expect(skills.length).toBeGreaterThan(0);
    expect(skills.map((s) => s.id)).toContain('nyxid');
    // Every visibility a user has: their own, shared, and public.
    expect(skills.map((s) => s.access)).toContain('owner');
    expect(skills.map((s) => s.access)).toContain('shared-via-org');
    // Stated before any profile exists, so a first form is honest about what
    // it will carry.
    expect(mandatory.skills).toContain('ornn-agent-manual-cli');
    expect(mandatory.packages).toContain('nyxid');
  });

  it('fails every call when told to, so error paths can be exercised', async () => {
    const store = createMockProfileStore({ failWith: 'The control plane is unreachable.' });
    await expect(store.list()).rejects.toThrow('The control plane is unreachable.');
    await expect(store.create(draft('x'))).rejects.toThrow();
    await expect(store.catalogue()).rejects.toThrow();
  });

  it('can fail the registry alone, which is the recoverable case', async () => {
    const store = createMockProfileStore({ failSkillsWith: 'The registry is unreachable.' });
    await expect(store.catalogue()).rejects.toThrow('The registry is unreachable.');
    // Everything else still works: a profile saves without a skill picker.
    await expect(store.create(draft('mine'))).resolves.toBeTruthy();
  });

  it('starts with the profiles it was seeded with, newest first', async () => {
    const store = createMockProfileStore({ initial: [draft('older'), draft('newer')] });
    const listed = await store.list();
    expect(listed.map((p) => p.name)).toEqual(['newer', 'older']);
  });
});
