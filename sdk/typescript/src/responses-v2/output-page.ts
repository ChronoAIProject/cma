import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read, segment } from '../resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';
import { v2OutputItem } from './types.js';

const outputPage = object({
  object: enumeration('cma.response.output_page'),
  response_id: text,
  data: array(v2OutputItem),
  has_more: boolean,
  next_after: nullable(text),
  through: optional(integer),
});
export type V2OutputPage = Output<typeof outputPage>;
export interface V2OutputPageOptions extends OperationOptions {
  /** Opaque continuation returned for this response. */
  readonly after?: string;
}

export async function readOutputPage(
  config: ResolvedConfig,
  id: string,
  options: V2OutputPageOptions,
) {
  const page = await read(
    config,
    query(`/api/v2/responses/${segment(id)}/output`, { after: options.after }),
    outputPage,
    options,
  );
  if (
    page.response_id !== id ||
    page.has_more !== (page.next_after !== null) ||
    (page.has_more &&
      (!page.next_after || page.next_after === options.after || page.data.length === 0))
  )
    throw new ProtocolError('Response output page identity or continuation is inconsistent');
  return page;
}
