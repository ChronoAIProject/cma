import { expect, it } from 'vitest';
import { summarizeTools, conciseToolError } from './toolSummary';
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

it('summarizes searches and folder listings independently from file reads and unknown kinds', () => {
  const call = (kind: string, title: string) => ({ id: title, kind, title, status: 'completed' });
  expect(
    summarizeTools([
      call('search', 'rg token'),
      call('search', 'find definition'),
      call('read', 'List files'),
      call('read', 'app.js'),
      call('future', 'Custom task'),
    ]).label,
  ).toBe('Read 1 file · Searched 2 times · Listed 1 folder · Custom task');
});

it('never reconstructs exact totals from a degraded diff body', () => {
  const text =
    'File changes: 1 counts unavailable\nFile: "/home/cma/app.js" counts unavailable\n@@ -1 +1 @@\n-before\n+after';
  expect(summarizeTools([edit(text)])).toMatchObject({
    label: 'Edited app.js',
    added: undefined,
    removed: undefined,
    countsUnavailable: true,
  });
});

it('bounds the final error lines after retaining a reported exit code', () => {
  const text = 'exit code 17\n' + 'earlier\n'.repeat(100) + ('x'.repeat(1000) + '\n').repeat(3);
  const error = conciseToolError(edit(text));
  expect(error).toContain('Exit code 17');
  expect(error.split('\n')).toHaveLength(3);
  expect(error.split('\n').every((line) => line.length <= 240)).toBe(true);
});

it('explains missing historical deliverable results without claiming a retry is safe', () => {
  expect(
    conciseToolError({
      id: 'delivery',
      title: 'mcp.cma_access.describe_deliverable',
      status: 'failed',
    }),
  ).toContain('result_projection_unavailable');
});
