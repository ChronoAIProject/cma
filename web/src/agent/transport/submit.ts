import { ApiError, type Cma, type V2ResponseReceipt, type AnswerContext } from '@chronoai/cma-sdk';
import type { AgentTransportJournal } from './journal';
import type { InputRecord } from './journalRecord';

interface SubmissionOptions {
  readonly answerContext?: AnswerContext;
  readonly signal?: AbortSignal;
  readonly authorize?: () => Promise<void>;
  readonly onAccepted: (receipt: V2ResponseReceipt) => void;
  /** Correlation only; identity and journal acceptance still follow separately. */
  readonly onReceiptObserved?: (receipt: V2ResponseReceipt) => void;
}

/** Input transport only. Rich Agent replay independently owns rendered output. */
export function submitAgentInput(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: string,
  options: SubmissionOptions,
): Promise<void> {
  options.signal?.throwIfAborted();
  return submit(sdk, journal, journal.retain(input, options.answerContext), options);
}

/** Explicit recovery never changes a pending key/body or silently switches to v1. */
export function recoverAgentInput(
  sdk: Cma,
  journal: AgentTransportJournal,
  options: SubmissionOptions,
): Promise<void> {
  options.signal?.throwIfAborted();
  const retained = journal.read();
  if (retained === null) return Promise.resolve();
  if (retained.state === 'accepted') {
    options.onAccepted(retained.receipt);
    return Promise.resolve();
  }
  return submit(sdk, journal, retained, options);
}

async function submit(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: InputRecord,
  options: SubmissionOptions,
): Promise<void> {
  const request = new AbortController();
  const abort = () => request.abort(options.signal?.reason);
  options.signal?.addEventListener('abort', abort, { once: true });
  let acknowledged = false;
  let receiptObserved = false;
  try {
    const send = () =>
      sdk.responsesV2.createForAgent(
        journal.agentId,
        {
          input: input.input,
          stream: true,
          ...(input.answerContext ? { answer_context: input.answerContext } : {}),
        },
        {
          idempotencyKey: input.key,
          signal: request.signal,
          async onReceipt(receipt) {
            receiptObserved = true;
            options.onReceiptObserved?.(receipt);
            await options.authorize?.();
            options.signal?.throwIfAborted();
            journal.accepted(input.key, receipt);
            options.onAccepted(receipt);
            acknowledged = true;
            // Closing this reader does not cancel an admitted Agent turn. The
            // independent Agent tail continues tools, approvals, text and Thinking.
            request.abort();
          },
          onEvent() {},
        },
      );
    for (let attempt = 0; ; attempt += 1) {
      try {
        await send();
        break;
      } catch (error) {
        // This typed response confirms that this observation did not admit the
        // command. Keep one durable intent through bounded contention retries;
        // lost acknowledgements and any observed receipt require recovery.
        if (
          receiptObserved ||
          attempt >= 3 ||
          !(error instanceof ApiError) ||
          error.status !== 409 ||
          error.code !== 'agent_command_admission_conflict'
        )
          throw error;
        await retryDelay(50 * 2 ** attempt, request.signal);
        await options.authorize?.();
        request.signal.throwIfAborted();
        const retained = journal.read();
        if (
          retained?.state !== 'pending' ||
          retained.key !== input.key ||
          retained.input !== input.input
        )
          throw error;
      }
    }
  } catch (error) {
    if (!acknowledged) {
      if (
        !receiptObserved &&
        error instanceof ApiError &&
        ((error.status === 422 &&
          (['invalid_response_input', 'invalid_answer_context'].includes(error.code ?? '') ||
            (input.answerContext !== undefined && error.code === 'responses_field_unsupported'))) ||
          (error.status === 409 && error.code === 'agent_command_not_admissible'))
      )
        journal.refused(input.key);
      throw error;
    }
  } finally {
    options.signal?.removeEventListener('abort', abort);
    request.abort();
  }
}

function retryDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', abort);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', abort, { once: true });
  });
}
