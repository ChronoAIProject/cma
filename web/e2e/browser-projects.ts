type BrowserName = 'chromium' | 'firefox' | 'webkit';

/** Keep CI's installed Chromium default; opt in to the other pinned Playwright engines. */
export function browserProjects(chromiumArgs: string[] = []) {
  const names = (process.env.CMA_E2E_BROWSERS ?? 'chromium').split(',');
  if (
    names.some((name) => !['chromium', 'firefox', 'webkit'].includes(name)) ||
    new Set(names).size !== names.length
  )
    throw new Error('CMA_E2E_BROWSERS must list distinct chromium, firefox or webkit engines.');
  return names.map((name) => ({
    name,
    use: {
      browserName: name as BrowserName,
      launchOptions: {
        executablePath: name === 'chromium' ? process.env.CMA_E2E_BROWSER_PATH : undefined,
        args: name === 'chromium' ? chromiumArgs : [],
      },
    },
  }));
}
