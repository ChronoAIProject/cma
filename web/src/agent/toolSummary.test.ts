import { expect, it } from 'vitest';
import { summarizeTools } from './toolSummary';
import type { ToolCall } from './model';

const edit = (text: string, truncated = false): ToolCall => ({
  id: 'edit',
  title: 'app.js',
  kind: 'edit',
  status: 'completed',
  output: { text, truncated },
});

it('does not turn signed ordinary edit output into invented change counts', () => {
  expect(summarizeTools([edit('+ check finished\n- previous warning resolved')])).toMatchObject({
    label: 'Edited app.js',
    added: undefined,
    removed: undefined,
  });
});

it('counts complete historical unified diffs while leaving truncated totals unknown', () => {
  const diff = '--- app.js\n+++ app.js\n@@ -1,2 +1,3 @@\n same\n-old\n+new\n+extra\n';
  expect(summarizeTools([edit(diff)])).toMatchObject({ added: 2, removed: 1 });
  expect(summarizeTools([edit(diff, true)]).added).toBeUndefined();
});

it('uses reported totals even when the retained diff body was truncated', () => {
  const output =
    'File changes: 1 +86 −14\nFile: "/home/cma/app.js" +86 −14\n@@ -1,2 +1,3 @@\n-old\n+new';
  expect(summarizeTools([edit(output, true)])).toMatchObject({
    label: 'Edited app.js',
    added: 86,
    removed: 14,
  });
});
