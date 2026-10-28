import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DeliveryFlowCanvas } from './DeliveryFlowCanvas';
import type { ChannelNode } from './channelTopology';
import type { Edge, Node } from '@xyflow/react';

const { capture } = vi.hoisted(() => ({ capture: vi.fn() }));
vi.mock('@xyflow/react', () => ({
  Background: () => null,
  Controls: () => null,
  Handle: () => null,
  Position: { Left: 'left', Right: 'right' },
  MarkerType: { ArrowClosed: 'arrowclosed' },
  useReactFlow: () => ({ fitView: vi.fn() }),
  ReactFlow: (props: { nodes: Node[]; edges: Edge[]; children: React.ReactNode }) => {
    capture(props);
    return <div>{props.children}</div>;
  },
}));
afterEach(() => {
  vi.unstubAllGlobals();
  capture.mockClear();
});
const node = (key: string, selection: ChannelNode['selection']): ChannelNode => ({
  key,
  selection,
  title: key,
  subtitle: key,
  detail: key,
});
const groups = [
  [node('source', { kind: 'producer', index: 0 })],
  [node('channel', { kind: 'channel' })],
  [node('agent', { kind: 'consumer', id: 'default' })],
];

it('shows directed routing motion, static tool associations, and accessible overlay content', () => {
  render(
    <DeliveryFlowCanvas
      groups={groups}
      toolsets={node('toolsets', { kind: 'channel' })}
      overlay={<button>Activity</button>}
    />,
  );
  const props = capture.mock.lastCall![0] as { nodes: Node[]; edges: Edge[] };
  expect(
    props.edges.filter((edge) => edge.animated).map(({ source, target }) => [source, target]),
  ).toEqual([
    ['source', 'channel'],
    ['channel', 'agent'],
  ]);
  expect(
    props.edges
      .filter((edge) => edge.source !== 'toolsets')
      .every(
        (edge) =>
          edge.markerEnd &&
          typeof edge.markerEnd !== 'string' &&
          edge.markerEnd.type === 'arrowclosed',
      ),
  ).toBe(true);
  expect(props.edges.find((edge) => edge.source === 'toolsets')).toMatchObject({
    target: 'agent',
    animated: false,
    label: 'Tools',
  });
  expect(props.edges.find((edge) => edge.source === 'toolsets')?.markerEnd).toBeUndefined();
  expect(props.nodes.find((value) => value.id === 'toolsets')!.position.y).toBeGreaterThan(
    props.nodes.find((value) => value.id === 'channel')!.position.y,
  );
  expect(
    screen.getByRole('button', { name: 'Activity' }).closest('.delivery-flow__overlay'),
  ).not.toBeNull();
});

it('keeps pending consumers visible without suggesting a tool connection', () => {
  const pending = { ...node('pending', { kind: 'consumer', id: 'pending' }), pending: true };
  render(
    <DeliveryFlowCanvas
      groups={[groups[0]!, groups[1]!, [...groups[2]!, pending]]}
      toolsets={node('toolsets', { kind: 'toolsets' })}
    />,
  );
  const props = capture.mock.lastCall![0] as { nodes: Node[]; edges: Edge[] };
  expect(props.nodes.some((value) => value.id === 'pending')).toBe(true);
  expect(
    props.edges.filter((edge) => edge.source === 'toolsets').map((edge) => edge.target),
  ).toEqual(['agent']);
});

it('responds to reduced-motion changes by leaving all arrows static', () => {
  let reduced = false;
  let notify = () => {};
  vi.stubGlobal('matchMedia', () => ({
    matches: reduced,
    addEventListener: (_: string, listener: () => void) => {
      notify = listener;
    },
    removeEventListener: vi.fn(),
  }));
  render(<DeliveryFlowCanvas groups={groups} />);
  expect(capture.mock.lastCall![0].edges.some((edge: Edge) => edge.animated)).toBe(true);
  act(() => {
    reduced = true;
    notify();
  });
  expect(
    capture.mock.lastCall![0].edges.every((edge: Edge) => !edge.animated && edge.markerEnd),
  ).toBe(true);
});
