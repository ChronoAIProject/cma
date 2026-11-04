import { useMemo } from 'react';
import type { AnswerDeliverable } from '@chronoai/cma-sdk';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { downloadFailureStatus } from './AnswerDeliverables';
import type { DeliverableSelection } from './previewSelection';
import { PREVIEW_BYTES } from '../api/agentFiles';
import { imageType } from './files/FilePreview';

export function previewFormat(item: AnswerDeliverable): 'file' | 'markdown' | 'unsupported' {
  const { name, media_type: mediaType } = item.reference;
  if (mediaType === 'application/pdf' || /\.pdf$/i.test(name)) return 'unsupported';
  if (mediaType === 'text/markdown' || /\.(md|markdown)$/i.test(name)) return 'markdown';
  if (
    imageType(name) ||
    ['text/plain', 'application/json'].includes(mediaType) ||
    !name.includes('.') ||
    /\.(txt|json|html?|svg|xml|csv|tsv|ya?ml|toml|ini|log|sh|py|js|jsx|ts|tsx|css|rs|sql)$/i.test(
      name,
    )
  )
    return 'file';
  return 'unsupported';
}

export interface PreviewValue {
  readonly status: AnswerDeliverable['status'] | 'error';
  readonly bytes?: Uint8Array;
  readonly tooLarge?: boolean;
  readonly unsupported?: boolean;
}

export function usePreviewRead(selected: DeliverableSelection) {
  const read = useMemo(
    () =>
      async (signal: AbortSignal): Promise<{ value: PreviewValue }> => {
        const { access, answerId, item } = selected;
        try {
          const current = (
            await access.client.readAnswerDeliverable(access.agentId, answerId, item.id, {
              signal,
              verify: false,
            })
          ).item;
          if (JSON.stringify(current.reference) !== JSON.stringify(item.reference))
            return { value: { status: 'changed' } };
          if (!['ready', 'unchecked', 'version_changed'].includes(current.status))
            return { value: { status: current.status } };
          if (item.reference.size_bytes > PREVIEW_BYTES)
            return { value: { status: current.status, tooLarge: true } };
          if (previewFormat(item) === 'unsupported')
            return { value: { status: current.status, unsupported: true } };
          const bytes = await access.client.downloadAnswerDeliverable(
            access.agentId,
            answerId,
            item,
            { signal },
          );
          return { value: { status: current.status, bytes } };
        } catch (error) {
          signal.throwIfAborted();
          return { value: { status: downloadFailureStatus(error) ?? 'error' } };
        }
      },
    [selected],
  );
  return useAuthorizedRead(read);
}
