import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, check, id, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 20000;

export async function suggestionDraftBrowser({page, api, adapter, agentId, witness}) {
  const {text, scope} = witness ?? {};
  check(id(agentId) && typeof text === 'string' && text.length > 0
    && Number.isSafeInteger(scope?.turn_sequence) && scope.turn_sequence > 0
    && Number.isSafeInteger(scope.answer_sequence) && scope.answer_sequence > scope.turn_sequence,
  'browser_suggestion_scope_unbound');
  const path = '/agents/' + agentId;
  const responses = '/api/v2' + path + '/responses';
  const caseId = 'browser-suggestion-' + randomUUID();
  const marker = 'WS4_' + randomUUID().replaceAll('-', '');
  const original = 'Keep this unsent draft.';
  const appended = original + '\n' + text;
  const submitted = appended + '\nReturn the exact marker ' + marker + ' without tools or file changes.';
  const completed = 'WS4 complete ' + marker;
  const compose = page.getByRole('form', {name: 'Message Agent', exact: true});
  const draft = compose.getByRole('textbox', {name: 'Message', exact: true});
  const edit = page.getByRole('region', {name: 'Next you can ask', exact: true});
  const dialog = page.getByRole('dialog', {name: 'Keep your draft?', exact: true});
  const requests = [];
  const observe = request => {
    if (request.method() !== 'POST') return;
    const route = new URL(request.url()).pathname;
    if (route === responses || (route === '/api/v1/agui/run' && request.postDataJSON()?.threadId === agentId)) {
      requests.push(request);
    }
  };
  const choose = async label => {
    await edit.getByRole('button', {name: 'Edit suggestion 1 before sending', exact: true}).click({timeout});
    await dialog.getByRole('button', {name: label, exact: true}).click({timeout});
    await dialog.waitFor({state: 'hidden', timeout});
  };
  const readTurn = async am => {
    const {body} = await api.rawRequest(am, 'GET', '/api/v1' + path + '/transcript');
    check(body?.agent_id === agentId && Array.isArray(body.turns), 'browser_suggestion_transcript_unbound');
    const matches = body.turns.filter(turn => turn.owner_message === submitted);
    check(matches.length <= 1, 'browser_suggestion_turn_duplicated');
    const turn = matches[0];
    return turn?.outcome?.type === 'completed' && turn.messages.some(message =>
      message.role === 'assistant' && message.content === completed) ? turn.turn_sequence : null;
  };

  await adapter.prepareTurn({caseId, marker});
  page.on('request', observe);
  try {
    await withRestoration(async () => {
      await page.goto(path, {waitUntil: 'domcontentloaded', timeout});
      await edit.getByRole('button', {name: text, exact: true}).waitFor({timeout});
      await draft.fill(original, {timeout});
      await choose('Cancel');
      check(await draft.inputValue() === original, 'browser_suggestion_cancel_changed_draft');
      await choose('Replace text');
      check(await draft.inputValue() === text, 'browser_suggestion_replace_text_changed');
      await page.getByRole('button', {name: 'Undo replacement', exact: true}).click({timeout});
      check(await draft.inputValue() === original, 'browser_suggestion_undo_lost_draft');
      await choose('Append to draft');
      check(await draft.inputValue() === appended, 'browser_suggestion_append_changed_draft');
      check(requests.length === 0, 'browser_suggestion_edit_sent_before_confirmation');
      await draft.fill(submitted, {timeout});

      await adapter.releaseTurn({caseId});
      const [response] = await Promise.all([
        page.waitForResponse(result => result.request().method() === 'POST'
          && new URL(result.url()).pathname === responses, {timeout}),
        compose.getByRole('button', {name: 'Send message', exact: true}).click({timeout}),
      ]);
      const body = response.request().postDataJSON();
      check(response.ok() && body.input === submitted && body.stream === true
        && equal(body.answer_context, scope), 'browser_suggestion_submission_unbound');
      await page.getByText(completed, {exact: true}).waitFor({timeout: 90000});
      const observed = await Promise.all(['a', 'b'].map(am => poll(() => readTurn(am),
        Number.isSafeInteger, {timeout, code: 'browser_suggestion_durable_turn_missing'})));
      check(observed[0] === observed[1] && observed[0] > scope.turn_sequence
        && requests.length === 1 && requests[0] === response.request(), 'browser_suggestion_turn_unbound');
      const oracle = await poll(() => adapter.oracle({caseId}), snapshot => snapshot.active_streams === 0,
        {timeout, code: 'browser_suggestion_provider_still_active'});
      assertOracle(oracle, caseId, 1);
    }, () => adapter.releaseTurn({caseId}));
  } finally {
    page.off('request', observe);
  }
}
