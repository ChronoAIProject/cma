import { describe, expect, it } from 'vitest';
import type { AgentFilesClient } from '../api/agentFiles';
import { conversationMedia } from './mediaSources';

const files = {} as AgentFilesClient;

describe('conversationMedia', () => {
  it('offers media access with a files client once access is settled and authorized', () => {
    expect(conversationMedia('agt_1', files, true, '/home/cma/project')).toEqual({
      agentId: 'agt_1',
      files,
      workingDirectory: '/home/cma/project',
    });
  });

  it('withholds media access without a files client or authorization', () => {
    expect(conversationMedia('agt_1', undefined, true, undefined)).toBeUndefined();
    expect(conversationMedia('agt_1', files, false, undefined)).toBeUndefined();
  });
});
