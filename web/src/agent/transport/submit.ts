import { ApiError, type Cma, type V2ResponseReceipt, type AnswerContext } from '@cma/client';
import type { AgentTransportJournal } from './journal';
import type { InputRecord } from './journalRecord';
import { transientRequestFailure, type RequestRecovery } from './requestRecovery';

interface SubmissionOptions {
  readonly answerContext?: AnswerContext;
  readonly signal?: AbortSignal;
  readonly authorize?: () => Promise<void>;
  readonly recovery?: RequestRecovery;
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
  const recovering = journal.read()?.state === 'pending';
  return submit(sdk, journal, journal.retain(input, options.answerContext), options, recovering);
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
  return submit(sdk, journal, retained, options, true);
}

async function submit(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: InputRecord,
  options: SubmissionOptions,
  uncertainAdmission: boolean,
): Promise<void> {
  const request = new AbortController();
  const abort = () => request.abort(options.signal?.reason);
  options.signal?.addEventListener('abort', abort, { once: true });
  let acknowledged = false;
  let receiptObserved = false;
  let knownReceipt: V2ResponseReceipt | undefined;
  let confirmationFailure: unknown;
  let contentionRetries = 0;
  const confirm = async (receipt: V2ResponseReceipt) => {
    try {
      await options.authorize?.();
    } catch (error) {
      confirmationFailure = error;
      throw error;
    }
    options.signal?.throwIfAborted();
    requireRetained();
    journal.accepted(input.key, receipt);
    options.onAccepted(receipt);
    acknowledged = true;
    request.abort();
  };
  const requireRetained = () => {
    const retained = journal.read();
    if (
      retained?.state !== 'pending' ||
      retained.key !== input.key ||
      retained.input !== input.input ||
      retained.answerContext?.turn_sequence !== input.answerContext?.turn_sequence ||
      retained.answerContext?.answer_sequence !== input.answerContext?.answer_sequence
    )
      throw new Error('The retained input changed while its receipt was being recovered.');
  };
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
          timeoutMs: options.recovery?.timeoutMs,
          async onReceipt(receipt) {
            receiptObserved = true;
            knownReceipt = receipt;
            options.onReceiptObserved?.(receipt);
            // Closing this reader does not cancel an admitted Agent turn. The
            // independent Agent tail continues tools, approvals, text and Thinking.
            await confirm(receipt);
          },
          onEvent() {},
        },
      );
    const attemptRequest = async (retry: number) => {
      if (retry > 0) {
        if (knownReceipt !== undefined) {
          requireRetained();
          await confirm(knownReceipt);
          return;
        }
        await options.authorize?.();
        options.signal?.throwIfAborted();
        requireRetained();
      }
      for (;;) {
        try {
          await send();
          break;
        } catch (error) {
          if (confirmationFailure !== undefined) throw confirmationFailure;
          // A failed POST may already have committed. Later admission refusal
          // cannot disprove that earlier outcome; retain custody across recovery.
          uncertainAdmission ||=
            transientRequestFailure(error) ||
            (error instanceof ApiError &&
              error.status === 409 &&
              error.code === 'agent_command_admission_conflict');
          // Contention retries also keep the original intent. This code does
          // not authorize re-keying or treating generic 409 as no admission.
          if (
            receiptObserved ||
            contentionRetries >= 3 ||
            !(error instanceof ApiError) ||
            error.status !== 409 ||
            error.code !== 'agent_command_admission_conflict'
          )
            throw error;
          await retryDelay(50 * 2 ** contentionRetries++, request.signal);
          await options.authorize?.();
          request.signal.throwIfAborted();
          requireRetained();
        }
      }
    };
    if (options.recovery) await options.recovery.retry(attemptRequest);
    else await attemptRequest(0);
  } catch (error) {
    if (!acknowledged) {
      if (
        !receiptObserved &&
        !uncertainAdmission &&
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
