import {
  absent,
  account,
  button,
  define,
  digest,
  go,
  observeDOM,
  peer,
  setup,
  shown,
  text,
} from './helpers.mjs';

const appearance = define('account.appearance', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-preferences');
    await go(s.page, '/agent-manager');
    s.original = await s.page.evaluate(() => localStorage.getItem('cma:appearance') ?? 'dark');
    ctx.defer('restore_appearance', async () => {
      await (await account(s.page))
        .getByRole('menuitemradio', { name: title(s.original), exact: true })
        .click();
    });
    for (const [label, dark] of [
      ['Light', false],
      ['Dark', true],
      ['System', false],
    ]) {
      if (label === 'System') await s.page.emulateMedia({ colorScheme: 'light' });
      await (await account(s.page))
        .getByRole('menuitemradio', { name: label, exact: true })
        .click();
      await checkTheme(ctx, s.page, dark);
      ctx.assert(
        (await s.page.evaluate(() => localStorage.getItem('cma:appearance'))) ===
          label.toLowerCase(),
        'theme_preference_saved',
      );
    }
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await checkTheme(ctx, s.page, false);
  },
  async (ctx, s) => {
    for (const colorScheme of ['dark', 'light']) {
      await s.page.emulateMedia({ colorScheme });
      await checkTheme(ctx, s.page, colorScheme === 'dark');
    }
  },
  async (ctx, s) => {
    await (await account(s.page))
      .getByRole('menuitemradio', { name: 'System', exact: true })
      .waitFor();
    ctx.assert(
      (await s.page
        .getByRole('menuitemradio', { name: 'System', exact: true })
        .getAttribute('aria-checked')) === 'true',
      'system_theme_selected',
    );
    await observeDOM(
      ctx,
      s,
      'appearance_ui',
      s.page.getByRole('menu', { name: 'Account', exact: true }),
    );
  },
]);
function title(value) {
  return value[0].toUpperCase() + value.slice(1);
}
async function checkTheme(ctx, page, dark) {
  await page.waitForFunction(
    (value) => document.body.hasAttribute('data-ds-dark-theme') === value,
    dark,
  );
  ctx.assert(
    (await page.evaluate(() => document.documentElement.style.colorScheme)) ===
      (dark ? 'dark' : 'light'),
    'effective_theme',
  );
}
const textSize = define('account.text-size', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-preferences');
    await go(s.page, '/agent-manager');
    s.original = (await s.page.locator('html').getAttribute('data-system-text-size')) ?? 'medium';
    ctx.defer('restore_text_size', async () => {
      await (await account(s.page))
        .getByRole('menuitemradio', { name: title(s.original), exact: true })
        .click();
    });
    for (const [label, scale] of [
      ['Small', '0.9'],
      ['Medium', '1'],
      ['Large', '1.15'],
    ]) {
      await (await account(s.page))
        .getByRole('menuitemradio', { name: label, exact: true })
        .click();
      ctx.assert(
        (await s.page.locator('html').getAttribute('data-system-text-size')) ===
          label.toLowerCase(),
        'text_size_applied',
      );
      ctx.assert(
        (await s.page.evaluate(() =>
          document.documentElement.style.getPropertyValue('--cma-system-font-scale'),
        )) === scale,
        'text_scale_applied',
      );
      await shown(ctx, button(s.page, /^Account menu for /), 'account_control_accessible');
    }
  },
  async (ctx, s) => {
    await peer(ctx, s);
    ctx.assert(
      (await s.page.locator('html').getAttribute('data-system-text-size')) === 'large',
      'text_size_persisted',
    );
  },
  async (ctx, s) => {
    await (await account(s.page))
      .getByRole('menuitemradio', { name: title(s.original), exact: true })
      .click();
    ctx.assert(
      (await s.page.locator('html').getAttribute('data-system-text-size')) === s.original,
      'text_size_restored',
    );
  },
  async (ctx, s) => {
    await observeDOM(
      ctx,
      s,
      'text_size_controls',
      s.page.getByRole('menu', { name: 'Account', exact: true }),
    );
  },
]);
const build = define('account.build-version', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-preferences');
    await go(s.page, '/agent-manager');
    await shown(ctx, await account(s.page), 'account_menu_open');
  },
  async (ctx, s) => {
    s.build = s.page.getByRole('menuitem', { name: /^Copy build version/ });
    ctx.assert(
      (await s.build.innerText()).includes(s.data.webBuild.label),
      'compiled_web_build_label',
    );
  },
  async (ctx, s) => {
    await s.build.click();
    await shown(
      ctx,
      s.page.getByRole('menuitem', { name: 'Build version copied', exact: true }),
      'build_copy_feedback',
    );
    s.copied = await s.page.evaluate(() => navigator.clipboard.readText());
    ctx.assert(s.copied === s.data.webBuild.copyText, 'compiled_web_build_clipboard');
  },
  async (ctx, s) => {
    ctx.assert(s.data.webBuild.provenance === 'compiled-web-manifest', 'web_build_provenance');
    await ctx.observe('web_build_copy', async () => ({
      sha256: digest(s.copied),
      provenance: s.data.webBuild.provenance,
    }));
  },
]);
const signIn = define('account.sign-in', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-authentication');
    await go(s.page, s.data.returnPath);
    await (await account(s.page)).getByRole('menuitem', { name: 'Sign out', exact: true }).click();
    await shown(ctx, button(s.page, 'Open Agent Manager'), 'signed_out_landing');
    await absent(ctx, text(s.page, s.data.otherAccountSentinel), 'no_retained_foreign_content');
  },
  async (ctx, s) => {
    await button(s.page, 'Open Agent Manager').click();
    await s.page.waitForURL((url) => url.origin === s.data.nyxidOrigin);
    // Values stay in the isolated fixture; authentication bodies are never observed.
    for (const field of s.data.loginFields)
      await s.page.getByLabel(field.label, { exact: true }).fill(field.value);
    await button(s.page, s.data.loginSubmitLabel).click();
    await s.page.waitForURL((url) => url.pathname === s.data.returnPath);
    ctx.assert(new URL(s.page.url()).pathname === s.data.returnPath, 'nyxid_return_route');
  },
  async (ctx, s) => {
    await peer(ctx, s, s.data.returnPath);
    await shown(
      ctx,
      button(s.page, `Account menu for ${s.data.displayName}`),
      'signed_in_identity',
    );
  },
  async (ctx, s) => {
    for (const name of s.data.permittedControls)
      await shown(ctx, button(s.page, name), 'authorized_control_visible');
    await absent(ctx, text(s.page, s.data.otherAccountSentinel), 'foreign_content_not_retained');
    await observeDOM(
      ctx,
      s,
      'authenticated_account',
      button(s.page, `Account menu for ${s.data.displayName}`),
    );
  },
]);
const signOut = define('account.sign-out', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-authentication');
    await go(s.page, s.data.privatePath);
    await shown(ctx, text(s.page, s.data.privateSentinel), 'owned_private_content_visible');
    ctx.assert(s.data.activeWorkCount === 0, 'no_unfinished_fixture_work');
  },
  async (ctx, s) => {
    await (await account(s.page)).getByRole('menuitem', { name: 'Sign out', exact: true }).click();
    await shown(ctx, button(s.page, 'Get Started'), 'signed_out_landing');
  },
  async (ctx, s) => {
    await peer(ctx, s, s.data.privatePath);
    await shown(ctx, button(s.page, 'Get Started'), 'protected_route_requires_auth');
  },
  async (ctx, s) => {
    await absent(ctx, text(s.page, s.data.privateSentinel), 'private_content_removed');
    await absent(ctx, s.page.getByLabel('Message', { exact: true }), 'protected_composer_removed');
    await absent(ctx, button(s.page, /^Account menu for /), 'authenticated_menu_removed');
    await observeDOM(ctx, s, 'signed_out_ui', s.page.locator('main'));
  },
]);
export const cases = [signIn, signOut, appearance, textSize, build];
