import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { SdkDocument, SdkDocumentFilename, SdkDocumentSets, SdkLanguage } from './docSet';
import { SDK_DOCUMENTS } from './documents';
import { DeveloperSurface } from './DeveloperSurface';

function document(
  sdk: SdkLanguage,
  filename: SdkDocumentFilename,
  label: string,
  markdown: string,
  order: number,
): SdkDocument {
  return { sdk, filename, label, title: `${sdk} ${label}`, markdown, order };
}

const TYPESCRIPT_OVERVIEW = `# TypeScript Overview

<!-- nav: 10 Overview -->

Read the [Authentication](authentication.md#credentials) guide or the
[public site](https://example.test/reference). A repository-only
[contract](../../../docs/API_REST.md) stays as words.

## Repeated heading

| Field | Meaning |
| --- | --- |
| cursor | Resume point |

## Repeated heading

\`\`\`ts
const session = "ready";
\`\`\`

<script>window.injected = true</script>

Inline <strong data-injected="true">words survive</strong> without markup.

![remote pixel](https://example.test/pixel.png)
`;

const DOCUMENTS: SdkDocumentSets = {
  typescript: [
    document('typescript', 'README.md', 'Overview', TYPESCRIPT_OVERVIEW, 10),
    document(
      'typescript',
      'authentication.md',
      'Authentication',
      '# TypeScript Authentication\n\n## Credentials\n\nRotate them.\n',
      20,
    ),
    document(
      'typescript',
      'sponsorships.md',
      'Sponsorships',
      '# TypeScript Sponsorships\n\nRead secret-free pages.\n',
      30,
    ),
  ],
  rust: [
    document('rust', 'README.md', 'Overview', '# Rust Overview\n\nBackend client.\n', 10),
    document(
      'rust',
      'authentication.md',
      'Authentication',
      '# Rust Authentication\n\n## Credentials\n\nUse a provider.\n',
      20,
    ),
  ],
};

describe('DeveloperSurface', () => {
  it('renders repository markdown with document hierarchy, GFM, code, and stable anchors', () => {
    const { container } = render(
      <DeveloperSurface documents={DOCUMENTS} apiBaseUrl="https://cma.example.test" />,
    );

    const title = screen.getByRole('heading', { level: 1, name: 'TypeScript Overview' });
    expect(title).toHaveAttribute('id', 'typescript-overview');
    const repeated = screen.getAllByRole('heading', { level: 2, name: 'Repeated heading' });
    expect(repeated.map((heading) => heading.id)).toEqual([
      'repeated-heading',
      'repeated-heading-2',
    ]);
    expect(screen.getByRole('table')).toHaveTextContent('Resume point');
    expect(container.querySelector('.developer__block')).toHaveTextContent(
      'const session = "ready";',
    );
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
    expect(container.textContent).not.toContain('<!-- nav:');
  });

  it('never injects raw HTML or images while retaining ordinary inline words', () => {
    const { container } = render(<DeveloperSurface documents={DOCUMENTS} />);

    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('[data-injected]')).toBeNull();
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).not.toContain('window.injected');
    expect(container).toHaveTextContent('Inline words survive without markup.');
    expect(container).toHaveTextContent('remote pixel');
  });

  it('switches bundled links and keeps the current filename when the SDK changes', async () => {
    const user = userEvent.setup();
    render(<DeveloperSurface documents={DOCUMENTS} />);

    await user.click(screen.getByRole('link', { name: 'Authentication' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'TypeScript Authentication' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Authentication', current: 'page' }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Rust' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'Rust Authentication' }),
    ).toBeInTheDocument();
  });

  it('falls back to the first guide when the selected guide is unavailable in another SDK', async () => {
    const user = userEvent.setup();
    render(<DeveloperSurface documents={DOCUMENTS} />);

    await user.click(screen.getByRole('button', { name: 'Sponsorships' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'TypeScript Sponsorships' }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Rust' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Rust Overview' })).toBeInTheDocument();
  });

  it('uses the narrow document selector for the same ordered guide state', async () => {
    const user = userEvent.setup();
    render(<DeveloperSurface documents={DOCUMENTS} />);

    const picker = screen.getByRole('combobox', { name: 'Guide' });
    await user.selectOptions(picker, 'authentication.md');

    expect(picker).toHaveValue('authentication.md');
    expect(
      screen.getByRole('heading', { level: 1, name: 'TypeScript Authentication' }),
    ).toBeInTheDocument();
  });

  it('states this deployment base and the non-self-service credential path', () => {
    render(<DeveloperSurface documents={DOCUMENTS} apiBaseUrl="https://cma.example.test" />);

    const preface = screen.getByRole('group', { name: 'Deployment integration' });
    expect(preface).toHaveTextContent('https://cma.example.test');
    expect(preface).toHaveTextContent(
      'Ask the deployment operator or identity administrator for a bearer credential.',
    );
    expect(within(preface).queryByRole('textbox')).toBeNull();
  });

  it('opens only safe external protocols and leaves unbundled repository links as text', () => {
    render(<DeveloperSurface documents={DOCUMENTS} />);

    const external = screen.getByRole('link', { name: 'public site' });
    expect(external).toHaveAttribute('href', 'https://example.test/reference');
    expect(external).toHaveAttribute('target', '_blank');
    expect(external).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.queryByRole('link', { name: 'contract' })).toBeNull();
  });

  it('loads the complete real repository sets through the raw Vite glob', () => {
    expect(SDK_DOCUMENTS.typescript).toHaveLength(10);
    expect(SDK_DOCUMENTS.rust).toHaveLength(9);
    expect(SDK_DOCUMENTS.typescript[0]?.label).toBe('Overview');
    expect(SDK_DOCUMENTS.typescript.map((document) => document.label)).toContain('Sponsorships');
    expect(SDK_DOCUMENTS.rust.at(-1)?.label).toBe('Reference');
  });

  it('keeps the documented responsive geometry and token boundary in its stylesheet', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/developer/DeveloperSurface.css'), 'utf8');
    expect(css).toMatch(
      /\.developer__layout\s*\{[^}]*grid-template-columns: 200px minmax\(0, var\(--read-column\)\)[^}]*gap: 32px/,
    );
    expect(css).toMatch(
      /@media \(max-width: 47\.999rem\)[\s\S]*grid-template-columns: minmax\(0, 1fr\)[^}]*gap: 16px/,
    );
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b/i);
  });

  it('scrolls a linked fragment after the target document renders', async () => {
    const originalScrollIntoView = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      'scrollIntoView',
    );
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
      configurable: true,
      value: scrollIntoView,
    });

    try {
      const user = userEvent.setup();
      render(<DeveloperSurface documents={DOCUMENTS} />);

      await user.click(screen.getByRole('link', { name: 'Authentication' }));

      expect(scrollIntoView).toHaveBeenCalledOnce();
    } finally {
      if (originalScrollIntoView === undefined) {
        Reflect.deleteProperty(HTMLElement.prototype, 'scrollIntoView');
      } else {
        Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', originalScrollIntoView);
      }
    }
  });
});
