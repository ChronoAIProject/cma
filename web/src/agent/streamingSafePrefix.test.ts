import { expect, it } from 'vitest';
import { streamingSafePrefix } from './answerDeliveries';

it('uses CommonMark flanking for streaming emphasis in CJK and intraword text', () => {
  for (const [input, expected] of [
    ['这是**重点', '这是'],
    ['这是**重点**内容', '这是**重点**内容'],
    ['**留统一验收**：', '**留统一验收**：'],
    ['a**b', 'a'],
    ['2 ** 3', '2 ** 3'],
    ['`**kwargs`', '`**kwargs`'],
    ['snake_case_name', 'snake_case_name'],
    ['__init__', '__init__'],
  ]) {
    expect(streamingSafePrefix(input!), input).toBe(expected);
  }
});

it('keeps intraword asterisks literal in completed paragraphs', () => {
  for (const expression of ['2**10 = 1024.', 'x**2']) {
    for (const boundary of ['\n\n', '\n \t\n', '\r\n\r\n']) {
      const input = `${expression}${boundary}Next paragraph keeps streaming`;
      expect(streamingSafePrefix(input)).toBe(input);
    }
  }
});

it('keeps unmatched emphasis literal in completed paragraphs', () => {
  for (const delimiter of ['**', '__']) {
    for (const boundary of ['\n\n', '\n \t\n', '\r\n\r\n']) {
      const input = `${delimiter}open${boundary}next`;
      expect(streamingSafePrefix(input)).toBe(input);
    }
  }
});

it('cuts only unmatched emphasis in the final paragraph', () => {
  expect(streamingSafePrefix('done.\n\n这是**重点')).toBe('done.\n\n这是');
  for (const delimiter of ['**', '__']) {
    for (const boundary of ['\n\n', '\n \t\n', '\r\n\r\n']) {
      const prefix = `${delimiter}open${boundary}done.${boundary}这是`;
      expect(streamingSafePrefix(`${prefix}**重点`)).toBe(prefix);
    }
  }
});

it('preserves UTF-16 offsets when masking an emoji code span before unfinished emphasis', () => {
  expect(streamingSafePrefix('`😀` ok **bold')).toBe('`😀` ok ');
});

it('matches backtick run widths before hiding an unclosed code span', () => {
  expect(streamingSafePrefix('``a`b`` then')).toBe('``a`b`` then');
  expect(streamingSafePrefix('text `open')).toBe('text ');
  expect(streamingSafePrefix('text `open ``')).toBe('text ');
});

it('keeps code spans within paragraphs and releases literal backticks in completed paragraphs', () => {
  for (const boundary of ['\n\n', '\n \t\n', '\r\n\r\n']) {
    const prefix = `A stray \` before${boundary}Use \`code\` now`;
    expect(streamingSafePrefix(prefix)).toBe(prefix);
    expect(streamingSafePrefix(`${prefix} **open`)).toBe(`${prefix} `);
    expect(streamingSafePrefix(`A stray \` before${boundary}Use \`open`)).toBe(
      `A stray \` before${boundary}Use `,
    );
  }
  expect(streamingSafePrefix('Use `code\non two lines` now')).toBe('Use `code\non two lines` now');
  expect(streamingSafePrefix('A stray ` before\n```\nexample\n```\nUse `code` now')).toBe(
    'A stray ` before\n```\nexample\n```\nUse `code` now',
  );
});

it('hides partial deliverable markers only on the final streaming line', () => {
  expect(streamingSafePrefix('a <\nnext')).toBe('a <\nnext');
  expect(streamingSafePrefix('a <\n<!--cma-deli')).toBe('a <\n');
});
