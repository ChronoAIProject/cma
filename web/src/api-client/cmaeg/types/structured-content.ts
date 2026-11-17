import { ProtocolError } from '../../errors.js';
import {
  nullable,
  object,
  optional,
  tagged,
  text,
  enumeration,
  type Output,
  type Parser,
} from '../../schema.js';

const number: Parser<number> = (value) => {
  if (typeof value !== 'number' || !Number.isFinite(value))
    throw new ProtocolError('expected a finite number');
  return value;
};

export const StructuredContentSchema = tagged(
  {
    contact: object({
      type: enumeration('contact'),
      phone_number: text,
      first_name: text,
      last_name: optional(nullable(text)),
      vcard: optional(nullable(text)),
    }),
    location: object({
      type: enumeration('location'),
      latitude: number,
      longitude: number,
      horizontal_accuracy: optional(nullable(number)),
    }),
    venue: object({
      type: enumeration('venue'),
      latitude: number,
      longitude: number,
      title: text,
      address: text,
    }),
  },
  'type',
);
export type StructuredContent = Output<typeof StructuredContentSchema>;
