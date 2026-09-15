import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
} from './resources.js';
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
  type Output,
} from './schema.js';

export const organizationRole = enumeration('admin', 'member', 'viewer');
export type OrganizationRole = Output<typeof organizationRole>;
export const organizationChoice = object({
  id: text,
  role: organizationRole,
  display_name: optional(text),
});
export type OrganizationChoice = Output<typeof organizationChoice>;

export const shareMode = enumeration('view', 'org_roles');
export const createShareRequest = object({ target_org_id: text, mode: shareMode });
export const updateShareRequest = object({ expected_revision: positiveInteger, mode: shareMode });
export type CreateShareRequest = Output<typeof createShareRequest>;
export type UpdateShareRequest = Output<typeof updateShareRequest>;
export const resourceOwner = tagged(
  {
    person: object({ kind: enumeration('person'), subject: text }),
    organization: object({ kind: enumeration('organization'), subject: text }),
  },
  'kind',
);
export const resourceActor = tagged(
  {
    human: object({ kind: enumeration('human'), subject: text }),
    api: object({ kind: enumeration('api'), subject: text, delegation_id: nullable(text) }),
    trigger: object({ kind: enumeration('trigger'), subject: text, launch_id: text }),
    channel: object({
      kind: enumeration('channel'),
      bridge_id: text,
      grant_id: text,
      platform: text,
      chat_id: text,
      sender_id: text,
    }),
  },
  'kind',
);
export const resourceSelector = tagged(
  {
    agent: object({ kind: enumeration('agent'), agent_id: text }),
    sandbox: object({ kind: enumeration('sandbox'), sandbox_id: text }),
  },
  'kind',
);
export const resourceShare = object({
  id: text,
  owner: resourceOwner,
  selector: resourceSelector,
  target_org_id: text,
  mode: shareMode,
  issuer_actor: resourceActor,
  created_at: text,
  revoked_at: nullable(text),
  revision: positiveInteger,
});
export const shareReceipt = object({
  operation_id: text,
  share: resourceShare,
  actor: resourceActor,
  at: text,
  policy_revision: positiveInteger,
});
/** Safe owner attribution; personal identities and ancestry are never exposed. */
export const resourceOwnership = tagged(
  {
    person: object({ kind: enumeration('person'), is_current_person: boolean }),
    organization: object({ kind: enumeration('organization'), organization_id: text }),
  },
  'kind',
);
export type ResourceOwnership = Output<typeof resourceOwnership>;
export const effectiveAccess = object({
  owner: optional(resourceOwnership),
  capabilities: array(text),
  revalidate_at: nullable(text),
  policy_revision: integer,
});
export type ResourceShare = Output<typeof resourceShare>;
export type ShareReceipt = Output<typeof shareReceipt>;
export type EffectiveAccess = Output<typeof effectiveAccess>;

/** Policy inspection and owner share management; a permission is not a later-operation credential. */
export class ResourceAccess {
  constructor(private readonly config: ResolvedConfig) {}
  /** Current organization choices; subsequent resource requests recheck access. */
  organizations(options: OperationOptions = {}) {
    return read(
      this.config,
      '/api/v1/organizations',
      object({ items: array(organizationChoice) }),
      options,
    );
  }
  effective(kind: string, id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/resource-access/${segment(kind)}/${segment(id)}`,
      effectiveAccess,
      options,
    );
  }
  listShares(kind: string, id: string, options: OperationOptions = {}) {
    return read(this.config, this.path(kind, id), object({ items: array(resourceShare) }), options);
  }
  readShare(kind: string, id: string, shareId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      this.path(kind, id, shareId),
      identity(resourceShare, shareId),
      options,
    );
  }
  createShare(kind: string, id: string, body: CreateShareRequest, options: CommandOptions) {
    return read(this.config, this.path(kind, id), shareReceipt, {
      ...command(options),
      method: 'POST',
      body: input(createShareRequest, body),
    });
  }
  updateShare(
    kind: string,
    id: string,
    shareId: string,
    body: UpdateShareRequest,
    options: CommandOptions,
  ) {
    return read(this.config, this.path(kind, id, shareId), shareReceipt, {
      ...command(options),
      method: 'PATCH',
      body: input(updateShareRequest, body),
    });
  }
  revokeShare(
    kind: string,
    id: string,
    shareId: string,
    expectedRevision: number,
    options: CommandOptions,
  ) {
    input(positiveInteger, expectedRevision);
    return read(
      this.config,
      query(this.path(kind, id, shareId), { expected_revision: expectedRevision }),
      shareReceipt,
      { ...command(options), method: 'DELETE' },
    );
  }
  private path(kind: string, id: string, shareId?: string) {
    return `/api/v1/resource-shares/${segment(kind)}/${segment(id)}${shareId === undefined ? '' : `/${segment(shareId)}`}`;
  }
}
