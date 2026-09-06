import type * as Wire from '@chronoai/cma-sdk';
import type {
  ProfileContents,
  ProfileDocument,
  ProfileEditorClient,
  ProfileRevision,
  ProfileSummary,
} from '../../profiles/client';
import type { ContentCodec } from './profile-content';

export function profileSummary(value: Wire.ProfileDocument<unknown>): ProfileSummary {
  return {
    id: value.id,
    owner: value.owner,
    name: value.name,
    description: value.description,
    visibility: value.visibility,
    forkedFrom: value.forked_from,
    usesCount: value.uses_count,
    draftVersion: value.draft_version,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
    retiredAt: value.retired_at,
    latestRevision:
      value.latest_revision === undefined
        ? undefined
        : {
            revision: value.latest_revision.revision,
            digest: value.latest_revision.digest,
            publishedAt: value.latest_revision.published_at,
          },
  };
}

export function profileDocument<WireContent, Content>(
  value: Wire.ProfileDocument<WireContent>,
  decode: (value: WireContent) => Content,
): ProfileDocument<Content> {
  return { ...profileSummary(value), draft: decode(value.draft) };
}

export function profileRevision<WireContent, Content>(
  value: Wire.ProfileRevision<WireContent>,
  decode: (value: WireContent) => Content,
): ProfileRevision<Content> {
  return {
    profileId: value.profile_id,
    revision: value.revision,
    digest: value.digest,
    publishedAt: value.published_at,
    content: decode(value.content),
  };
}

export function profileEditor<Kind extends Wire.ProfileKind>(
  resource: Wire.Profiles<Kind>,
  codec: ContentCodec<Wire.ProfileContents[Kind], ProfileContents[Kind]>,
): ProfileEditorClient<ProfileContents[Kind]> {
  const document = (value: Wire.ProfileDocument<Wire.ProfileContents[Kind]>) =>
    profileDocument(value, codec.decode);
  return {
    readProfile: async (id, options) => document(await resource.read(id, options)),
    createProfile: async (input, options) =>
      document(
        await resource.create(
          {
            name: input.name,
            description: input.description,
            content: codec.encode(input.content),
          },
          options,
        ),
      ),
    saveProfile: async (id, input, options) =>
      document(
        await resource.save(
          id,
          {
            name: input.name,
            description: input.description,
            content: codec.encode(input.content),
            draft_version: input.draftVersion,
          },
          options,
        ),
      ),
    publishProfile: async (id, input, options) =>
      profileRevision(
        await resource.publish(id, { draft_version: input.draftVersion }, options),
        codec.decode,
      ),
    setProfileVisibility: async (id, input, options) =>
      document(await resource.setVisibility(id, input, options)),
    retireProfile: async (id, options) => document(await resource.retire(id, options)),
  };
}
