import { createParser, type EventSourceMessage } from 'eventsource-parser';
import {
  headersFor,
  observeResponse,
  resolveRequestId,
  urlOf,
  type ResolvedConfig,
} from './config.js';
import {
  CmaError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
import {
  errorFromResponse,
  raceWithAbortSignal,
  request,
  throwIfSignalAborted,
  type OperationOptions,
} from './http.js';

const MAX_FRAME_BYTES = 2 * 1024 * 1024;
const MAX_ITEMS_LIMIT = 100;

/** A stateful chat conversation in CMA's Responses subset. */
export interface ResponsesConversation {
  readonly id: string;
  readonly object: 'conversation';
  readonly created_at: number;
  readonly metadata: Readonly<Record<string, string>>;
}

/** Token detail fields retained by the Responses projection. */
export interface ResponseUsage {
  readonly input_tokens: number;
  readonly input_tokens_details: { readonly cached_tokens: number };
  readonly output_tokens: number;
  readonly output_tokens_details: { readonly reasoning_tokens: number };
  readonly total_tokens: number;
}

/** One completed assistant text part. */
export interface ResponseOutputText {
  readonly type: 'output_text';
  readonly text: string;
  readonly annotations: readonly unknown[];
  readonly logprobs: readonly unknown[];
}

/** One assistant message in a response, including partial durable output. */
export interface ResponseOutputMessage {
  readonly id: string;
  readonly type: 'message';
  readonly status: 'in_progress' | 'completed';
  readonly role: 'assistant';
  readonly content: readonly ResponseOutputText[];
}

/** Terminal error attached to a failed response. */
export interface ResponseError {
  readonly code: string;
  readonly message: string;
}

/** Lifecycle values emitted and reconstructed by CMA. */
export type ResponseStatus = 'in_progress' | 'completed' | 'failed' | 'incomplete';

/** A response resource in CMA's supported, stateful subset. */
export interface ResponsesResponse {
  readonly id: string;
  readonly object: 'response';
  readonly created_at: number;
  readonly status: ResponseStatus;
  readonly background: false;
  readonly error: ResponseError | null;
  readonly incomplete_details: unknown | null;
  readonly instructions: string | null;
  readonly max_output_tokens: number | null;
  readonly model: string;
  readonly output: readonly ResponseOutputMessage[];
  readonly parallel_tool_calls: false;
  readonly previous_response_id: string | null;
  readonly reasoning: Readonly<Record<string, unknown>>;
  readonly store: true;
  readonly temperature: number | null;
  readonly text: Readonly<Record<string, unknown>>;
  readonly tool_choice: 'none';
  readonly tools: readonly unknown[];
  readonly top_p: number | null;
  readonly truncation: 'disabled';
  readonly usage: ResponseUsage | null;
  readonly metadata: Readonly<Record<string, string>>;
}

/** One recorded body on CMA's namespaced tool activity item. */
export interface ResponseToolBody {
  readonly text: string;
  readonly truncated: boolean;
}

/** Durable or live tool state carried in CMA's extension lane. */
export interface ResponseToolActivity {
  readonly id: string;
  readonly type: 'cma_chat.tool_activity';
  readonly call_id: string;
  readonly kind: string;
  readonly target: string;
  readonly status: string;
  readonly duration_ms?: number | null;
  readonly arguments?: ResponseToolBody | null;
  readonly result?: ResponseToolBody | null;
}

/** Text content retained on a conversation message item. */
export interface ConversationMessageContent {
  readonly type: 'input_text' | 'output_text';
  readonly text: string;
  readonly annotations?: readonly unknown[];
  readonly logprobs?: readonly unknown[];
}

/** A standard durable user or assistant message item. */
export interface ConversationMessageItem {
  readonly id: string;
  readonly type: 'message';
  readonly status: 'completed';
  readonly role: 'user' | 'assistant';
  readonly content: readonly ConversationMessageContent[];
}

/** The supported standard and extension conversation-item union. */
export type ResponsesConversationItem = ConversationMessageItem | ResponseToolActivity;

/** OpenAI list envelope used for conversation history. */
export interface ResponsesConversationItemList {
  readonly object: 'list';
  readonly data: readonly ResponsesConversationItem[];
  readonly first_id: string | null;
  readonly last_id: string | null;
  readonly has_more: boolean;
}

/** One user text message accepted as response input. */
export interface ResponseInputMessage {
  readonly type?: 'message';
  readonly role: 'user';
  readonly content: string | readonly [{ readonly type: 'input_text'; readonly text: string }];
}

/** Request fields CMA honors or explicitly accepts on response creation. */
export interface CreateResponseRequest {
  readonly conversation: string;
  readonly input: string | readonly [ResponseInputMessage];
  readonly metadata?: Readonly<Record<string, string>>;
  readonly previous_response_id?: string | null;
  readonly model?: string;
  readonly store?: true;
}

/** Pagination fields accepted by conversation item history. */
export interface ConversationItemsRequest {
  readonly after?: string;
  readonly limit?: number;
}

interface ResponseEventBase<T extends string> {
  readonly type: T;
  readonly sequence_number: number;
}

/** Response lifecycle events carrying a response snapshot. */
export interface ResponseLifecycleEvent extends ResponseEventBase<
  | 'response.created'
  | 'response.in_progress'
  | 'response.completed'
  | 'response.failed'
  | 'response.incomplete'
> {
  readonly response: ResponsesResponse;
}

/** A live text delta for the assistant output message. */
export interface ResponseOutputTextDeltaEvent extends ResponseEventBase<'response.output_text.delta'> {
  readonly item_id: string;
  readonly output_index: number;
  readonly content_index: number;
  readonly delta: string;
  readonly logprobs: readonly unknown[];
}

/** The durable final assistant text. */
export interface ResponseOutputTextDoneEvent extends ResponseEventBase<'response.output_text.done'> {
  readonly item_id: string;
  readonly output_index: number;
  readonly content_index: number;
  readonly text: string;
  readonly logprobs: readonly unknown[];
}

/** A live reasoning-summary text delta. */
export interface ResponseReasoningDeltaEvent extends ResponseEventBase<'response.reasoning_summary_text.delta'> {
  readonly item_id: string;
  readonly output_index: number;
  readonly summary_index: number;
  readonly delta: string;
}

/** The final live reasoning-summary text. */
export interface ResponseReasoningDoneEvent extends ResponseEventBase<'response.reasoning_summary_text.done'> {
  readonly item_id: string;
  readonly output_index: number;
  readonly summary_index: number;
  readonly text: string;
}

/** CMA's namespaced live tool activity event. */
export interface ResponseToolActivityEvent extends ResponseEventBase<'response.cma_chat.tool_activity'> {
  readonly response_id: string;
  readonly item: ResponseToolActivity;
}

type StructuralResponseEventType =
  | 'response.reasoning_summary_part.added'
  | 'response.reasoning_summary_part.done'
  | 'response.output_item.added'
  | 'response.content_part.added'
  | 'response.content_part.done'
  | 'response.output_item.done';

/** A structural stream boundary whose payload remains available verbatim. */
export type ResponseStructuralEvent = ResponseEventBase<StructuralResponseEventType> &
  Readonly<Record<string, unknown>>;

/** Every event in CMA's declared Responses stream subset. */
export type ResponsesStreamEvent =
  | ResponseLifecycleEvent
  | ResponseOutputTextDeltaEvent
  | ResponseOutputTextDoneEvent
  | ResponseReasoningDeltaEvent
  | ResponseReasoningDoneEvent
  | ResponseToolActivityEvent
  | ResponseStructuralEvent;

/** Incremental state folded by a {@link ResponsesConnection}. */
export interface ResponsesConnectionState {
  readonly outputText: string;
  readonly reasoningText: string;
  readonly toolActivities: readonly ResponseToolActivity[];
  readonly response?: ResponsesResponse;
  readonly terminal?: ResponsesResponse;
}

/** Create a deployment-managed chat conversation. */
export async function createConversation(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<ResponsesConversation> {
  return toConversation(
    await request(config, '/api/v1/conversations', {
      ...options,
      method: 'POST',
      body: {},
      expectedStatus: 200,
    }),
  );
}

/** Read one caller-owned chat conversation. */
export async function readConversation(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<ResponsesConversation> {
  return toConversation(
    await request(config, `/api/v1/conversations/${encodeURIComponent(id)}`, {
      ...options,
      expectedStatus: 200,
    }),
  );
}

/** Read one newest-first page of durable conversation history. */
export async function listConversationItems(
  config: ResolvedConfig,
  id: string,
  page: ConversationItemsRequest = {},
  options: OperationOptions = {},
): Promise<ResponsesConversationItemList> {
  const params = new URLSearchParams();
  if (page.after !== undefined) {
    if (page.after === '') {
      throw new ValidationError('conversation item cursor must not be empty');
    }
    params.set('after', page.after);
  }
  if (page.limit !== undefined) {
    if (!Number.isSafeInteger(page.limit) || page.limit < 1 || page.limit > MAX_ITEMS_LIMIT) {
      throw new ValidationError('conversation item limit must be an integer from 1 through 100');
    }
    params.set('limit', String(page.limit));
  }
  const query = params.toString();
  const path = `/api/v1/conversations/${encodeURIComponent(id)}/items${query === '' ? '' : `?${query}`}`;
  return toConversationItemList(await request(config, path, { ...options, expectedStatus: 200 }));
}

/** Retrieve one response reconstructed from durable conversation history. */
export async function readResponse(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<ResponsesResponse> {
  return toResponse(
    await request(config, `/api/v1/responses/${encodeURIComponent(id)}`, {
      ...options,
      expectedStatus: 200,
    }),
  );
}

/** Admit one chat turn and open its validated SSE connection. */
export function createResponse(
  config: ResolvedConfig,
  response: CreateResponseRequest,
  options: OperationOptions = {},
): Promise<ResponsesConnection> {
  return ResponsesConnection.open(config, response, options);
}

/**
 * A single-consumer Responses stream that validates ordering and folds display state.
 *
 * Cancelling or dropping this client-side stream closes only the HTTP response
 * body. It never calls CMA's response-cancel route and does not cancel the core
 * turn, which remains recoverable through response and item retrieval.
 */
export class ResponsesConnection implements AsyncIterable<ResponsesStreamEvent> {
  readonly #response: Response;
  readonly #controller: AbortController;
  readonly #callerSignal: AbortSignal | undefined;
  readonly #onCallerAbort: (() => void) | undefined;
  #consumed = false;
  #outputText = '';
  #reasoningText = '';
  #responseState: ResponsesResponse | undefined;
  #terminal: ResponsesResponse | undefined;
  readonly #tools = new Map<string, ResponseToolActivity>();

  private constructor(
    response: Response,
    controller: AbortController,
    callerSignal: AbortSignal | undefined,
    onCallerAbort: (() => void) | undefined,
  ) {
    this.#response = response;
    this.#controller = controller;
    this.#callerSignal = callerSignal;
    this.#onCallerAbort = onCallerAbort;
  }

  static async open(
    config: ResolvedConfig,
    response: CreateResponseRequest,
    options: OperationOptions = {},
  ): Promise<ResponsesConnection> {
    validateCreateRequest(response);
    const timeoutMs = options.timeoutMs ?? config.requestTimeoutMs;
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
      throw new ValidationError('timeoutMs must be a positive safe integer');
    }
    if (options.signal?.aborted === true) {
      throw new RequestAborted(undefined, { cause: options.signal.reason });
    }

    const controller = new AbortController();
    const onCallerAbort =
      options.signal === undefined ? undefined : () => controller.abort(options.signal?.reason);
    if (onCallerAbort !== undefined) {
      options.signal?.addEventListener('abort', onCallerAbort, { once: true });
    }
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);

    try {
      const received = await raceWithAbortSignal(controller.signal, async () => {
        const requestId = await resolveRequestId(config, options.requestId);
        throwIfSignalAborted(controller.signal);
        const headers = await headersFor(config, { Accept: 'text/event-stream' }, { requestId });
        throwIfSignalAborted(controller.signal);
        const opened = await config.fetch(urlOf(config, '/api/v1/responses'), {
          method: 'POST',
          headers,
          credentials: config.cookies ? 'include' : 'omit',
          redirect: 'error',
          body: JSON.stringify({ ...response, stream: true }),
          signal: controller.signal,
        });
        throwIfSignalAborted(controller.signal);
        const metadata = await observeResponse(config, opened);
        throwIfSignalAborted(controller.signal);
        if (!opened.ok) {
          throw await errorFromResponse(opened, controller.signal, metadata.requestId);
        }
        if (opened.status !== 200) {
          throw new ProtocolError(
            `Responses stream expected HTTP 200 but received ${String(opened.status)}`,
          );
        }
        const contentType = opened.headers.get('content-type') ?? '';
        if (contentType.split(';', 1)[0]?.trim().toLowerCase() !== 'text/event-stream') {
          throw new ProtocolError(`Responses stream returned content type '${contentType}'`);
        }
        if (opened.body === null) {
          throw new ProtocolError('Responses stream response carried no body');
        }
        return opened;
      });
      clearTimeout(timer);
      return new ResponsesConnection(received, controller, options.signal, onCallerAbort);
    } catch (cause) {
      clearTimeout(timer);
      if (onCallerAbort !== undefined) {
        options.signal?.removeEventListener('abort', onCallerAbort);
      }
      controller.abort();
      if (isAborted(options.signal)) {
        throw new RequestAborted(undefined, { cause });
      }
      if (timedOut) {
        throw new TimeoutError(timeoutMs, { cause });
      }
      if (cause instanceof CmaError) {
        throw cause;
      }
      throw new TransportError(`cannot open the Responses stream: ${messageOf(cause)}`, { cause });
    }
  }

  /** Current immutable display projection. */
  get state(): ResponsesConnectionState {
    return {
      outputText: this.#outputText,
      reasoningText: this.#reasoningText,
      toolActivities: [...this.#tools.values()],
      ...(this.#responseState === undefined ? {} : { response: this.#responseState }),
      ...(this.#terminal === undefined ? {} : { terminal: this.#terminal }),
    };
  }

  /** Close this client's body reader without issuing a server-side cancel operation. */
  cancel(reason?: unknown): void {
    this.#controller.abort(reason);
  }

  /** Consume the stream completely and return its terminal folded state. */
  async fold(): Promise<ResponsesConnectionState> {
    for await (const event of this) {
      // Folding occurs before each event is yielded.
      void event;
    }
    return this.state;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ResponsesStreamEvent> {
    if (this.#consumed) {
      throw new ProtocolError('a Responses connection can be consumed only once');
    }
    this.#consumed = true;
    let expectedSequence = 0;
    try {
      for await (const raw of framesFromBody(this.#response, this.#controller.signal)) {
        const event = toResponseStreamEvent(raw);
        if (event.sequence_number !== expectedSequence) {
          throw new ProtocolError(
            `Responses stream expected sequence ${String(expectedSequence)} but received ${String(event.sequence_number)}`,
          );
        }
        expectedSequence += 1;
        this.#accept(event);
        yield event;
        if (this.#terminal !== undefined) {
          return;
        }
      }
      throw new ProtocolError('Responses stream closed before a terminal event');
    } catch (cause) {
      if (this.#controller.signal.aborted) {
        throw new RequestAborted(undefined, { cause });
      }
      if (cause instanceof CmaError) {
        throw cause;
      }
      throw new TransportError(`Responses stream failed: ${messageOf(cause)}`, { cause });
    } finally {
      if (this.#onCallerAbort !== undefined) {
        this.#callerSignal?.removeEventListener('abort', this.#onCallerAbort);
      }
      if (!this.#controller.signal.aborted) {
        this.#controller.abort();
      }
    }
  }

  #accept(event: ResponsesStreamEvent): void {
    switch (event.type) {
      case 'response.created':
      case 'response.in_progress':
        this.#responseState = event.response;
        break;
      case 'response.output_text.delta':
        this.#outputText += event.delta;
        break;
      case 'response.output_text.done':
        this.#outputText = event.text;
        break;
      case 'response.reasoning_summary_text.delta':
        this.#reasoningText += event.delta;
        break;
      case 'response.reasoning_summary_text.done':
        this.#reasoningText = event.text;
        break;
      case 'response.cma_chat.tool_activity':
        this.#tools.set(event.item.call_id, event.item);
        break;
      case 'response.completed':
      case 'response.failed':
      case 'response.incomplete':
        this.#responseState = event.response;
        this.#terminal = event.response;
        if (this.#outputText === '') {
          this.#outputText = event.response.output
            .flatMap((item) => item.content)
            .map((part) => part.text)
            .join('');
        }
        break;
      case 'response.reasoning_summary_part.added':
      case 'response.reasoning_summary_part.done':
      case 'response.output_item.added':
      case 'response.content_part.added':
      case 'response.content_part.done':
      case 'response.output_item.done':
        break;
    }
  }
}

/** Decode a complete Responses SSE fixture under the same sequence rules as the live client. */
export function parseResponsesEventStream(body: string): ResponsesStreamEvent[] {
  const raw: unknown[] = [];
  let failure: ProtocolError | undefined;
  const parser = parserFor(
    (frame) => raw.push(frame),
    (error) => {
      failure = error;
    },
  );
  try {
    parser.feed(body);
    parser.reset({ consume: true });
  } catch (cause) {
    throw new ProtocolError('Responses stream was not valid SSE', { cause });
  }
  if (failure !== undefined) {
    throw failure;
  }
  return raw.map(toResponseStreamEvent).map((event, index) => {
    if (event.sequence_number !== index) {
      throw new ProtocolError(
        `Responses stream expected sequence ${String(index)} but received ${String(event.sequence_number)}`,
      );
    }
    return event;
  });
}

/** Validate a conversation response at the SDK boundary. */
export function toConversation(raw: unknown): ResponsesConversation {
  const body = objectOf(raw, 'conversation');
  literalOf(body, 'object', 'conversation');
  return {
    id: stringOf(body, 'id'),
    object: 'conversation',
    created_at: integerOf(body, 'created_at'),
    metadata: stringsOf(body.metadata, 'conversation metadata'),
  };
}

/** Validate a response resource at the SDK boundary. */
export function toResponse(raw: unknown): ResponsesResponse {
  const body = objectOf(raw, 'response');
  literalOf(body, 'object', 'response');
  const status = stringOf(body, 'status');
  if (!['in_progress', 'completed', 'failed', 'incomplete'].includes(status)) {
    throw new ProtocolError(`response carried unknown status '${status}'`);
  }
  literalOf(body, 'background', false);
  literalOf(body, 'parallel_tool_calls', false);
  literalOf(body, 'store', true);
  literalOf(body, 'tool_choice', 'none');
  literalOf(body, 'truncation', 'disabled');
  const output = arrayOf(body.output, 'response output').map(toOutputMessage);
  const error = body.error === null ? null : toResponseError(body.error);
  const usage = body.usage === null ? null : toUsage(body.usage);
  return {
    id: stringOf(body, 'id'),
    object: 'response',
    created_at: integerOf(body, 'created_at'),
    status: status as ResponseStatus,
    background: false,
    error,
    incomplete_details: nullableField(body, 'incomplete_details'),
    instructions: nullableStringOf(body, 'instructions'),
    max_output_tokens: nullableIntegerOf(body, 'max_output_tokens'),
    model: stringOf(body, 'model'),
    output,
    parallel_tool_calls: false,
    previous_response_id: nullableStringOf(body, 'previous_response_id'),
    reasoning: objectOf(body.reasoning, 'response reasoning'),
    store: true,
    temperature: nullableNumberOf(body, 'temperature'),
    text: objectOf(body.text, 'response text configuration'),
    tool_choice: 'none',
    tools: arrayOf(body.tools, 'response tools'),
    top_p: nullableNumberOf(body, 'top_p'),
    truncation: 'disabled',
    usage,
    metadata: stringsOf(body.metadata, 'response metadata'),
  };
}

/** Validate one conversation item list. */
export function toConversationItemList(raw: unknown): ResponsesConversationItemList {
  const body = objectOf(raw, 'conversation item list');
  literalOf(body, 'object', 'list');
  return {
    object: 'list',
    data: arrayOf(body.data, 'conversation item data').map(toConversationItem),
    first_id: nullableStringOf(body, 'first_id'),
    last_id: nullableStringOf(body, 'last_id'),
    has_more: booleanOf(body, 'has_more'),
  };
}

/** Validate one Responses stream frame. */
export function toResponseStreamEvent(raw: unknown): ResponsesStreamEvent {
  const body = objectOf(raw, 'Responses event');
  const type = stringOf(body, 'type');
  const sequence = integerOf(body, 'sequence_number');
  const base = { ...body, type, sequence_number: sequence };
  switch (type) {
    case 'response.created':
    case 'response.in_progress':
    case 'response.completed':
    case 'response.failed':
    case 'response.incomplete':
      return { ...base, type, response: toResponse(body.response) } as ResponseLifecycleEvent;
    case 'response.output_text.delta':
      return {
        ...base,
        type,
        item_id: stringOf(body, 'item_id'),
        output_index: integerOf(body, 'output_index'),
        content_index: integerOf(body, 'content_index'),
        delta: stringOf(body, 'delta'),
        logprobs: arrayOf(body.logprobs, 'output delta logprobs'),
      } as ResponseOutputTextDeltaEvent;
    case 'response.output_text.done':
      return {
        ...base,
        type,
        item_id: stringOf(body, 'item_id'),
        output_index: integerOf(body, 'output_index'),
        content_index: integerOf(body, 'content_index'),
        text: stringOf(body, 'text'),
        logprobs: arrayOf(body.logprobs, 'output done logprobs'),
      } as ResponseOutputTextDoneEvent;
    case 'response.reasoning_summary_text.delta':
      return {
        ...base,
        type,
        item_id: stringOf(body, 'item_id'),
        output_index: integerOf(body, 'output_index'),
        summary_index: integerOf(body, 'summary_index'),
        delta: stringOf(body, 'delta'),
      } as ResponseReasoningDeltaEvent;
    case 'response.reasoning_summary_text.done':
      return {
        ...base,
        type,
        item_id: stringOf(body, 'item_id'),
        output_index: integerOf(body, 'output_index'),
        summary_index: integerOf(body, 'summary_index'),
        text: stringOf(body, 'text'),
      } as ResponseReasoningDoneEvent;
    case 'response.cma_chat.tool_activity':
      return {
        ...base,
        type,
        response_id: stringOf(body, 'response_id'),
        item: toToolActivity(body.item),
      } as ResponseToolActivityEvent;
    case 'response.reasoning_summary_part.added':
    case 'response.reasoning_summary_part.done':
    case 'response.output_item.added':
    case 'response.content_part.added':
    case 'response.content_part.done':
    case 'response.output_item.done':
      return { ...base, type } as ResponseStructuralEvent;
    default:
      throw new ProtocolError(`Responses stream carried unknown event type '${type}'`);
  }
}

function validateCreateRequest(request: CreateResponseRequest): void {
  if (request.conversation.trim() === '') {
    throw new ValidationError('response conversation must not be empty');
  }
  if (typeof request.input === 'string') {
    if (request.input === '') {
      throw new ValidationError('response input must not be empty');
    }
    return;
  }
  if (!Array.isArray(request.input) || request.input.length !== 1) {
    throw new ValidationError('response input must contain exactly one user message');
  }
}

async function* framesFromBody(response: Response, signal: AbortSignal): AsyncGenerator<unknown> {
  const body = response.body;
  if (body === null) {
    throw new ProtocolError('Responses stream response carried no body');
  }
  const pending: unknown[] = [];
  let failure: ProtocolError | undefined;
  const parser = parserFor(
    (frame) => pending.push(frame),
    (error) => {
      failure = error;
    },
  );
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = body.getReader();
  let endedNaturally = false;
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (done) {
        endedNaturally = true;
        break;
      }
      try {
        parser.feed(decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('Responses stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) {
        throw failure;
      }
      while (pending.length > 0) {
        yield pending.shift();
      }
    }
    try {
      parser.feed(decoder.decode());
      parser.reset({ consume: true });
    } catch (cause) {
      throw new ProtocolError('Responses stream ended with an invalid frame', { cause });
    }
    if (failure !== undefined) {
      throw failure;
    }
    while (pending.length > 0) {
      yield pending.shift();
    }
  } finally {
    if (!endedNaturally) {
      await reader.cancel(signal.reason).catch(() => undefined);
    }
    reader.releaseLock();
  }
}

function parserFor(onFrame: (frame: unknown) => void, onFailure: (error: ProtocolError) => void) {
  return createParser({
    maxBufferSize: MAX_FRAME_BYTES,
    onEvent: (message) => {
      try {
        onFrame(frameFromMessage(message));
      } catch (cause) {
        onFailure(
          cause instanceof ProtocolError
            ? cause
            : new ProtocolError('Responses frame could not be decoded', { cause }),
        );
      }
    },
    onError: (error) =>
      onFailure(
        new ProtocolError(`Responses stream is not valid SSE: ${error.message}`, { cause: error }),
      ),
  });
}

function frameFromMessage(message: EventSourceMessage): unknown {
  if (new TextEncoder().encode(message.data).byteLength > MAX_FRAME_BYTES) {
    throw new ProtocolError(`Responses frame exceeds the ${String(MAX_FRAME_BYTES)}-byte boundary`);
  }
  let raw: unknown;
  try {
    raw = JSON.parse(message.data) as unknown;
  } catch (cause) {
    throw new ProtocolError('Responses frame was not valid JSON', { cause });
  }
  const frame = objectOf(raw, 'Responses frame');
  const type = stringOf(frame, 'type');
  if (message.event !== undefined && message.event !== 'message' && message.event !== type) {
    throw new ProtocolError(
      `Responses SSE event name '${message.event}' did not match payload type '${type}'`,
    );
  }
  return frame;
}

function toConversationItem(raw: unknown): ResponsesConversationItem {
  const body = objectOf(raw, 'conversation item');
  const type = stringOf(body, 'type');
  if (type === 'cma_chat.tool_activity') {
    return toToolActivity(body);
  }
  if (type !== 'message') {
    throw new ProtocolError(`conversation item carried unknown type '${type}'`);
  }
  literalOf(body, 'status', 'completed');
  const role = stringOf(body, 'role');
  if (role !== 'user' && role !== 'assistant') {
    throw new ProtocolError(`conversation message carried unknown role '${role}'`);
  }
  return {
    id: stringOf(body, 'id'),
    type: 'message',
    status: 'completed',
    role,
    content: arrayOf(body.content, 'conversation message content').map((rawPart) => {
      const part = objectOf(rawPart, 'conversation message content part');
      const partType = stringOf(part, 'type');
      if (partType !== 'input_text' && partType !== 'output_text') {
        throw new ProtocolError(`conversation message carried unknown content type '${partType}'`);
      }
      return {
        type: partType,
        text: stringOf(part, 'text'),
        ...(part.annotations === undefined
          ? {}
          : { annotations: arrayOf(part.annotations, 'message annotations') }),
        ...(part.logprobs === undefined
          ? {}
          : { logprobs: arrayOf(part.logprobs, 'message logprobs') }),
      };
    }),
  };
}

function toToolActivity(raw: unknown): ResponseToolActivity {
  const body = objectOf(raw, 'tool activity');
  literalOf(body, 'type', 'cma_chat.tool_activity');
  const optionalBody = (field: 'arguments' | 'result'): ResponseToolBody | null | undefined => {
    if (body[field] === undefined) return undefined;
    if (body[field] === null) return null;
    const value = objectOf(body[field], `tool activity ${field}`);
    return { text: stringOf(value, 'text'), truncated: booleanOf(value, 'truncated') };
  };
  const duration =
    body.duration_ms === undefined
      ? undefined
      : body.duration_ms === null
        ? null
        : integerOf(body, 'duration_ms');
  return {
    id: stringOf(body, 'id'),
    type: 'cma_chat.tool_activity',
    call_id: stringOf(body, 'call_id'),
    kind: stringOf(body, 'kind'),
    target: stringOf(body, 'target'),
    status: stringOf(body, 'status'),
    ...(duration === undefined ? {} : { duration_ms: duration }),
    ...(body.arguments === undefined ? {} : { arguments: optionalBody('arguments') }),
    ...(body.result === undefined ? {} : { result: optionalBody('result') }),
  };
}

function toOutputMessage(raw: unknown): ResponseOutputMessage {
  const body = objectOf(raw, 'response output message');
  literalOf(body, 'type', 'message');
  const status = stringOf(body, 'status');
  if (status !== 'in_progress' && status !== 'completed') {
    throw new ProtocolError('response output message status was unsupported');
  }
  literalOf(body, 'role', 'assistant');
  return {
    id: stringOf(body, 'id'),
    type: 'message',
    status,
    role: 'assistant',
    content: arrayOf(body.content, 'response output content').map((rawPart) => {
      const part = objectOf(rawPart, 'response output text');
      literalOf(part, 'type', 'output_text');
      return {
        type: 'output_text',
        text: stringOf(part, 'text'),
        annotations: arrayOf(part.annotations, 'response annotations'),
        logprobs: arrayOf(part.logprobs, 'response logprobs'),
      };
    }),
  };
}

function toResponseError(raw: unknown): ResponseError {
  const body = objectOf(raw, 'response error');
  return { code: stringOf(body, 'code'), message: stringOf(body, 'message') };
}

function toUsage(raw: unknown): ResponseUsage {
  const body = objectOf(raw, 'response usage');
  const input = objectOf(body.input_tokens_details, 'input token details');
  const output = objectOf(body.output_tokens_details, 'output token details');
  return {
    input_tokens: integerOf(body, 'input_tokens'),
    input_tokens_details: { cached_tokens: integerOf(input, 'cached_tokens') },
    output_tokens: integerOf(body, 'output_tokens'),
    output_tokens_details: { reasoning_tokens: integerOf(output, 'reasoning_tokens') },
    total_tokens: integerOf(body, 'total_tokens'),
  };
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function arrayOf(raw: unknown, name: string): readonly unknown[] {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an array`);
  }
  return raw;
}

function stringOf(body: Record<string, unknown>, field: string): string {
  const value = body[field];
  if (typeof value !== 'string' || value === '') {
    throw new ProtocolError(`${field} was not a non-empty string`);
  }
  return value;
}

function stringsOf(raw: unknown, name: string): Readonly<Record<string, string>> {
  const body = objectOf(raw, name);
  for (const [key, value] of Object.entries(body)) {
    if (typeof value !== 'string') {
      throw new ProtocolError(`${name} field '${key}' was not a string`);
    }
  }
  return body as Readonly<Record<string, string>>;
}

function integerOf(body: Record<string, unknown>, field: string): number {
  const value = body[field];
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new ProtocolError(`${field} was not a non-negative safe integer`);
  }
  return value;
}

function booleanOf(body: Record<string, unknown>, field: string): boolean {
  const value = body[field];
  if (typeof value !== 'boolean') {
    throw new ProtocolError(`${field} was not a boolean`);
  }
  return value;
}

function nullableField(body: Record<string, unknown>, field: string): unknown | null {
  if (!(field in body)) {
    throw new ProtocolError(`response omitted ${field}`);
  }
  return body[field] ?? null;
}

function nullableStringOf(body: Record<string, unknown>, field: string): string | null {
  const value = nullableField(body, field);
  if (value !== null && typeof value !== 'string') {
    throw new ProtocolError(`${field} was neither a string nor null`);
  }
  return value;
}

function nullableIntegerOf(body: Record<string, unknown>, field: string): number | null {
  const value = nullableField(body, field);
  if (value === null) return null;
  return integerOf(body, field);
}

function nullableNumberOf(body: Record<string, unknown>, field: string): number | null {
  const value = nullableField(body, field);
  if (value !== null && (typeof value !== 'number' || !Number.isFinite(value))) {
    throw new ProtocolError(`${field} was neither a finite number nor null`);
  }
  return value;
}

function literalOf<T extends string | number | boolean>(
  body: Record<string, unknown>,
  field: string,
  expected: T,
): T {
  if (body[field] !== expected) {
    throw new ProtocolError(`${field} did not equal ${String(expected)}`);
  }
  return expected;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function isAborted(signal: AbortSignal | undefined): boolean {
  return signal?.aborted === true;
}
