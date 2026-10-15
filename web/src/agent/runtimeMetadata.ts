import type { AgentRuntimeMetadata } from '../api/types';

export function descriptionSource(value: unknown): AgentRuntimeMetadata['descriptionSource'] {
  return value === 'provider_title' || value === 'prompt_preview' ? value : undefined;
}

/** A title and its provenance travel together; a preview cannot downgrade an explicit title. */
export function mergeRuntimeMetadata(
  previous: AgentRuntimeMetadata | undefined,
  incoming: AgentRuntimeMetadata | undefined,
): AgentRuntimeMetadata | undefined {
  if (incoming === undefined) return previous;
  const description =
    incoming.description === undefined ||
    (previous?.descriptionSource === 'provider_title' &&
      incoming.descriptionSource !== 'provider_title') ||
    (incoming.description === previous?.description &&
      incoming.descriptionSource === undefined &&
      previous?.descriptionSource !== undefined)
      ? previous
      : incoming;
  const result: AgentRuntimeMetadata = {
    workingDirectory: incoming.workingDirectory ?? previous?.workingDirectory,
    description: description?.description,
    descriptionSource: description?.descriptionSource,
    descriptionIsSetupPreview:
      description?.descriptionIsSetupPreview ??
      (description?.description === previous?.description &&
      description?.descriptionSource === previous?.descriptionSource
        ? previous?.descriptionIsSetupPreview
        : undefined),
  };
  return result.workingDirectory === previous?.workingDirectory &&
    result.description === previous?.description &&
    result.descriptionSource === previous?.descriptionSource &&
    result.descriptionIsSetupPreview === previous?.descriptionIsSetupPreview
    ? previous
    : result;
}
