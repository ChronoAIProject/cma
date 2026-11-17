import { enumeration, integer, object, tagged, text, type Output } from '../../schema.js';
import { IntentReceiptSchema } from './messages.js';

const common = { preparation_id: text, expires_at: integer };
/** Native WhatsApp preparation never sends; only an explicit ready commit may send. */
const parseFormPreparationReceipt = tagged(
  {
    pending: object({
      ...common,
      status: enumeration('pending'),
      customer_message: enumeration('not_sent'),
      retry_after_ms: integer,
    }),
    ready: object({
      ...common,
      status: enumeration('ready'),
      customer_message: enumeration('not_sent'),
      preparation_ref: text,
    }),
    declined: object({
      ...common,
      status: enumeration('declined'),
      customer_message: enumeration('not_sent'),
      reason: text,
    }),
    uncertain: object({
      ...common,
      status: enumeration('uncertain'),
      customer_message: enumeration('not_sent'),
      reason: text,
    }),
    revoked: object({
      ...common,
      status: enumeration('revoked'),
      customer_message: enumeration('not_sent'),
      reason: text,
    }),
    delivery: object({
      ...common,
      status: enumeration('delivery'),
      customer_message: enumeration('attempted'),
      receipt: IntentReceiptSchema,
    }),
  },
  'status',
);
export type FormCustomerMessage = 'not_sent' | 'attempted';
export type FormPreparationState =
  | { readonly status: 'pending'; readonly retry_after_ms: number }
  | { readonly status: 'ready'; readonly preparation_ref: string }
  | { readonly status: 'declined'; readonly reason: string }
  | { readonly status: 'uncertain'; readonly reason: string }
  | { readonly status: 'revoked'; readonly reason: string }
  | { readonly status: 'delivery'; readonly receipt: Output<typeof IntentReceiptSchema> };
export type FormPreparationReceipt = FormPreparationState & {
  readonly preparation_id: string;
  readonly expires_at: number;
  readonly customer_message: FormCustomerMessage;
};
/** Validate the wire shape and the status/customer-message relationship. */
export function FormPreparationReceiptSchema(value: unknown): FormPreparationReceipt {
  return parseFormPreparationReceipt(value);
}
