import { describe, expect, it } from 'vitest';

import { emptyConversation } from './conversation';
import type { Conversation } from './conversation';
import { resolveDetailPanel } from './detailPanel';
import type { DetailPanelSelection } from './detailPanel';
import type { Subagent } from './types';

const agent: Subagent = {
  id: 'worker',
  toolCallId: 'call',
  name: 'explore',
  displayName: 'Inspect files',
  description: 'Read project files',
  status: 'running',
};
const selection: DetailPanelSelection = {
  kind: 'subagent',
  sessionId: 'session',
  viewKey: 'view',
  agentId: agent.id,
};
const conversation: Conversation = {
  ...emptyConversation,
  messages: [
    {
      id: 'message',
      sessionId: 'session',
      role: 'agent',
      content: '',
      createdAt: '',
      attachments: [],
      agent,
    },
  ],
};

describe('detail panel selection', () => {
  it('starts closed and resolves the current agent rather than a captured snapshot', () => {
    expect(resolveDetailPanel(null, 'view', 'session', conversation)).toBeNull();
    expect(resolveDetailPanel(selection, 'view', 'session', conversation)?.agent.status).toBe(
      'running',
    );
    const updated: Conversation = {
      ...conversation,
      messages: conversation.messages.map((message) => ({
        ...message,
        agent: { ...agent, status: 'completed' },
      })),
    };
    expect(resolveDetailPanel(selection, 'view', 'session', updated)?.agent.status).toBe(
      'completed',
    );
  });

  it.each([
    ['another view', 'session'],
    ['view', 'another session'],
    ['view', undefined],
  ])('does not leak output across navigation (%s, %s)', (viewKey, sessionId) => {
    expect(resolveDetailPanel(selection, viewKey ?? '', sessionId, conversation)).toBeNull();
  });

  it('does not reuse a missing agent or another session’s matching identifier', () => {
    expect(resolveDetailPanel(selection, 'view', 'session', emptyConversation)).toBeNull();
    const other = {
      ...conversation,
      messages: conversation.messages.map((message) => ({ ...message, sessionId: 'other' })),
    };
    expect(resolveDetailPanel(selection, 'view', 'session', other)).toBeNull();
    expect(
      resolveDetailPanel({ ...selection, agentId: 'other' }, 'view', 'session', conversation),
    ).toBeNull();
  });
});
