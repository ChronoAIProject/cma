import {browser, navigateSettings, apiPath, P, check, required, equal} from './common.mjs';
import {periodFields} from './spend-period-data.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';

const SETTINGS = P + '/settings', REPORTS = P + '/usage/metrics/reports';
const READS = new Set(['GET', 'HEAD', 'OPTIONS']);
const scope = 'Disabling stops new admission accounting and retains configured policies. Existing reservations and stops remain effective. '
  + 'These settings do not yet establish a strict provider-spend ceiling; platform threshold actions and model-call enforcement require their backend integrations.';

export async function availabilityUI(ctx, ownerFields) {
  const session = await browser(ctx), {page} = session, reports = operationsPrefetch(ctx, session);
  const writes = [], byRequest = new WeakMap(), pending = new Set(), failures = [], expected = [];
  let closeJob;
  const responseData = async r => ({status: r.status(), request_id: await r.headerValue('x-request-id'), body: await r.json()});
  const onRequest = req => {
    try {
      if (READS.has(req.method()) || apiPath(req.url()) === REPORTS) return;
      check(writes.length < 256, 'admin_spend_availability_request_bound');
      const row = {method: req.method(), path: apiPath(req.url()), body: req.postDataJSON()};
      writes.push(row); byRequest.set(req, row);
    } catch (error) {failures.push(error);}
  };
  const onResponse = r => {
    const row = byRequest.get(r.request()); if (!row) return;
    const job = (async () => {row.response = await responseData(r);})(); pending.add(job);
    void job.then(() => pending.delete(job), e => {failures.push(e); pending.delete(job);});
  };
  page.on('request', onRequest); page.on('response', onResponse);
  const capture = async (final = false) => {
    const errors = []; let proof;
    try {proof = await reports.capture('admin_spend_availability_browser', final);} catch (e) {errors.push(e);}
    while (pending.size) await Promise.allSettled([...pending]);
    errors.push(...failures);
    await ctx.observe('admin_spend_availability_writes', async () => structuredClone(writes));
    try {
      required(proof?.authentication === 'browser_session', 'admin_spend_availability_cookie_session');
      check(writes.length === expected.length, 'admin_spend_availability_extra_write');
      const receipts = proof.requests.filter(r => !READS.has(r.method));
      check(receipts.length === writes.length, 'admin_spend_availability_uncaptured_mutation');
      for (const [i, row] of writes.entries()) {
        check(row.method === 'PUT' && row.path === SETTINGS && equal(row.body, expected[i].body), 'admin_spend_availability_body');
        const r = row.response;
        check(r?.request_id?.trim(), 'admin_spend_availability_unresolved');
        check(r.status === expected[i].status, 'admin_spend_availability_status');
        if (r.status === 503) check(r.body?.code === 'model_budget_writes_unavailable', 'admin_spend_availability_refusal');
        const matched = receipts.filter(v => v.request_id === r.request_id);
        check(matched.length === 1 && matched[0].method === row.method && matched[0].status === r.status
          && matched[0].am === ctx.am && matched[0].serving_am === ctx.am, 'admin_spend_availability_ingress');
      }
    } catch (e) {errors.push(e);}
    if (errors.length) throw new AggregateError(errors, 'admin_spend_availability_capture');
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (e) {errors.push(e);}
    page.off('request', onRequest); page.off('response', onResponse);
    try {await capture(true);} catch (e) {errors.push(e);}
    if (errors.length) throw new AggregateError(errors, 'admin_spend_availability_browser_finality');
  })();
  ctx.defer('admin_spend_availability_close', close);
  const toggle = page.getByLabel('Enable spend controls', {exact: true});
  const button = name => page.getByRole('button', {name, exact: true});
  const visibleBudget = async (budget, disabled) => {
    ctx.assert(await toggle.isVisible() && await toggle.isChecked() === budget.enabled
      && await toggle.isDisabled() === disabled, 'admin_spend_availability_toggle');
    const fields = async (value, labels, prefix = '') => {
      for (const [key, label] of Object.entries(labels)) {
        const field = page.getByLabel(prefix + label, {exact: true});
        ctx.assert(await field.isVisible() && await field.inputValue() === String(value[key])
          && await field.isDisabled() === disabled, 'admin_spend_availability_retained_field');
      }
    };
    await fields(budget.platform, periodFields);
    if (budget.default_policy) await fields(budget.default_policy, ownerFields);
    else ctx.assert(await button('Configure default owner policy').isVisible()
      && await button('Configure default owner policy').isDisabled() === disabled, 'admin_spend_availability_default_absence');
    ctx.assert(await page.getByRole('group', {name: /^Owner override \d+$/}).count() === budget.owners.length
      && await page.getByRole('group', {name: /^Request model \d+$/}).count() === (budget.request_models ?? []).length,
    'admin_spend_availability_list_count');
    for (const [i, entry] of budget.owners.entries()) {
      await fields(entry, {owner: 'owner ID'}, `Owner override ${i + 1}: `);
      await fields(entry.policy, ownerFields, `Owner override ${i + 1}: `);
    }
    for (const [i, entry] of (budget.request_models ?? []).entries())
      await fields(entry, {model: 'model name', input_overhead_tokens: 'input overhead (tokens)'}, `Request model ${i + 1}: `);
  };
  const scopeProof = async () => {
    const description = await ctx.observe('admin_spend_availability_scope', () => toggle.evaluate(node => {
      const id = node.getAttribute('aria-describedby');
      return {id, text: id ? node.ownerDocument.getElementById(id)?.textContent?.replace(/\s+/g, ' ').trim() : null};
    }));
    ctx.assert(description.id && description.text === scope && await page.getByText(scope, {exact: true}).isVisible(),
      'admin_spend_availability_scope_text');
  };
  return {close, capture, visibleBudget, scopeProof,
    async open(budget) {await navigateSettings(session); await capture(); await visibleBudget(budget, false); await scopeProof();},
    async toggle(value) {await toggle.setChecked(value);},
    async save(body, status) {
      expected.push({body: structuredClone(body), status});
      const [response] = await Promise.all([
        page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === SETTINGS), button('Save settings').click(),
      ]);
      const saved = await ctx.observe('admin_spend_availability_save', () => responseData(response));
      check(equal(response.request().postDataJSON(), body), 'admin_spend_availability_body');
      if (status === 503) check(saved.status === 503 && saved.body?.code === 'model_budget_writes_unavailable', 'admin_spend_availability_refusal');
      else check(saved.status === 200, 'admin_spend_availability_save_response');
      await capture(); return saved;
    },
    async refused(budget) {
      await page.getByText(/Saving spend controls is not available yet/).first().waitFor();
      await page.getByText(/The current revision is /).waitFor();
      await visibleBudget(budget, true);
      ctx.assert(await button('Save settings').isDisabled(), 'admin_spend_availability_disabled_save');
      // The submit handler must guard even a form submission that bypasses the disabled button.
      await button('Save settings').evaluate(node => node.form.requestSubmit());
      await capture(); await scopeProof();
    },
    async refresh(before, budget) {
      const [response] = await Promise.all([
        page.waitForResponse(r => r.request().method() === 'GET' && apiPath(r.url()) === SETTINGS),
        button('Refresh settings availability').click(),
      ]);
      const latest = await ctx.observe('admin_spend_availability_refresh', () => responseData(response));
      check(latest.status === 200 && equal(latest.body, before), 'admin_spend_availability_refresh_snapshot');
      await page.getByText(/The current revision is /).waitFor();
      ctx.assert(await button('Save settings').isDisabled(), 'admin_spend_availability_review_required');
      await button('Review retained draft').click(); await visibleBudget(budget, false);
      ctx.assert(await button('Save settings').isEnabled(), 'admin_spend_availability_recovered');
      await capture();
    },
    async saved(budget) {await page.getByText(/^Settings saved at revision /).waitFor(); await visibleBudget(budget, false); await scopeProof();},
  };
}
