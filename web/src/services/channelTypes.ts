export interface Channel {
  native_discord_activation?: {
    state: string;
    detail: string;
    capacity: number;
    selected_records: number;
  };
  created_at?: string;
  owner?: unknown;
  channel_id: string;
  record_id: string;
  version: number;
  provider: { slug: string; level: string };
  definition: {
    [key: string]: unknown;
    name: string;
    sources: unknown[];
    creation?: unknown;
    schema_version?: number;
    profile?: unknown;
    provider?: {
      [key: string]: unknown;
      slug?: string;
      binding_id?: string;
      target?: { type: 'profile' } | { type: 'existing_agent'; agent_id: string };
    };
    partition?: unknown;
    reply?: unknown;
    routing?: unknown;
    connection_ref?: string;
    durable_custom_execution?: boolean;
  };
  endpoints: {
    custom_event_url: string;
    nyxid_callback_url: string;
    telegram_webhook_url?: string;
    whatsapp_webhook_url?: string;
    native_lark_webhook_url?: string;
  };
}
export type ChannelListItem = Omit<Channel, 'channel_id' | 'endpoints'> & {
  readonly channel_id?: string | null;
  readonly endpoints?: Channel['endpoints'] | null;
};
export function hasChannelAddress(channel: ChannelListItem): channel is Channel {
  return (
    typeof channel.channel_id === 'string' &&
    channel.channel_id.length > 0 &&
    channel.endpoints != null &&
    typeof channel.endpoints.custom_event_url === 'string' &&
    typeof channel.endpoints.nyxid_callback_url === 'string'
  );
}
