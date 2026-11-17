import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { boolean, object, text, type Output } from '../schema.js';
import { permissionLevel, thinkingEffort } from '../profiles/types.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';

const model = (value: unknown) => {
  const result = text(value);
  if (
    result.length === 0 ||
    new TextEncoder().encode(result).length > 256 ||
    result.trim() !== result ||
    /\p{Cc}/u.test(result)
  )
    throw new ProtocolError(
      'expected a nonempty model of at most 256 bytes without control characters',
    );
  return result;
};
const updateAgentSettings = object({
  model,
  reasoning_effort: thinkingEffort,
  permission_mode: permissionLevel,
});
/** Absolute settings for an idle Agent; all fields must be supplied. */
export type UpdateAgentSettingsRequest = Output<typeof updateAgentSettings>;
const agentSettings = object({
  // Historical profiles may delegate model choice to the runtime (empty string).
  model: text,
  reasoning_effort: thinkingEffort,
  permission_mode: permissionLevel,
  pending: boolean,
});
/** Desired settings; pending means runtime acknowledgement is still outstanding. */
export type AgentSettings = Output<typeof agentSettings>;
export function readAgentSettings(config: ResolvedConfig, id: string, options: OperationOptions) {
  return read(config, `/api/v1/agents/${segment(id)}/settings`, agentSettings, options);
}
export function changeAgentSettings(
  config: ResolvedConfig,
  id: string,
  body: UpdateAgentSettingsRequest,
  options: CommandOptions,
) {
  return read(config, `/api/v1/agents/${segment(id)}/settings`, agentSettings, {
    ...command(options),
    method: 'PATCH',
    body: input(updateAgentSettings, body),
    expectedStatus: 200,
  });
}
