import type * as Wire from '@cma/client';
import type {
  AgentProfileContent,
  SandboxProfileContent,
  WorkspaceProfileContent,
} from '../../profiles/client';

export interface ContentCodec<WireContent, Content> {
  readonly decode: (value: WireContent) => Content;
  readonly encode: (value: Content) => WireContent;
}

export const agentContent: ContentCodec<Wire.AgentProfileContent, AgentProfileContent> = {
  decode: (value) => ({
    model: value.model,
    thinkingEffort: value.thinking_effort,
    permissionLevel: value.permission_level,
    skills: value.skills,
    systemPrompt: value.system_prompt,
  }),
  encode: (value) => ({
    model: value.model,
    thinking_effort: value.thinkingEffort,
    permission_level: value.permissionLevel,
    skills: value.skills,
    system_prompt: value.systemPrompt,
  }),
};

export const workspaceContent: ContentCodec<Wire.WorkspaceProfileContent, WorkspaceProfileContent> =
  {
    decode: (value) => ({
      location: value.location,
      name: value.name,
      setupCommands: value.setup_commands,
      agentProfiles: value.agent_profiles,
    }),
    encode: (value) => ({
      location: value.location,
      name: value.name,
      setup_commands: value.setupCommands,
      agent_profiles: value.agentProfiles.map(({ id, revision }) => ({ id, revision })),
    }),
  };

export const sandboxContent: ContentCodec<Wire.SandboxProfileContent, SandboxProfileContent> = {
  decode: (value) => ({
    cpuMillis: value.cpu_millis,
    memoryMib: value.memory_mib,
    nyxidServices: value.nyxid_services,
    agentProvider: value.agent_provider,
    modelAccess: value.model_access,
    maxWorkspaces: value.max_workspaces,
    maxAgentsPerWorkspace: value.max_agents_per_workspace,
    workspaceProfiles: value.workspace_profiles,
    softwareInstallCommands: value.software_install_commands,
    environmentVariables: value.environment_variables,
  }),
  encode: (value) => ({
    cpu_millis: value.cpuMillis,
    memory_mib: value.memoryMib,
    nyxid_services: value.nyxidServices,
    agent_provider: value.agentProvider,
    model_access: value.modelAccess,
    max_workspaces: value.maxWorkspaces,
    max_agents_per_workspace: value.maxAgentsPerWorkspace,
    workspace_profiles: value.workspaceProfiles.map(({ id, revision }) => ({ id, revision })),
    software_install_commands: value.softwareInstallCommands,
    environment_variables: value.environmentVariables,
  }),
};
