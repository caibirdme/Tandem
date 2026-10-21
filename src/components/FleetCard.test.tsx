import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { Message, Subagent } from '../lib/types';
import { AgentOutput, FleetCard } from './FleetCard';

const first: Subagent = {
  id: 'worker-a',
  toolCallId: 'call-a',
  name: 'explore',
  displayName: 'Inspect permissions',
  description: 'Trace the approval flow',
  status: 'running',
  model: 'test-model',
};
const second: Subagent = {
  ...first,
  id: 'worker-b',
  toolCallId: 'call-b',
  displayName: 'Review persistence',
  status: 'completed',
};
const messages: Message[] = [
  {
    id: 'a-output',
    sessionId: 'session',
    agentId: first.id,
    role: 'assistant',
    content: 'Only worker A reported this finding.',
    createdAt: '2026-10-21T10:00:00Z',
    attachments: [],
  },
  {
    id: 'a-tools',
    sessionId: 'session',
    agentId: first.id,
    role: 'activity',
    content: 'Tracing approvals',
    activity: {
      status: 'completed',
      tools: [
        {
          toolCallId: 'read-a',
          name: 'view',
          status: 'completed',
          arguments: { path: '/project/permissions.rs' },
          result: 'Worker A tool result',
        },
      ],
    },
    createdAt: '2026-10-21T10:00:01Z',
    attachments: [],
  },
  {
    id: 'b-output',
    sessionId: 'session',
    agentId: second.id,
    role: 'assistant',
    content: 'Only worker B reported this finding.',
    createdAt: '2026-10-21T10:00:02Z',
    attachments: [],
  },
];

afterEach(() => setLocale('en'));

describe('Multi-agent inspection', () => {
  it('lists concurrent workers without exposing their transcripts until selected', () => {
    const markup = renderToStaticMarkup(
      <FleetCard
        agents={[first, second]}
        messages={messages}
        streams={{}}
        selectedAgentId={null}
        onSelect={() => undefined}
      />,
    );
    expect(markup).toContain('1 running');
    expect(markup).toContain('Inspect permissions');
    expect(markup).toContain('Review persistence');
    expect(markup).toContain('Running');
    expect(markup).toContain('Completed');
    expect(markup).not.toContain('Only worker');
    expect(markup).not.toContain('Worker A tool result');
    expect(markup).not.toContain('fleet-inspector');
    expect(markup.match(/aria-pressed="false"/g)).toHaveLength(2);
  });

  it('shows only the selected worker’s output, tools, and live stream', () => {
    const markup = renderToStaticMarkup(
      <FleetCard
        agents={[first, second]}
        messages={messages}
        streams={{ [first.id]: 'Worker A live update', [second.id]: 'Worker B live update' }}
        selectedAgentId={first.id}
        onSelect={() => undefined}
      />,
    );
    expect(markup).toContain('aria-label="Output from Inspect permissions"');
    expect(markup).toContain('Only worker A reported this finding.');
    expect(markup).toContain('/project/permissions.rs');
    expect(markup).toContain('Worker A tool result');
    expect(markup).toContain('Worker A live update');
    expect(markup).not.toContain('Only worker B reported');
    expect(markup).not.toContain('Worker B live update');
    expect(markup).toContain('test-model');
    expect(markup).toContain('Back to conversation');
    expect(markup.match(/aria-pressed="true"/g)).toHaveLength(1);
  });

  it('switches the inspected trace without showing a sibling’s tools', () => {
    const markup = renderToStaticMarkup(
      <FleetCard
        agents={[first, second]}
        messages={messages}
        streams={{}}
        selectedAgentId={second.id}
        onSelect={() => undefined}
      />,
    );
    expect(markup).toContain('Only worker B reported this finding.');
    expect(markup).not.toContain('Only worker A reported');
    expect(markup).not.toContain('Worker A tool result');
  });

  it('shows a failed worker’s error even when it has no output', () => {
    const markup = renderToStaticMarkup(
      <AgentOutput
        agent={{ ...first, status: 'error', error: 'Worker could not read the project' }}
        messages={[]}
        stream=""
      />,
    );
    expect(markup).toContain('Worker could not read the project');
    expect(markup).not.toContain('Waiting for');
  });

  it('translates worker state and inspection controls', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <FleetCard
        agents={[first]}
        messages={[]}
        streams={{}}
        selectedAgentId={first.id}
        onSelect={() => undefined}
      />,
    );
    expect(markup).toContain('1 个运行中');
    expect(markup).toContain('返回主对话');
    expect(markup).toContain('正在等待此智能体的首次更新');
  });
});
