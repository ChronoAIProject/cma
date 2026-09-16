// Bounded public CMA Responses v2 observation. Never retries or submits writes.
const check = (condition, code) => { if (!condition) throw new Error(code); };
const identifier = (value) => typeof value === 'string' && /^[a-zA-Z0-9_-]{1,200}$/.test(value);
const limit = 4 * 1024 * 1024;

function streamReceipt(response, expected) {
  check(response.status === 200 && /^text\/event-stream(?:;|$)/i.test(response.headers.get('content-type') ?? ''), 'v2_stream_required');
  const responseId = response.headers.get('x-cma-response-id');
  const commandId = response.headers.get('x-cma-command-id');
  check(identifier(responseId) && identifier(commandId) && response.body, 'v2_durable_receipt_required');
  return { responseId, commandId, conversation: expected.conversation };
}

// Admission is durable at headers. Closing this observer never cancels the turn.
export async function admittedResponse(response, expected, observe) {
  const receipt = streamReceipt(response, expected);
  await observe({ ...receipt, status: 'stream_opened' });
  await response.body.cancel().catch(() => undefined);
  return { ...receipt, status: 'admitted_observer_closed' };
}

export async function completedResponse(response, expected, observe, forbidden) {
  const receipt = streamReceipt(response, expected);
  const { responseId } = receipt;
  await observe({ ...receipt, status: 'stream_opened' });
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  const progress = setInterval(() => console.log(JSON.stringify({ stage: 'v2_response_stream', status: 'waiting' })), 20_000);
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      check(size <= limit, 'v2_stream_size_limit');
      chunks.push(value);
    }
  } catch (error) {
    if (error.message === 'v2_stream_size_limit') throw error;
    throw new Error('v2_stream_outcome_unknown');
  } finally {
    clearInterval(progress);
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
  const bytes = Buffer.concat(chunks);
  let text;
  try { text = new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
  catch { throw new Error('v2_invalid_utf8'); }
  check(!text.includes(forbidden), 'v2_unsafe_response');
  let sequence = 0;
  let terminal;
  let frame = { data: [] };
  const known = new Set(['response.created', 'response.in_progress', 'response.completed',
    'response.failed', 'response.incomplete', 'response.output_item.added',
    'response.output_item.done', 'response.content_part.added', 'response.content_part.done',
    'response.output_text.delta', 'response.output_text.done', 'error']);
  for (const line of text.split(/\r\n|\r|\n/)) {
    if (line === '') {
      if (frame.data.length) {
        check(!terminal, 'v2_event_after_terminal');
        let event;
        try { event = JSON.parse(frame.data.join('\n')); }
        catch { throw new Error('v2_invalid_event_json'); }
        check(known.has(event.type) && frame.event === event.type
          && Number.isSafeInteger(event.sequence_number) && event.sequence_number === sequence,
          'v2_event_envelope_mismatch');
        check(event.type === 'error' ? frame.id === undefined : frame.id === String(sequence), 'v2_event_cursor_mismatch');
        check(sequence > 0 || event.type === 'response.created', 'v2_creation_event_required');
        sequence++;
        if (event.type === 'error') throw new Error('v2_observation_unavailable');
        if (event.response !== undefined) {
          const value = event.response;
          const status = event.type === 'response.created' ? 'queued' : event.type.slice('response.'.length);
          check(value.id === responseId && value.conversation?.id === expected.conversation
            && value.status === status && value.previous_response_id === null,
            'v2_response_identity_mismatch');
          if (['completed', 'failed', 'incomplete'].includes(status)) {
            terminal = { ...receipt, status, terminalSequence: event.sequence_number, frames: sequence };
            await observe(terminal);
            check(status === 'completed', 'v2_response_not_completed');
            check(Array.isArray(value.output), 'v2_output_required');
            const output = value.output.flatMap((item) => {
              check(item.role === 'assistant' && item.status === 'completed' && Array.isArray(item.content), 'v2_output_invalid');
              return item.content.map((part) => {
                check(part.type === 'output_text' && typeof part.text === 'string', 'v2_output_invalid');
                return part.text;
              });
            }).join('\n');
            check(output.includes(expected.marker), 'v2_conversation_marker_missing');
          }
        }
      }
      frame = { data: [] };
      continue;
    }
    if (line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'data') frame.data.push(value);
    else if (field === 'event' || field === 'id') {
      check(frame[field] === undefined, 'v2_duplicate_event_field');
      frame[field] = value;
    }
  }
  check(frame.data.length === 0 && terminal?.status === 'completed', 'v2_terminal_completion_required');
  return terminal;
}
