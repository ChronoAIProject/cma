import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { MessageContent } from './MessageContent';

it.each([
  ['https://example.test/en/about/previous-releases', '；访问说明'],
  ['https://docs.example.test/cli/v12/commands/npm-ci/?a=1#options', '；后续文字'],
])('ends a bare %s link before the Chinese semicolon', (url, suffix) => {
  const { container, rerender } = render(<MessageContent content={url + suffix} streaming />);
  const assertBoundary = () => {
    const links = screen.getAllByRole('link');
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute('href', url);
    expect(links[0]?.textContent).toBe(url);
    expect(links[0]?.nextSibling?.textContent).toBe(suffix);
    expect(container.querySelector('p')?.textContent).toBe(url + suffix);
  };
  assertBoundary();
  rerender(<MessageContent content={url + suffix} />);
  assertBoundary();
});

it('keeps GFM www links and encoded URL punctuation', () => {
  render(<MessageContent content="www.example.test/；访问 https://example.test/a%EF%BC%9Bb" />);
  expect(screen.getByRole('link', { name: 'www.example.test/' })).toHaveAttribute(
    'href',
    'http://www.example.test/',
  );
  expect(screen.getByRole('link', { name: 'https://example.test/a%EF%BC%9Bb' })).toHaveAttribute(
    'href',
    'https://example.test/a%EF%BC%9Bb',
  );
});

it.each(['\uFEFFhttps:', 'https&#58;'])('ends a link after GFM parses the %s prefix', (prefix) => {
  render(<MessageContent content={`${prefix}//example.test/path；后续`} />);
  const link = screen.getByRole('link', { name: 'https://example.test/path' });
  expect(link).toHaveAttribute('href', 'https://example.test/path');
  expect(link.nextSibling?.textContent).toBe('；后续');
});

it('preserves semicolons inside explicit Markdown links and inline code', () => {
  const url = 'https://example.test/path；详情';
  const { container } = render(<MessageContent content={`[${url}](${url}) <${url}> \`${url}\``} />);
  const links = screen.getAllByRole('link');
  expect(links).toHaveLength(2);
  for (const link of links) {
    expect(link).toHaveAttribute('href', 'https://example.test/path%EF%BC%9B%E8%AF%A6%E6%83%85');
    expect(link.textContent).toBe(url);
  }
  expect(container.querySelector('code')?.textContent).toBe(url);
});

it.each([
  ['检查日期：', '2026 年'],
  ['检查范围：', '仅对当前本地工作区'],
  ['QA 结论：证据不足，暂不具备通过验收的依据。', '上述为结果'],
  ['中文', '文字'],
  ['检查日期：', ' 2026 年'],
  ['English:', ' text'],
  ['確認：', '次の項目'],
  ['확인：', '다음 항목'],
])('renders emphasis around %s without changing adjacent text', (label, suffix) => {
  const { container } = render(<MessageContent content={`**${label}**${suffix}`} />);
  expect(container.querySelectorAll('strong')).toHaveLength(1);
  expect(container.querySelector('strong')?.textContent).toBe(label);
  expect(container.querySelector('p')?.textContent).toBe(label + suffix);
});

it('uses the same CJK boundaries for partial, live and completed messages', () => {
  const partial = '**检查日期：';
  const content = `${partial}**2026 年`;
  const { container, rerender } = render(<MessageContent content={partial} streaming />);
  expect(container.querySelector('p')?.textContent).toBe(partial);
  expect(container.querySelector('strong')).toBeNull();
  rerender(<MessageContent content={content} streaming />);
  expect(container.querySelector('strong')?.textContent).toBe('检查日期：');
  expect(container.querySelector('p')?.textContent).toBe('检查日期：2026 年');
  const live = container.querySelector('.agent-content')!.innerHTML;
  rerender(<MessageContent content={content} />);
  expect(container.querySelector('.agent-content')!.innerHTML).toBe(live);
  expect(container.querySelectorAll('strong')).toHaveLength(1);
  expect(container.querySelector('.agent-content')).not.toHaveAttribute('data-streaming');
});

it('keeps CJK emphasis nested in lists, tables and link labels', () => {
  const { container } = render(
    <MessageContent
      content={
        '- **检查：*完成。***继续\n- *斜体：*继续\n\n| 结果 |\n| --- |\n| **范围：**本地 |\n\n[**报告：**详情](https://example.test/report)'
      }
    />,
  );
  expect(container.querySelector('li strong em')?.textContent).toBe('完成。');
  expect(screen.getAllByRole('listitem').map((item) => item.textContent)).toEqual([
    '检查：完成。继续',
    '斜体：继续',
  ]);
  expect(screen.getByRole('cell').textContent).toBe('范围：本地');
  const link = screen.getByRole('link', { name: '报告：详情' });
  expect(link.querySelector('strong')?.textContent).toBe('报告：');
  expect(link).toHaveAttribute('href', 'https://example.test/report');
  expect(link).toHaveAttribute('rel', 'noopener noreferrer');
});

it.each([false, true])(
  'preserves literal stars and unpaired markers (streaming=%s)',
  (streaming) => {
    const { container } = render(
      <MessageContent
        streaming={streaming}
        content={
          '`**检查日期：**2026 年`\n\n```text\n**检查范围：**本地\n```\n\n\\*\\*文字\\*\\*\n\n**未完成：继续阅读'
        }
      />,
    );
    expect([...container.querySelectorAll('code')].map((code) => code.textContent)).toEqual([
      '**检查日期：**2026 年',
      '**检查范围：**本地\n',
    ]);
    expect(screen.getByText('**文字**').textContent).toBe('**文字**');
    expect(screen.getByText('**未完成：继续阅读').textContent).toBe('**未完成：继续阅读');
    expect(container.querySelector('strong,em')).toBeNull();
  },
);

it.each([false, true])(
  'preserves URL and raw HTML safety with CJK text (inert=%s)',
  (inertResources) => {
    const raw = '<img src="https://example.test/tracker" onerror="alert(1)">';
    const { container } = render(
      <MessageContent
        inertResources={inertResources}
        content={`**检查：**继续\n\n${raw}\n\n[**危险：**链接](javascript:alert(1))\n\n[**安全：**链接](https://example.test/report)\n\n![图片](data:image/svg+xml;base64,PHN2Zz4=)`}
      />,
    );
    expect(container.querySelector('strong')?.textContent).toBe('检查：');
    expect(container).toHaveTextContent(raw);
    expect(container.querySelector('img,script,svg,iframe,[onerror]')).toBeNull();
    expect(screen.queryByRole('link', { name: '危险：链接' })).not.toBeInTheDocument();
    expect(screen.queryAllByRole('link')).toHaveLength(inertResources ? 0 : 1);
    expect(screen.getByText('图片')).toBeVisible();
  },
);
