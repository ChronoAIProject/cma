import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { array, boolean, object, text, type Output } from '../schema.js';
import { read, segment } from '../resources.js';

const agentModel = object({ id: text, label: text, available: boolean });
/** An admitted model; available also reflects the Agent's current runtime support. */
export type AgentModel = Output<typeof agentModel>;
const agentModels = object({ models: array(agentModel) });
export type AgentModels = Output<typeof agentModels>;

export function listAgentModels(config: ResolvedConfig, id: string, options: OperationOptions) {
  return read(config, `/api/v1/agents/${segment(id)}/models`, agentModels, options);
}
