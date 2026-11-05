import { expect, it } from 'vitest';
import { agentNameFor, parseRepository, validRepository, validRepositoryRef } from './repository';

it.each([
  ['owner/repo', 'https://github.com/owner/repo'],
  ['github.com/owner/repo', 'https://github.com/owner/repo'],
  ['https://github.com/owner/repo', 'https://github.com/owner/repo'],
  ['https://github.com/owner/repo.git/', 'https://github.com/owner/repo'],
  ['owner/repo.git', 'https://github.com/owner/repo'],
  ['owner/repo/', 'https://github.com/owner/repo'],
  [' github.com/owner/repo.git/ ', 'https://github.com/owner/repo'],
  ['a-b_1.c/repo.name_2-3', 'https://github.com/a-b_1.c/repo.name_2-3'],
])('normalizes %s to %s', (input, canonical) => {
  expect(parseRepository(input)).toBe(canonical);
  expect(validRepository(canonical)).toBe(true);
  expect(agentNameFor(canonical)).toBe(canonical.split('/').at(-1));
});

it.each([
  '',
  'owner',
  'owner/',
  '/owner/repo',
  'owner/repo/extra',
  'owner/repo//',
  'http://github.com/owner/repo',
  'https://www.github.com/owner/repo',
  'www.github.com/owner/repo',
  'https://gitlab.com/owner/repo',
  'https://github.com:443/owner/repo',
  'https://user@github.com/owner/repo',
  'owner/repo?ref=main',
  'owner/repo#readme',
  'owner/r epo',
  'owner/r%20epo',
  './repo',
  '../repo',
  '.git/repo',
  'owner/.',
  'owner/..',
  'owner/.git',
  'owner/.git.git',
  'owner/..git',
  'owner/...git',
  'owner/repo/../other',
])('rejects ambiguous or invalid repository input %s', (input) => {
  expect(parseRepository(input)).toBeUndefined();
});

it('keeps the existing optional HTTPS validation and naming contract', () => {
  expect(validRepository('')).toBe(true);
  expect(validRepository('owner/repo')).toBe(false);
  expect(validRepository('https://github.com/owner/repo.git/')).toBe(true);
  expect(agentNameFor('')).toBe('New agent');
  expect(agentNameFor('https://github.com/owner/repo.git/')).toBe('repo');
});

it.each(['main', 'feature/branch', 'v1.2.3', 'a'.repeat(255)])('accepts ref %s', (ref) => {
  expect(validRepositoryRef(ref)).toBe(true);
});
it.each([
  '-main',
  'two words',
  'a..b',
  'a@{b',
  'a~b',
  'a^b',
  'a:b',
  'a?b',
  'a*b',
  'a[b',
  'a\\b',
  '\u007f',
  '你'.repeat(86),
])('rejects invalid ref %s', (ref) => {
  expect(validRepositoryRef(ref)).toBe(false);
});
