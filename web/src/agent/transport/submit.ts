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
  readonly onRetained?: (key: string) => void;
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
  const retained = journal.retain(input, options.answerContext);
  options.onRetained?.(retained.key);
  if (recovering) {
    return recoverAgentInput(sdk, journal, options).then((confirmed) => {
      if (!confirmed) throw unconfirmedInput();
    });
  }
  return submit(sdk, journal, retained, options);
}

/** Explicit recovery never changes a pending key/body or silently switches to v1. */
export async function recoverAgentInput(
  sdk: Cma,
  journal: AgentTransportJournal,
  options: SubmissionOptions,
): Promise<boolean> {
  options.signal?.throwIfAborted();
  const retained = journal.read();
  if (retained === null) return false;
  if (retained.state === 'accepted') {
    options.onAccepted(retained.receipt);
    return true;
  }
  const lookup = async () => {
    const receipt = await findReceipt(
      sdk,
      journal,
      retained,
      options.signal,
      options.recovery?.timeoutMs,
    );
    if (!receipt) return false;
    await options.authorize?.();
    options.signal?.throwIfAborted();
    requireRetained(journal, retained);
    journal.accepted(retained.key, receipt);
    options.onAccepted(receipt);
    return true;
  };
  return options.recovery ? options.recovery.retry(lookup) : lookup();
}

function responseBody(input: InputRecord) {
  return {
    input: input.input,
    stream: true as const,
    ...(input.answerContext ? { answer_context: input.answerContext } : {}),
  };
}

function requireRetained(journal: AgentTransportJournal, input: InputRecord) {
  const retained = journal.read();
  if (
    retained?.key !== input.key ||
    retained.input !== input.input ||
    retained.answerContext?.turn_sequence !== input.answerContext?.turn_sequence ||
    retained.answerContext?.answer_sequence !== input.answerContext?.answer_sequence
  ) {
    throw new Error('The retained input changed while its receipt was being recovered.');
  }
}

async function findReceipt(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: InputRecord,
  signal?: AbortSignal,
  timeoutMs?: number,
) {
  requireRetained(journal, input);
  const status = await sdk.responsesV2.findAgentInputReceipt(journal.agentId, responseBody(input), {
    idempotencyKey: input.key,
    signal,
    timeoutMs: timeoutMs ?? 5000,
  });
  const receipt = status?.receipt;
  return receipt?.response_id
    ? { responseId: receipt.response_id, commandId: receipt.command_id }
    : undefined;
}

function unconfirmedInput(): Error {
  return new Error(
    'The original message is still awaiting confirmation. Check the pending request again.',
  );
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
  let knownReceipt: V2ResponseReceipt | undefined;
  let confirmationFailure: unknown;
  let uncertainAdmission = false;
  const confirm = async (receipt: V2ResponseReceipt) => {
    try {
      await options.authorize?.();
    } catch (error) {
      confirmationFailure = error;
      throw error;
    }
    options.signal?.throwIfAborted();
    requireRetained(journal, input);
    journal.accepted(input.key, receipt);
    options.onAccepted(receipt);
    acknowledged = true;
    request.abort();
  };
  try {
    const send = () =>
      sdk.responsesV2.createForAgent(journal.agentId, responseBody(input), {
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
      });
    const attemptRequest = async (retry: number) => {
      if (retry > 0) {
        if (knownReceipt) return confirm(knownReceipt);
        await options.authorize?.();
        request.signal.throwIfAborted();
        const receipt = await findReceipt(
          sdk,
          journal,
          input,
          request.signal,
          options.recovery?.timeoutMs,
        );
        if (!receipt) throw unconfirmedInput();
        await confirm(receipt);
        return;
      }
      try {
        await send();
      } catch (error) {
        if (confirmationFailure !== undefined) throw confirmationFailure;
        // Any retry now looks up this key. Failures after dispatch cannot release custody.
        uncertainAdmission =
          transientRequestFailure(error) ||
          (error instanceof ApiError &&
            error.status === 409 &&
            error.code === 'agent_command_admission_conflict');
        throw error;
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
