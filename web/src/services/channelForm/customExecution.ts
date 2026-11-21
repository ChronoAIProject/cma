import type { RelayDiscovery } from '../RelayCapabilities';
import { object, records, text, type Fields } from './model';

export function hasCustomSource(draft: Fields): boolean {
  return !draft.connection_ref && records(draft.sources).some((source) => source.type === 'custom');
}

export function customExecutionWaiting(discovery: RelayDiscovery, draft: Fields): boolean {
  return hasCustomSource(draft) && ['loading', 'failed'].includes(discovery.status);
}

export function customExecutionSelected(discovery: RelayDiscovery, draft: Fields): boolean {
  return (
    hasCustomSource(draft) &&
    discovery.status === 'ready' &&
    discovery.authoring?.custom_durable_execution_enabled === true &&
    discovery.authoring.response_recovery_providers?.includes(text(object(draft.provider).slug)) ===
      true
  );
}

/** Only submission creates the channel; discovery never admits or upgrades work. */
export function customExecutionRequest(discovery: RelayDiscovery, draft: Fields): Fields {
  if (customExecutionWaiting(discovery, draft))
    throw new Error('Check channel support before creating this channel. Your draft is retained.');
  return customExecutionSelected(discovery, draft) ? { durable_custom_execution: true } : {};
}

export function customAttachmentDescription(draft: Fields, durable: boolean): string {
  const provider = object(draft.provider);
  if (object(provider.target).type === 'existing_agent')
    return durable && provider.slug === 'cma_codex'
      ? 'Supported images, files and transcripts are delivered to the agent.'
      : 'Text and audio excerpts are supported. Image and file delivery depends on this channel’s agent connection.';
  return 'Images and files require an agent with channel attachment tools. Text and audio excerpts are supported.';
}
