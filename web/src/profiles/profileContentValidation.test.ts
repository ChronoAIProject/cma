import { expect, it } from 'vitest';
import { PROFILE_BOUNDS, SANDBOX_PROFILE, WORKSPACE_PROFILE } from '../test/profiles';
import { validateSandboxProfile } from './sandboxValidation';
import { validateWorkspaceProfile } from './workspaceValidation';

it('rejects invalid repository authorities, names, references and command rows independently', () => {
  const errors = validateWorkspaceProfile({
    name: '',
    description: 'a'.repeat(2001),
    content: {
      ...WORKSPACE_PROFILE.draft,
      location: {
        type: 'git_repository',
        url: 'https://github.com:443/example/repo',
        reference: 'invalid ref',
      },
      name: '../escape',
      setupCommands: ['', 'a'.repeat(4097)],
      agentProfiles: [{ id: 'agent', revision: 0 }],
    },
  });
  expect(Object.keys(errors).sort()).toEqual([
    'agentProfiles',
    'description',
    'directoryName',
    'name',
    'repositoryReference',
    'repositoryUrl',
    'setupCommands.0',
    'setupCommands.1',
  ]);
});

it('rejects fractional resource units, duplicate or reserved variables and invalid limits together', () => {
  const errors = validateSandboxProfile(
    {
      name: 'Sandbox',
      description: '',
      content: {
        ...SANDBOX_PROFILE.draft,
        cpuMillis: 100.5,
        memoryMib: 128.5,
        maxWorkspaces: 0,
        maxAgentsPerWorkspace: 17,
        environmentVariables: [
          { name: 'RUN_MODE', value: 'a' },
          { name: 'RUN_MODE', value: 'b' },
          { name: 'PATH', value: 'c' },
        ],
      },
    },
    PROFILE_BOUNDS,
  );
  expect(Object.keys(errors).sort()).toEqual([
    'cpuMillis',
    'environmentVariables.1.name',
    'environmentVariables.2.name',
    'maxAgentsPerWorkspace',
    'maxWorkspaces',
    'memoryMib',
    'workspaceProfiles',
  ]);
});

it('preserves complete multiline commands for authoritative server validation', () => {
  const content = {
    ...WORKSPACE_PROFILE.draft,
    setupCommands: ["cat <<'END'\nplain instructions\nEND"],
  };
  expect(validateWorkspaceProfile({ name: 'Workspace', description: '', content })).toEqual({});
  expect(content.setupCommands[0]).toContain('\n');
});
