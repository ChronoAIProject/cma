import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { input, read } from './resources.js';
import {
  array,
  boolean,
  nullable,
  object,
  optional,
  positiveInteger,
  record,
  text,
  type Output,
} from './schema.js';
import {
  agentProfileContent,
  workspaceProfileContent,
  sandboxProfileContent,
  sandboxProfileAvailability,
} from './profiles/types.js';
import { resourceOwner } from './resource-access.js';
import { botAdmissionSchema } from './bots.js';

const customization = object({
  agent: optional(nullable(agentProfileContent)),
  workspace: optional(nullable(workspaceProfileContent)),
  sandbox: optional(nullable(sandboxProfileContent)),
  availability: optional(nullable(sandboxProfileAvailability)),
});
const advance = object({
  event_ref: text,
  // Preserve the exact activity; the gateway authenticates its sealed digest.
  activity: optional(nullable(record)),
  draft: optional(
    object({ name: optional(nullable(text)), instructions: optional(nullable(text)) }),
  ),
  customization: optional(customization),
});
const configuration = object({
  agent: agentProfileContent,
  workspace: workspaceProfileContent,
  sandbox: sandboxProfileContent,
  availability: sandboxProfileAvailability,
});
const view = object({
  setup_id: text,
  revision: positiveInteger,
  step: text,
  interaction_id: text,
  identity: object({
    grant_id: nullable(text),
    subject: nullable(text),
    display_name: nullable(text),
    login_url: nullable(text),
  }),
  owner: nullable(resourceOwner),
  name: nullable(text),
  configuration,
  choices: array(object({ id: text, label: text, selected: boolean })),
  services: array(
    object({
      id: text,
      slug: text,
      name: text,
      connected: boolean,
      selected: boolean,
      required: boolean,
    }),
  ),
  connection_url: nullable(text),
  external_bot_id: nullable(text),
  admission: nullable(botAdmissionSchema),
  result: nullable(
    object({
      bot_id: text,
      state: text,
      operation: nullable(text),
      failure: nullable(text),
      sandbox_id: nullable(text),
      workspace_id: nullable(text),
      agent_id: nullable(text),
      bot_url: nullable(text),
    }),
  ),
  notice: nullable(text),
});
export type AdvanceBotSetup = Output<typeof advance>;
export type BotSetupView = Output<typeof view>;
export type BotSetupConfiguration = Output<typeof configuration>;
/** Bot Father provider workflow; requires the configured binding's agent key. */
export class BotSetups {
  constructor(private readonly config: ResolvedConfig) {}
  /** Advance only from a verified event. The returned plan still requires user confirmation. */
  advance(body: AdvanceBotSetup, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/bot-setups/advance', view, {
      ...options,
      method: 'POST',
      body: input(advance, body),
    });
  }
}
