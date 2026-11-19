import { expect, it } from 'vitest';
import { homeFilePath } from '../fileChanges';
import { absoluteFilePath, agentWorkingDirectory } from './filePath';
import { AGENT_DETAIL } from '../../test/agent';

it('recognizes /home metadata as the authorized root', () => {
  expect(homeFilePath('/home')).toBe('');
});

it.each([
  ['/home/', '', '/home/'],
  [
    '/home/cma/project/.worktrees/wt-fixture',
    'cma/project/.worktrees/wt-fixture',
    '/home/cma/project/.worktrees/wt-fixture',
  ],
  ['/home/cma/space here/笔记.md', 'cma/space here/笔记.md', '/home/cma/space here/笔记.md'],
  ['/home/cma/./project//note.md', 'cma/project/note.md', '/home/cma/project/note.md'],
  ['/home/cma/project/../note.md', 'cma/note.md', '/home/cma/note.md'],
  ['/home/cma/..', '', '/home/'],
])(
  'maps %s to a relative API path and one absolute display path',
  (metadata, relative, absolute) => {
    expect(homeFilePath(metadata)).toBe(relative);
    expect(absoluteFilePath(relative)).toBe(absolute);
  },
);

it.each([
  '/home/../outside',
  '/home/cma/../../outside',
  '/homeward/file',
  '/tmp/file',
  '',
  'relative',
  '/home/a\\b',
  '/home/a\nb',
])('rejects invalid working metadata %j', (path) => {
  expect(homeFilePath(path)).toBeUndefined();
});

it.each(['/home/note.md', '/tmp/note.md', '../note.md', 'a/../../note.md', 'a\\b', 'a\nb'])(
  'does not manufacture an absolute target from %j',
  (path) => {
    expect(absoluteFilePath(path)).toBeUndefined();
  },
);

it('normalizes safe relative segments without quoting spaces or Unicode', () => {
  expect(absoluteFilePath('cma/./project/../space here/笔记.md')).toBe(
    '/home/cma/space here/笔记.md',
  );
});

it('prefers conversation, then detail metadata, then worktree without inventing missing metadata', () => {
  const detail = {
    ...AGENT_DETAIL,
    runtimeMetadata: { workingDirectory: '/home/detail' },
    worktree: { ...AGENT_DETAIL.worktree, path: '/home/fallback' },
  };
  expect(agentWorkingDirectory('/home/conversation', detail)).toBe('/home/conversation');
  expect(agentWorkingDirectory(undefined, detail)).toBe('/home/detail');
  expect(agentWorkingDirectory(undefined, { ...detail, runtimeMetadata: undefined })).toBe(
    '/home/fallback',
  );
  expect(agentWorkingDirectory()).toBeUndefined();
  expect(agentWorkingDirectory('', detail)).toBe('');
  expect(agentWorkingDirectory('/outside', detail)).toBe('/outside');
});
