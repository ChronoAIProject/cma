import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { boolean, integer, object, optional, text, type Output } from '../schema.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';

const agentPreferences = object({
  title: optional(text),
  system_prompt: text,
  prompt_pending: boolean,
  prompt_supported: boolean,
  revision: integer,
});
/** Owner-visible instructions exclude deployment-managed guidance. */
export type AgentPreferences = Output<typeof agentPreferences>;
const fields = object({
  expected_revision: integer,
  title: optional(text),
  system_prompt: optional(text),
});
const updatePreferences = (value: unknown) => {
  const result = fields(value);
  if ((result.title === undefined) === (result.system_prompt === undefined))
    throw new ProtocolError('supply exactly one of title or system_prompt');
  if (result.title !== undefined) {
    const title = result.title.trim();
    if (!title || [...title].length > 64 || /\p{Cc}/u.test(title))
      throw new ProtocolError('title must contain 1–64 characters without control characters');
  }
  if (
    result.system_prompt !== undefined &&
    (new TextEncoder().encode(result.system_prompt).length > 32768 ||
      result.system_prompt.includes('\0'))
  )
    throw new ProtocolError('system_prompt must contain at most 32 KiB without null bytes');
  return result;
};
export type UpdateAgentPreferencesRequest = Output<typeof fields>;
export function readAgentPreferences(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions,
) {
  return read(config, `/api/v1/agents/${segment(id)}/preferences`, agentPreferences, options);
}
export function changeAgentPreferences(
  config: ResolvedConfig,
  id: string,
  body: UpdateAgentPreferencesRequest,
  options: CommandOptions,
) {
  return read(config, `/api/v1/agents/${segment(id)}/preferences`, agentPreferences, {
    ...command(options),
    method: 'PATCH',
    body: input(updatePreferences, body),
    expectedStatus: 200,
  });
}
