import type { ResolvedConfig } from './config.js';
import { request, type OperationOptions } from './http.js';
import { array, boolean, object, text, type Output } from './schema.js';

const agentCard = object({
  name: text,
  version: text,
  url: text,
  capabilities: object({
    streaming: boolean,
    activity_items: boolean,
    client_tools: boolean,
    cancel: boolean,
    rich_input: boolean,
  }),
  provider_contract: object({ binding: text, conversation: text, responses: text }),
  authentication: object({ schemes: array(text) }),
});
export type ProviderAgentCard = Output<typeof agentCard>;
/** Read public provider discovery without sending cookies or a configured credential. */
export async function readAgentCard(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<ProviderAgentCard> {
  return agentCard(
    await request(
      { ...config, credential: undefined, cookies: false },
      '/.well-known/agent-card.json',
      { ...options, expectedStatus: 200 },
    ),
  );
}
