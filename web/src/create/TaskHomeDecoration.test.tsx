import { useContext, useEffect, type ComponentProps } from 'react';
import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import { emptyTemplate } from './taskTemplateModel';
import { TaskHomeLayout } from './TaskHomeLayout';
import { TaskHomeDecorationContext } from './TaskHomeDecoration';

type Props = ComponentProps<typeof TaskHomeLayout>;
const props: Props = {
  titleId: 'home-title',
  contextual: false,
  submitted: false,
  home: { navigation: { status: 'ready', items: [] } },
  onNavigate: vi.fn(),
  children: <textarea aria-label="Your task" defaultValue="Keep my draft" />,
};

it('renders one static rest poster with no decoration provider', () => {
  function DefaultSlot() {
    const value = useContext(TaskHomeDecorationContext);
    expect(value).toBeNull();
    return <TaskHomeLayout {...props} />;
  }
  const { container } = render(<DefaultSlot />);
  expect(container.querySelector('.task-home__decoration')).toBeEmptyDOMElement();
  const mascot = container.querySelector('.task-home__first-mascot');
  expect(mascot).toHaveAttribute('aria-hidden', 'true');
  expect(mascot?.querySelectorAll('.agent-mascot__poster')).toHaveLength(1);
  expect(mascot?.querySelector('.agent-mascot__sprite')).toBeNull();
  expect(screen.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
});

const excluded: [string, Partial<Props>][] = [
  ['no navigation', { home: undefined }],
  ['loading', { home: { navigation: { status: 'loading' } } }],
  ['error', { home: { navigation: { status: 'error', message: 'Unavailable' } } }],
  [
    'stale empty navigation',
    { home: { navigation: { status: 'ready', items: [], refreshError: 'Unavailable' } } },
  ],
  ['crew', { home: { navigation: { status: 'ready', items: navigationFixture() } } }],
  ['shared scope', { home: { ...props.home, scope: 'shared' } }],
  ['organization scope', { home: { ...props.home, scope: 'organization' } }],
  ['contextual', { contextual: true, contextLabel: 'Existing workspace' }],
  ['template', { template: emptyTemplate('research') }],
  ['submitted', { submitted: true }],
];

it.each(excluded)('does not mount decoration for %s', (_, excludedProps) => {
  const mount = vi.fn();
  const dispose = vi.fn();
  function Decoration() {
    useEffect(() => {
      mount();
      return dispose;
    }, []);
    return <span data-testid="decoration">Synthetic decoration</span>;
  }
  const view = (overrides: Partial<Props>) => (
    <TaskHomeDecorationContext.Provider value={<Decoration />}>
      <TaskHomeLayout {...props} {...overrides} />
    </TaskHomeDecorationContext.Provider>
  );
  const { rerender } = render(view(excludedProps));
  expect(screen.queryByTestId('decoration')).not.toBeInTheDocument();
  expect(mount).not.toHaveBeenCalled();
  const input = screen.getByLabelText('Your task');

  rerender(view({}));
  expect(screen.getByTestId('decoration').parentElement).toHaveAttribute('aria-hidden', 'true');
  expect(mount).toHaveBeenCalledOnce();
  expect(screen.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
  expect(screen.getByLabelText('Your task')).toBe(input);

  rerender(view(excludedProps));
  expect(screen.queryByTestId('decoration')).not.toBeInTheDocument();
  expect(dispose).toHaveBeenCalledOnce();
  expect(screen.getByLabelText('Your task')).toBe(input);
  expect(input).toHaveValue('Keep my draft');
});
