import type { AgentDetail } from '../../api/types';
import { homeFilePath } from '../fileChanges';

export function absoluteFilePath(relative: string): string | undefined {
  if (relative.startsWith('/')) return undefined;
  const path = homeFilePath(relative, '/home');
  return path === undefined ? undefined : `/home/${path}`;
}

export function agentWorkingDirectory(
  conversationDirectory?: string,
  detail?: Pick<AgentDetail, 'runtimeMetadata' | 'worktree'>,
): string | undefined {
  return (
    conversationDirectory ?? detail?.runtimeMetadata?.workingDirectory ?? detail?.worktree.path
  );
}
