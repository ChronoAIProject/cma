import { ProtocolError } from '../errors.js';
import { record, text } from '../schema.js';

/** Complete immutable event snapshot, available only with the owning binding key. */
export interface V2EventContext {
  readonly schema_version: 1;
  readonly channel_id: string;
  readonly conversation_id: string;
  readonly event_ref: string;
  readonly activity: Readonly<Record<string, unknown>> & {
    readonly schema_version: 1;
    readonly event_id: string;
  };
}
export function v2EventContext(value: unknown): V2EventContext {
  const body = record(value);
  const activity = record(body.activity);
  const label = (value: unknown, max: number) => {
    const label = text(value);
    return (
      label.trim().length > 0 &&
      new TextEncoder().encode(label).length <= max &&
      ![...label].some(
        (char) =>
          char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159),
      )
    );
  };
  if (
    body.schema_version !== 1 ||
    activity.schema_version !== 1 ||
    Object.keys(body).some(
      (key) =>
        !['schema_version', 'channel_id', 'conversation_id', 'event_ref', 'activity'].includes(key),
    ) ||
    !label(body.channel_id, 512) ||
    !label(body.conversation_id, 512) ||
    !label(body.event_ref, 4096) ||
    !label(activity.event_id, 512) ||
    new TextEncoder().encode(JSON.stringify(value)).length > 1_048_576
  )
    throw new ProtocolError('Event context is invalid or exceeds 1 MiB');
  return value as V2EventContext;
}
