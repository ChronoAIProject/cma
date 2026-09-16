// Pure checks for one replacement-owned manual Agent turn across a rollout.
const check = (condition, code) => { if (!condition) throw new Error(code); };
export const ACTIVE_SLEEP_SECONDS = 600;
export const ACTIVE_START_SECONDS = 120;

export function activeTurnSpec(change, originalMarker) {
  check(/^[a-z0-9][a-z0-9-]{0,47}$/.test(change), 'bounded_change_identity_required');
  check(/^[A-Za-z0-9-]{1,160}$/.test(originalMarker), 'bounded_original_marker_required');
  const marker = `CMA-ACTIVE-${change}`;
  const shell = `sleep ${ACTIVE_SLEEP_SECONDS}; printf '%s\\n' '${marker}'`;
  const final = `${originalMarker} ${marker}`;
  return { marker, shell, final, sleepSeconds: ACTIVE_SLEEP_SECONDS,
    prompt: `Perform one harmless continuity check in this owned disposable Agent. Launch this exact shell command once using your ordinary terminal tool: ${shell}\n`
      + `Allow a total command timeout of at least ${ACTIVE_SLEEP_SECONDS + 60} seconds if supported. Include ${marker} and sleep ${ACTIVE_SLEEP_SECONDS} in the launch tool's display title if the interface accepts a title. `
      + `If the terminal yields an existing running session, use only that tool's wait/poll operation on the exact returned session, with no input characters, in waits of at most 30 seconds and at most 40 polls. Include ${marker} poll same session in each poll's display title if supported. `
      + 'Do not launch the shell command again or run other commands, access or modify files, use the network, inspect credentials, or create another process to recover an uncertain result. '
      + 'If the terminal tool is unavailable, requires approval, or the returned running session cannot be identified, stop and report that limitation. '
      + `After the original command has exited successfully and printed its marker, reply with exactly: ${final}` };
}

export function checkActiveReading(reading, expected) {
  const { agent, transcript, history, response, command } = reading;
  const { agentId, receipt, beforeTurns, spec, retained } = expected;
  check(agent?.id === agentId && transcript?.agent_id === agentId, 'active_agent_identity_changed');
  check(command?.receipt?.command_id === receipt.commandId
    && command.receipt.response_id === receipt.responseId && command.receipt.agent_id === agentId,
  'active_command_receipt_changed');
  check(response?.id === receipt.responseId && response.conversation?.id === agentId
    && response.previous_response_id === null, 'active_response_identity_changed');
  check(!['failed', 'stopped', 'deleted', 'waiting_approval'].includes(agent.status), 'active_agent_not_executable');
  check(!['failed', 'cancelled', 'waiting_action'].includes(command.status)
    && !['failed', 'incomplete'].includes(response.status), 'active_response_failed_or_blocked');
  check(Array.isArray(history) && history.every((event, index) => event.seq === index + 1), 'active_event_sequence_invalid');
  check(Array.isArray(transcript.turns) && transcript.turns.length >= beforeTurns
    && transcript.turns.length <= beforeTurns + 1, 'active_unexpected_turn_count');
  if (transcript.turns.length === beforeTurns) {
    check(!retained && command.turn_sequence == null, 'active_retained_turn_missing');
    return { phase: 'queued' };
  }
  const turn = transcript.turns.at(-1);
  check(turn.owner_message === spec.prompt && Number.isSafeInteger(command.turn_sequence)
    && turn.turn_sequence === command.turn_sequence, 'active_turn_binding_changed');
  check(turn.outcome?.type === 'active' || turn.outcome?.type === 'completed', 'active_turn_failed_or_interrupted');
  if (retained) check(turn.turn_sequence === retained.turnSequence, 'active_turn_identity_changed');
  const start = history.findIndex((event) => event.seq === turn.turn_sequence
    && event.kind.type === 'turn_started' && event.kind.turn_sequence === turn.turn_sequence);
  check(start >= 0, 'active_turn_start_missing');
  const turnEvents = history.slice(start + 1);
  check(!turnEvents.some((event) => event.kind.type === 'turn_started'), 'active_extra_turn_started');
  check(!turnEvents.some((event) => ['approval_requested', 'turn_failed', 'turn_interrupted'].includes(event.kind.type)), 'active_turn_failed_or_blocked');
  const calls = turnEvents.filter((event) => event.kind.type === 'tool_call_started');
  check(calls.length <= 41, 'active_tool_poll_limit');
  if (!calls.length) {
    check(!retained && turn.outcome.type === 'active', 'active_tool_not_observed');
    return { phase: 'waiting_tool', turnSequence: turn.turn_sequence };
  }
  const started = calls[0];
  const tools = new Map();
  for (const [index, call] of calls.entries()) {
    const tool = { ...call.kind, startedSequence: call.seq };
    check(typeof tool.call_id === 'string' && tool.call_id.length > 0 && !tools.has(tool.call_id), 'active_tool_identity_changed');
    if (index === 0) check(tool.kind === 'execute' && tool.title.includes(spec.marker)
      && tool.title.includes(`sleep ${spec.sleepSeconds}`), 'active_expected_tool_required');
    else check(['execute', 'other'].includes(tool.kind) && tool.title.includes(spec.marker)
      && tool.title.toLowerCase().includes('poll same session') && !tool.title.includes(`sleep ${spec.sleepSeconds}`),
    'active_only_original_session_polls_allowed');
    tools.set(tool.call_id, tool);
  }
  for (const event of turnEvents.filter((event) => event.kind.type === 'tool_call_updated')) {
    const tool = tools.get(event.kind.call_id);
    check(tool, 'active_tool_identity_changed');
    if (event.kind.status !== 'updated') tool.status = event.kind.status;
  }
  const tool = tools.get(started.kind.call_id);
  if (retained) check(tool.call_id === retained.toolCallId, 'active_tool_identity_changed');
  const values = [...tools.values()];
  check(values.every((item) => item.status !== 'failed'), 'active_tool_failed');
  check(values.every((item) => ['pending', 'in_progress', 'completed'].includes(item.status)), 'active_unknown_tool_status');
  const running = values.filter((item) => item.status === 'in_progress').at(-1);
  const result = { turnSequence: turn.turn_sequence, toolCallId: tool.call_id,
    toolStartedSequence: started.seq, toolStartedAt: started.appended_at, toolStatus: tool.status,
    activeToolCallId: running?.call_id, observedToolCalls: values.length,
    responseId: receipt.responseId, commandId: receipt.commandId, observedThrough: history.at(-1)?.seq };
  if (turn.outcome.type === 'completed') {
    const final = turn.messages.filter((message) => message.role === 'assistant').at(-1)?.content;
    const responseText = response.output?.flatMap((item) => item.content ?? []).map((part) => part.text ?? '').join('\n');
    check(values.every((item) => item.status === 'completed') && final?.trim() === spec.final, 'active_completion_evidence_missing');
    if (command.status !== 'completed' || response.status !== 'completed' || agent.status !== 'ready') {
      return { ...result, phase: 'settling' };
    }
    check(responseText?.includes(spec.final), 'active_response_final_marker_missing');
    check(turnEvents.some((event) => event.kind.type === 'turn_completed'
      && event.kind.turn_sequence === turn.turn_sequence), 'active_completion_event_missing');
    return { ...result, phase: 'completed' };
  }
  return { ...result, phase: running && command.status === 'in_progress'
    && response.status === 'in_progress' && agent.status === 'working' ? 'active' : 'waiting_tool' };
}

export function checkOwnedTurnCounts(before, after) {
  check(after.agents.length === 2 && before.agents.length === 2, 'active_owned_pair_required');
  check(after.agents[0].transcript.turns.length === before.agents[0].transcript.turns.length,
    'active_sibling_turn_changed');
  check(after.agents[1].transcript.turns.length === before.agents[1].transcript.turns.length + 1,
    'active_unexpected_turn_count');
}
