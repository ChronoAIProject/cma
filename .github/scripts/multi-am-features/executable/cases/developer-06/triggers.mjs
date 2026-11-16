import { define, setup, open, api, button, visible, contains, absent, enc } from './helpers.mjs';
export const cases = [
  define('triggers.browse', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'trigger-browse');
      await page.goto(origin + '/triggers/manage');
      await visible(
        ctx,
        page.getByRole('heading', { name: 'Triggers', exact: true }),
        'publisher_trigger_surface',
      );
      ctx.assert(f.publisher === true, 'publisher_capability_fixture');
    },
    async (ctx) => {
      const nav = ctx.state.b.page.getByRole('navigation', { name: 'Your Triggers', exact: true });
      const before = await nav.getByRole('link').count();
      await button(nav, 'Load more').click();
      await contains(ctx, nav, ctx.state.f.secondPageTitle, 'trigger_page_two');
      ctx.assert((await nav.getByRole('link').count()) > before, 'trigger_pagination_loaded');
    },
    async (ctx) => {
      const f = ctx.state.f;
      for (const row of f.targets) {
        await ctx.state.b.page.locator(`a[href^="/triggers/manage/${row.id}"]`).click();
        await contains(ctx, ctx.state.b.page.locator('main'), row.title, 'trigger_selected_title');
        const r = await api(ctx, `/api/v1/triggers/${enc(row.id)}`);
        ctx.assert(
          r.body.id === row.id &&
            r.body.availability === row.availability &&
            r.body.draft.presentation.title === row.title,
          'trigger_state_matches_durable',
        );
      }
    },
    async (ctx) => {
      const b = await open(ctx, 'outsider');
      await b.page.goto(b.origin + '/triggers/manage');
      await contains(ctx, b.page.locator('main'), 'unavailable', 'nonpublisher_clear_unavailable');
      await absent(ctx, button(b.page, 'New Trigger'), 'nonpublisher_no_publish_control');
      const r = await api(ctx, '/api/v1/triggers', { actor: 'outsider', allowed: [403] });
      ctx.assert(r.status === 403, 'nonpublisher_server_denied');
    },
  ]),
];
