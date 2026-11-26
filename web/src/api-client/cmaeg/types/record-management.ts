import {
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';
import { EndpointResponseSchema } from './channel-results.js';
import { ChannelIdSchema, ConformanceLevelSchema, RecordIdSchema } from './primitives.js';
import { NyxIdIdentitySchema } from './profile.js';

export const ManagedProviderSchema = object({ slug: text, level: ConformanceLevelSchema });
export type ManagedProvider = Output<typeof ManagedProviderSchema>;

export const RecordedCreationSchema = enumeration('legacy', 'incomplete', 'complete');
export type RecordedCreation = Output<typeof RecordedCreationSchema>;

export const RecordedAddressSchema = tagged(
  {
    available: object({
      state: enumeration('available'),
      channel_id: ChannelIdSchema,
      endpoints: EndpointResponseSchema,
    }),
    not_recorded: object({ state: enumeration('not_recorded') }),
    unverifiable: object({ state: enumeration('unverifiable') }),
  },
  'state',
);
export type RecordedAddress = Output<typeof RecordedAddressSchema>;

export const RecordedOperationSchema = enumeration(
  'create',
  'update',
  'profile_revision',
  'delete',
  'audit_routing',
  'audit_attachments',
);
export type RecordedOperation = Output<typeof RecordedOperationSchema>;

export const LegacyRetirementStateSchema = enumeration(
  'available',
  'blocked_missing_credential',
  'pending',
  'cleanup_unconfirmed',
);
export type LegacyRetirementState = Output<typeof LegacyRetirementStateSchema>;

export const LegacyRetirementSchema = object({
  expected_version: integer,
  state: LegacyRetirementStateSchema,
});
export type LegacyRetirement = Output<typeof LegacyRetirementSchema>;

export const ManagedRecordSchema = tagged(
  {
    valid: object({
      state: enumeration('valid'),
      name: text,
      owner: NyxIdIdentitySchema,
      provider: ManagedProviderSchema,
      creation: RecordedCreationSchema,
      address: RecordedAddressSchema,
      pending_operation: optional(nullable(RecordedOperationSchema)),
      retirement: optional(nullable(LegacyRetirementSchema)),
      retirement_expected_version: optional(nullable(integer)),
    }),
    invalid: object({ state: enumeration('invalid') }),
  },
  'state',
);
export type ManagedRecord = Output<typeof ManagedRecordSchema>;

export const ChannelRecordManagementSchema = object({
  record_id: RecordIdSchema,
  version: integer,
  record: ManagedRecordSchema,
});
export type ChannelRecordManagement = Output<typeof ChannelRecordManagementSchema>;

export const CancelCreateCapabilitySchema = object({ contract_version: integer });
export type CancelCreateCapability = Output<typeof CancelCreateCapabilitySchema>;

export const CancelCreateReceiptOwnerSchema = object({ issuer: text, kind: text, subject: text });
export type CancelCreateReceiptOwner = Output<typeof CancelCreateReceiptOwnerSchema>;

export const CancelCreateReceiptProviderSchema = object({ slug: text, binding_id: text });
export type CancelCreateReceiptProvider = Output<typeof CancelCreateReceiptProviderSchema>;

export const CancelCreateReceiptSchema = object({
  contract_version: integer,
  status: text,
  record_id: RecordIdSchema,
  owner: CancelCreateReceiptOwnerSchema,
  provider: CancelCreateReceiptProviderSchema,
});
export type CancelCreateReceipt = Output<typeof CancelCreateReceiptSchema>;
