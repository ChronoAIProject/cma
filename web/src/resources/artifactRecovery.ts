import type { ResourceViewsClient } from './client';
import { ArtifactPublicationRecovery } from './PublishArtifact';

/** Mounted viewer custody; a different Console cannot adopt another viewer's approval. */
export class ArtifactPublicationScope {
  #clients = new WeakMap<ResourceViewsClient, Map<string, ArtifactPublicationRecovery>>();
  for(client: ResourceViewsClient, id: string): ArtifactPublicationRecovery {
    let values = this.#clients.get(client);
    if (values === undefined) {
      values = new Map();
      this.#clients.set(client, values);
    }
    let value = values.get(id);
    if (value === undefined) {
      value = new ArtifactPublicationRecovery();
      values.set(id, value);
    }
    return value;
  }
}
