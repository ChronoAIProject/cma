import {define, openBrowser, account, service, visible, uiReceipt, invoke} from './helpers.mjs';
import {textSizeCase} from './text-size.mjs';

const menu = page => page.getByRole('menu', {name: 'Account', exact: true});
const choice = (page, name) => menu(page).getByRole('menuitemradio', {name, exact: true});
const nav = page => page.getByRole('navigation', {name: 'Event Gateway navigation'});

export const browserCases = [
  define(1, 'browser', ['browser'], async ctx => {
    let session, page, mutations = 0;
    await ctx.step('step_1', async () => {
      session = await openBrowser(ctx); page = session.page;
      page.on('request', request => {
        if (/\/api\/(event-gateway|v1\/bots)/.test(request.url())
          && ['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method())) mutations++;
      });
      await service(page, 'Agent Manager');
      await service(page, 'Event Gateway');
      await visible(ctx, nav(page), 'gateway_navigation_visible');
      await page.getByRole('button', {name: 'New channel', exact: true}).click();
      await page.getByLabel('Channel name', {exact: true}).fill('EG channel draft');
      ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() === 'EG channel draft',
        'channel_draft_created');
    });
    await ctx.step('step_2', async () => {
      await nav(page).getByRole('button', {name: 'Consumers', exact: true}).click();
      await page.getByRole('button', {name: 'New consumer', exact: true}).click();
      await page.getByLabel('Consumer name', {exact: true}).fill('EG consumer draft');
      await service(page, 'Agent Manager');
      await service(page, 'Event Gateway');
      await nav(page).getByRole('button', {name: 'Consumers', exact: true}).click();
      ctx.assert(await page.getByLabel('Consumer name', {exact: true}).inputValue() === 'EG consumer draft',
        'consumer_draft_retained');
      await nav(page).getByRole('button', {name: 'Channels', exact: true}).click();
      ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() === 'EG channel draft',
        'channel_draft_retained');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(mutations === 0, 'section_switch_has_no_mutation');
      await visible(ctx, nav(page).getByRole('button', {name: 'Channels', exact: true}), 'channels_permitted');
      await visible(ctx, nav(page).getByRole('button', {name: 'Consumers', exact: true}), 'consumers_permitted');
      await uiReceipt(ctx, page);
    });
  }),
  define(3, 'browser', ['browser', 'eg_browser'], async ctx => {
    let session, page, originalUrl;
    await ctx.step('step_1', async () => {
      session = await openBrowser(ctx, 'user', {avatar: 'unavailable'}); page = session.page;
      originalUrl = page.url();
      for (const collapsed of [false, true]) {
        const toggle = page.getByRole('button', {name: collapsed ? 'Hide navigation' : 'Show navigation', exact: true});
        if (await toggle.isVisible()) await toggle.click();
        await account(page);
        await visible(ctx, menu(page), 'account_menu_visible');
        await page.keyboard.press('Escape');
        ctx.assert(!await menu(page).isVisible(), 'account_escape_dismisses');
      }
    });
    await ctx.step('step_2', async () => {
      const trigger = page.getByRole('button', {name: /^Account menu for /});
      ctx.assert(await trigger.getAttribute('aria-label') === `Account menu for ${session.identity.displayName}`,
        'account_current_identity');
      ctx.assert((await trigger.locator('.profile__face').innerText()).trim() === session.identity.initials,
        'account_initials_fallback');
      await account(page); await page.keyboard.press('Escape');
      ctx.assert(await trigger.evaluate(node => document.activeElement === node), 'account_focus_restored');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(page.url() === originalUrl, 'account_does_not_navigate');
      await uiReceipt(ctx, page);
    });
  }),
  define(5, 'browser', ['browser'], async ctx => {
    let page, original;
    await ctx.step('step_1', async () => {
      ({page} = await openBrowser(ctx));
      await account(page);
      original = await menu(page).getByRole('menuitemradio', {checked: true}).allTextContents();
      original = original.find(value => ['System', 'Light', 'Dark'].includes(value.trim())).trim();
      ctx.defer('restore_appearance', async () => { await account(page); await choice(page, original).click(); });
      for (const name of ['System', 'Light', 'Dark']) {
        await choice(page, name).click();
        ctx.assert(await choice(page, name).getAttribute('aria-checked') === 'true', 'appearance_choice_checked');
        const actual = await page.evaluate(() => ({
          stored: localStorage.getItem('cma:appearance'), dark: document.body.hasAttribute('data-ds-dark-theme'),
          systemDark: matchMedia('(prefers-color-scheme: dark)').matches,
        }));
        ctx.assert(actual.stored === name.toLowerCase(), 'appearance_persisted');
        ctx.assert(actual.dark === (name === 'Dark' || (name === 'System' && actual.systemDark)), 'appearance_applied');
      }
    });
    await ctx.step('step_2', async () => {
      await page.keyboard.press('Escape'); await account(page);
      ctx.assert(await choice(page, 'Dark').getAttribute('aria-checked') === 'true', 'appearance_reopened');
      await choice(page, original).click();
      ctx.assert(await choice(page, original).getAttribute('aria-checked') === 'true', 'appearance_restored');
    });
    await ctx.step('expected_result', async () => {
      await page.reload(); await account(page);
      ctx.assert(await choice(page, original).getAttribute('aria-checked') === 'true', 'appearance_reload_persists');
      await uiReceipt(ctx, page);
    });
  }),
  textSizeCase,
  define(7, 'browser', ['browser', 'eg_browser'], async ctx => {
    let session, page;
    await ctx.step('step_1', async () => {
      session = await openBrowser(ctx); page = session.page;
      await invoke(session, 'clipboard', 'allow');
      await account(page); await menu(page).getByRole('menuitem', {name: /^Copy build version/}).click();
      await visible(ctx, menu(page).getByRole('menuitem', {name: 'Build version copied', exact: true}), 'build_copy_success');
      const copied = await page.evaluate(() => navigator.clipboard.readText());
      ctx.assert(copied === session.buildText, 'build_clipboard_exact');
    });
    await ctx.step('step_2', async () => {
      await page.keyboard.press('Escape'); await invoke(session, 'clipboard', 'deny');
      await account(page); await menu(page).getByRole('menuitem', {name: /^Copy build version/}).click();
      const fallback = page.locator('.profile__build-fallback');
      await visible(ctx, fallback, 'build_clipboard_failure_visible');
      ctx.assert(await fallback.locator('code').innerText() === session.buildText, 'build_fallback_exact');
      ctx.assert(await page.evaluate(() => getSelection()?.toString()) === session.buildText, 'build_fallback_selected');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert((await page.locator('.profile__build-fallback').innerText()).includes('Could not copy.'),
        'clipboard_failure_explicit');
      await uiReceipt(ctx, page);
    });
  }),
  define(8, 'browser', ['browser', 'eg_browser'], async ctx => {
    let admin, user;
    await ctx.step('step_1', async () => {
      admin = await openBrowser(ctx, 'admin'); await account(admin.page);
      await visible(ctx, menu(admin.page).getByRole('menuitem', {name: 'Operations', exact: true}), 'operator_operations_available');
    });
    await ctx.step('step_2', async () => {
      await menu(admin.page).getByRole('menuitem', {name: 'Operations', exact: true}).click();
      ctx.assert(new URL(admin.page.url()).pathname === '/operations', 'operator_operations_navigation');
      user = await openBrowser(ctx, 'user'); await account(user.page);
      ctx.assert(await menu(user.page).getByRole('menuitem', {name: 'Operations', exact: true}).count() === 0,
        'non_operator_operations_absent');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(admin.identity.canOpenOperations === true && user.identity.canOpenOperations === false,
        'operations_current_capability_fixtures');
      await uiReceipt(ctx, admin.page, 'admin_browser_receipt');
      await uiReceipt(ctx, user.page, 'user_browser_receipt');
    });
  }),
];
