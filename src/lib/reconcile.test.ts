import { describe, expect, it } from 'vitest';

import { emptyConversation } from './conversation';
import type { Conversation } from './conversation';
import { protectedSessionIds, reconcileConversations, reconcileSessions } from './reconcile';
import type { Session } from './types';

const session: Session = {
  reasoningEffort: null,
  contextUsage: null,
  titleState: 'preserved',
  id: 'session-1',
  title: 'A task',
  projectId: null,
  providerId: 'provider-1',
  modelId: 'model-1',
  createdAt: '2026-10-17T00:00:00Z',
  updatedAt: '2026-10-17T00:00:00Z',
  status: 'idle',
  permissionMode: 'ask',
};
const permission = { requestId: 'request-1', kind: 'write', description: 'Write file' };

describe('reconnecting to a running or completed session', () => {
  it.each(['idle', 'running'] as const)(
    'clears stale output and approvals when the %s snapshot has no active turn',
    (status) => {
      const current: Record<string, Conversation> = {
        [session.id]: {
          ...emptyConversation,
          status: 'running',
          stream: 'partial',
          permissions: [permission],
          messages: [
            {
              id: 'activity-1',
              sessionId: session.id,
              role: 'activity',
              content: 'Creating a file',
              attachments: [],
              createdAt: session.createdAt,
              activity: {
                status: 'running',
                tools: [{ toolCallId: 'tool-1', name: 'create', status: 'running' }],
              },
            },
          ],
        },
      };
      const restored = reconcileConversations(
        current,
        { sessions: [{ ...session, status }], activeTurns: {} },
        new Set(),
      );
      expect(restored[session.id]).toMatchObject({
        status,
        stream: '',
        permissions: [],
        messages: [
          {
            activity: {
              status: status === 'running' ? 'running' : 'interrupted',
              tools: [{ status: status === 'running' ? 'running' : 'interrupted' }],
            },
          },
        ],
      });
    },
  );

  it('restores live output and pending approval on a fresh page', () => {
    const restored = reconcileConversations(
      {},
      {
        sessions: [{ ...session, status: 'running' }],
        activeTurns: { [session.id]: { stream: 'Current output', permissions: [permission] } },
      },
      new Set(),
    );
    expect(restored[session.id]).toMatchObject({
      status: 'running',
      stream: 'Current output',
      permissions: [permission],
    });
  });

  it.each([
    {
      name: 'a send began after the snapshot request',
      requested: {},
      current: { 'session-1': 1 },
      before: [],
      after: ['session-1'],
    },
    {
      name: 'a send is still waiting for acknowledgement',
      requested: { 'session-1': 1 },
      current: { 'session-1': 1 },
      before: ['session-1'],
      after: ['session-1'],
    },
    {
      name: 'a send finished while the snapshot was in flight',
      requested: { 'session-1': 1 },
      current: { 'session-1': 2 },
      before: ['session-1'],
      after: [],
    },
    {
      name: 'a newer stream event already arrived',
      requested: { 'session-1': 1 },
      current: { 'session-1': 2 },
      before: [],
      after: [],
    },
  ])('preserves newer local state when $name', ({ requested, current, before, after }) => {
    const protectedIds = protectedSessionIds(requested, current, new Set(before), new Set(after));
    const running = {
      ...emptyConversation,
      status: 'running' as const,
      stream: 'Newer output',
      permissions: [permission],
    };
    const restored = reconcileConversations(
      { [session.id]: running },
      { sessions: [session], activeTurns: {} },
      protectedIds,
    );
    expect(restored[session.id]).toBe(running);
    expect(
      reconcileSessions([{ ...session, status: 'running' }], [session], protectedIds)[0]?.status,
    ).toBe('running');
  });

  it('keeps a just-created session absent from an older snapshot', () => {
    const protectedIds = protectedSessionIds({}, { [session.id]: 1 }, new Set(), new Set());
    expect(reconcileSessions([session], [], protectedIds)).toEqual([session]);
  });
});
