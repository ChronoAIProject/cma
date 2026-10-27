import type { CmaWebClient } from '../api/client';
import type { UploadAttachment } from './attachments';

/** Validate the public upload receipt before admitting its path to a message. */
export function agentAttachmentUploader(
  agentId: string,
  client?: CmaWebClient,
): UploadAttachment | undefined {
  if (client?.uploadAgentAttachment === undefined) return;
  return async (file, options) => {
    const uploaded = await client.uploadAgentAttachment!(agentId, file, options);
    if (uploaded.agentId !== agentId || !uploaded.path.startsWith('/home/attachments/'))
      throw new Error('The upload could not be verified for this Agent. Retry the upload.');
    return uploaded;
  };
}
