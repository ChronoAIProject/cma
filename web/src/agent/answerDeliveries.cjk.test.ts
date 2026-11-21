import { expect, it } from 'vitest';
import { answerProse, streamingSafePrefix } from './answerDeliveries';

it.each([
  '**检查日期：**2026 年',
  '**检查范围：**仅对当前本地工作区',
  '**QA 结论：证据不足，暂不具备通过验收的依据。**上述为结果',
  '**English:**中文',
  '中文**(范围)**继续',
  '**𠮷。**继续',
  '**葛󠄀**继续',
  '__检查日期：__ 2026 年',
])('keeps completed CJK emphasis in live prose: %s', (content) => {
  expect(streamingSafePrefix(content)).toBe(content);
  expect(answerProse(content, true).content).toBe(content);
  expect(answerProse(content).content).toBe(content);
});

it.each([
  ['**', '2026 年'],
  ['__', ' 2026 年'],
])('retains the incomplete-emphasis guard for %s until the pair arrives', (marker, suffix) => {
  const prefix = '已完成。\n\n';
  const partial = `${prefix}${marker}检查日期：`;
  expect(answerProse(partial, true).content).toBe(prefix);
  const complete = `${partial}${marker}${suffix}`;
  expect(answerProse(complete, true).content).toBe(complete);
});
