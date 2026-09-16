export function resourceCount(count: number, resource: 'Agent' | 'Workspace' | 'Worktree'): string {
  return `${count} ${resource}${count === 1 ? '' : 's'}`;
}
