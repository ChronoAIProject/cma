import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  object,
  optional,
  record,
  text,
  type Output,
} from '../schema.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';

const agentSkill = object({
  id: text,
  name: optional(text),
  version: text,
  source: enumeration('ornn'),
  status: enumeration('pending', 'installed', 'failed'),
  error: optional(text),
});
export type AgentSkill = Output<typeof agentSkill>;
const agentSkills = object({
  items: array(agentSkill),
  pending: boolean,
  can_install: boolean,
  unavailable_reason: optional(text),
});
/** Runtime-confirmed inventory. Pending/failed upgrades retain the previous effective version. */
export type AgentSkills = Output<typeof agentSkills>;

const installRequest = object({
  id: (value: unknown) => {
    const id = text(value);
    if (
      !/^[a-zA-Z0-9_-][a-zA-Z0-9_.-]{0,63}$/.test(id) ||
      id === 'nyxid' ||
      id === 'ornn-agent-manual-cli'
    )
      throw new ProtocolError('expected an installable skill identity');
    return id;
  },
  version: (value: unknown) => {
    const version = text(value);
    if (
      !version ||
      version.startsWith('@') ||
      version.trim() !== version ||
      new TextEncoder().encode(version).length > 256 ||
      /\p{Cc}/u.test(version)
    )
      throw new ProtocolError('expected an exact skill version of at most 256 bytes');
    return version;
  },
});
/** Install or upgrade an Ornn skill to an exact version on a capable idle Agent. */
export type InstallAgentSkillRequest = Output<typeof installRequest>;

function installAgentSkillRequest(value: unknown): InstallAgentSkillRequest {
  if (Object.keys(record(value)).some((key) => key !== 'id' && key !== 'version'))
    throw new ProtocolError('unknown skill installation field');
  return installRequest(value);
}

export function readAgentSkills(config: ResolvedConfig, id: string, options: OperationOptions) {
  return read(config, `/api/v1/agents/${segment(id)}/skills`, agentSkills, options);
}

export function installAgentSkill(
  config: ResolvedConfig,
  id: string,
  body: InstallAgentSkillRequest,
  options: CommandOptions,
) {
  return read(config, `/api/v1/agents/${segment(id)}/skills`, agentSkills, {
    ...command(options),
    method: 'POST',
    body: input(installAgentSkillRequest, body),
    expectedStatus: 202,
  });
}
