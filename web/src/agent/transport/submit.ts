import { ApiError, type Cma, type V2ResponseReceipt, type AnswerContext } from '@cma/client';
import type { AgentTransportJournal } from './journal';
import type { InputRecord } from './journalRecord';
import { responseBody, sameInput } from './inputRefusal';
import { inputRefusalRecoveryEnabled } from './recoveryConfig';
import { transientRequestFailure, type RequestRecovery } from './requestRecovery';

export const inputRateLimited = 'The message was rate limited. Try sending it again shortly.';

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
): Promise<'refused' | void> {
  options.signal?.throwIfAborted();
  const recovering = journal.read()?.state === 'pending';
  if (!recovering && inputRetryAt(journal) !== undefined) return Promise.resolve('refused');
  const retained = journal.retain(input, options.answerContext);
  options.onRetained?.(retained.key);
  if (recovering) {
    return recoverAgentInput(sdk, journal, options).then((confirmed) => {
      if (!confirmed) throw unconfirmedInput();
      if (confirmed === 'refused') return confirmed;
    });
  }
  return submit(sdk, journal, retained, options);
}

/** Explicit recovery never changes a pending key/body or silently switches to v1. */
export async function recoverAgentInput(
  sdk: Cma,
  journal: AgentTransportJournal,
  options: SubmissionOptions,
): Promise<boolean | 'refused'> {
  options.signal?.throwIfAborted();
  const retained = journal.read();
  if (retained === null) return false;
  if (retained.state === 'accepted') {
    options.onAccepted(retained.receipt);
    return true;
  }
  const lookup = () =>
    reconcileInput(sdk, journal, retained, options, async (receipt) => {
      await options.authorize?.();
      options.signal?.throwIfAborted();
      acceptInput(journal, retained, receipt);
      options.onAccepted(receipt);
    });
  return options.recovery ? options.recovery.retry(lookup) : lookup();
}

export function inputRetryAt(journal: AgentTransportJournal): number | undefined {
  return inputRefusalRecoveryEnabled ? journal.inputRefusal.retryAt() : undefined;
}

function requireRetained(journal: AgentTransportJournal, input: InputRecord) {
  if (!sameInput(journal.read(), input)) {
    throw new Error('The retained input changed while its receipt was being recovered.');
  }
}

function acceptInput(
  journal: AgentTransportJournal,
  input: InputRecord,
  receipt: V2ResponseReceipt,
) {
  requireRetained(journal, input);
  journal.accepted(input.key, receipt);
  if (inputRefusalRecoveryEnabled) journal.inputRefusal.clear(input);
}

async function reconcileInput(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: InputRecord,
  options: SubmissionOptions,
  confirm: (receipt: V2ResponseReceipt) => Promise<void>,
): Promise<boolean | 'refused'> {
  const canRelease = inputRefusalRecoveryEnabled && journal.inputRefusal.matches(input);
  const receipt = await findReceipt(
    sdk,
    journal,
    input,
    options.signal,
    options.recovery?.timeoutMs,
    canRelease,
  );
  if (receipt) {
    await confirm(receipt);
    return true;
  }
  if (receipt !== null || !canRelease) return false;

  await options.authorize?.();
  options.signal?.throwIfAborted();
  if (!journal.inputRefusal.matches(input)) return false;
  journal.refused(input.key);
  journal.inputRefusal.clear(input);
  return 'refused';
}

async function findReceipt(
  sdk: Cma,
  journal: AgentTransportJournal,
  input: InputRecord,
  signal?: AbortSignal,
  timeoutMs?: number,
  requireJson?: boolean,
) {
  requireRetained(journal, input);
  const status = await sdk.responsesV2.findAgentInputReceipt(journal.agentId, responseBody(input), {
    idempotencyKey: input.key,
    signal,
    timeoutMs: timeoutMs ?? 5000,
    ...(requireJson ? { requireJson } : {}),
  });
  if (status === null) return null;
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
): Promise<'refused' | void> {
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
    acceptInput(journal, input, receipt);
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
    const lookup = async () => {
      await options.authorize?.();
      request.signal.throwIfAborted();
      const result = await reconcileInput(
        sdk,
        journal,
        input,
        { ...options, signal: request.signal },
        confirm,
      );
      if (!result) throw unconfirmedInput();
      if (result === 'refused') return result;
    };
    const attemptRequest = async (retry: number) => {
      if (retry > 0) return knownReceipt ? confirm(knownReceipt) : lookup();
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
        if (
          inputRefusalRecoveryEnabled &&
          !receiptObserved &&
          error instanceof ApiError &&
          error.status === 429 &&
          error.code === 'rate_limited'
        ) {
          request.signal.throwIfAborted();
          if (journal.inputRefusal.remember(input, error.retryAfterMs)) return lookup();
        }
        throw error;
      }
    };
    return options.recovery
      ? await options.recovery.retry(attemptRequest)
      : await attemptRequest(0);
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
