import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import type { OperationOptions } from './http.js';
import { command, input, query, read, segment, type CommandOptions } from './resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  variant,
  type Output,
} from './schema.js';

const profile = object({ id: text, revision: positiveInteger });
const image = object({ id: text, version: optional(text) });
const repository = object({ url: text, ref: optional(text) });
const inlineSkill = object({ id: text, version: optional(text) });
const inlineAgent = object({
  model: optional(text),
  reasoning_effort: optional(text),
  permission_mode: optional(enumeration('guarded', 'autonomous')),
  system_prompt: optional(text),
  skills: array(inlineSkill),
  permissions: object({ services: array(text) }),
});
const identityPolicy = tagged({
  deny: variant('deny', {}),
  open: variant('open', {}),
  allowlist: variant('allowlist', { ids: array(text) }),
});
const admission = tagged({
  all: variant('all', {}),
  allowlist: variant('allowlist', { sender_ids: array(text) }),
  scoped: variant('scoped', {
    senders: identityPolicy,
    chats: identityPolicy,
    groups: enumeration('deny', 'all', 'mention', 'reply_to_bot', 'mention_or_reply_to_bot'),
  }),
});
export { admission as botAdmissionSchema };
const createBotFields = object({
  name: text,
  nyxid_bot_id: optional(text),
  connection_ref: optional(text),
  image: optional(image),
  repository: optional(repository),
  agent: optional(inlineAgent),
  profile: optional(profile),
  sandbox_profile: optional(profile),
  workspace_profile: optional(profile),
  event_worker_profile: optional(profile),
  admission,
  reply: object({ auto_final: boolean, ack_after_ms: integer }),
});
const createBot = (value: unknown) => {
  const parsed = createBotFields(value);
  if (Boolean(parsed.nyxid_bot_id) === Boolean(parsed.connection_ref))
    throw new ProtocolError('choose exactly one connection_ref or legacy nyxid_bot_id');
  if (Boolean(parsed.profile) === Boolean(parsed.agent))
    throw new ProtocolError('choose exactly one profile or inline agent configuration');
  return parsed;
};
const test = object({ accepted_after_ms: integer, first_text: text, level: text, tested_at: text });
const bot = object({
  bot_id: text,
  owner: text,
  name: text,
  nyxid_bot_id: text,
  connection_ref: optional(text),
  profile,
  sandbox_profile: optional(profile),
  workspace_profile: optional(profile),
  event_worker_profile: optional(profile),
  provider: text,
  state: enumeration('creating', 'active', 'disabled', 'failed', 'deleting', 'deleted'),
  failure: nullable(text),
  operation: nullable(object({ operation: text, step: text })),
  channel: nullable(
    object({
      channel_id: text,
      record_id: text,
      version: positiveInteger,
      binding_id: text,
      level: text,
      callback_url: text,
      event_url: text,
    }),
  ),
  credentials_expire_at: nullable(text),
  created_at: text,
  updated_at: text,
  last_test: nullable(test),
});
const externalBot = object({ id: text, name: text, platform: text });
const connection = object({ connection_ref: text, label: text });
export type Bot = Output<typeof bot>;
export type CreateBot = Output<typeof createBot>;
export type BotAdmission = Output<typeof admission>;
export type BotTest = Output<typeof test>;
export type ExternalBot = Output<typeof externalBot>;
export type BotConnection = Output<typeof connection>;
export interface BotListOptions extends OperationOptions {
  readonly after?: string;
  readonly limit?: number;
}
const base = '/api/v1/bots';
/** Durable Bot management. Creation and mutations return a resource to poll with get. */
export class Bots {
  constructor(private readonly config: ResolvedConfig) {}
  create(body: CreateBot, options: CommandOptions) {
    return read(this.config, base, bot, {
      ...command(options),
      method: 'POST',
      expectedStatus: 201,
      body: input(createBot, body),
    });
  }
  list(options: BotListOptions = {}) {
    return read(
      this.config,
      query(base, { after: options.after, limit: options.limit }),
      object({ items: array(bot), next_after: nullable(text) }),
      options,
    );
  }
  get(id: string, options: OperationOptions = {}) {
    return read(this.config, `${base}/${segment(id)}`, bot, options);
  }
  available(options: OperationOptions = {}) {
    return read(this.config, `${base}/nyxid-bots`, array(externalBot), options);
  }
  connections(options: OperationOptions = {}) {
    return read(this.config, `${base}/connections`, array(connection), options);
  }
  test(id: string, options: OperationOptions = {}) {
    return read(this.config, `${base}/${segment(id)}/test`, test, { ...options, method: 'POST' });
  }
  disable(id: string, options: OperationOptions = {}) {
    return this.mutate(id, '/disable', 'POST', options);
  }
  enable(id: string, options: OperationOptions = {}) {
    return this.mutate(id, '/enable', 'POST', options);
  }
  rotateKeys(id: string, options: OperationOptions = {}) {
    return this.mutate(id, '/rotate-keys', 'POST', options);
  }
  delete(id: string, options: OperationOptions = {}) {
    return this.mutate(id, '', 'DELETE', options);
  }
  private mutate(id: string, suffix: string, method: 'POST' | 'DELETE', options: OperationOptions) {
    return read(this.config, `${base}/${segment(id)}${suffix}`, bot, {
      ...options,
      method,
      expectedStatus: 202,
    });
  }
}
