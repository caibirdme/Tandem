import { describe, expect, it } from 'vitest';

import { protectedSessionIds } from './reconcile';
import { initialTandemState, reduceTandem } from './tandemState';
import type { Message, Session, Snapshot } from './types';

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
const snapshot: Snapshot = {
  titleGenerations: [],
  sessions: [session],
  projects: [],
  settings: {
    providers: [],
    titleModel: null,
    telemetry: { enabled: false, endpoint: '', protocol: 'http/json', captureContent: false },
    cliPath: null,
    fontSize: 'medium',
  },
  runtime: { available: true, version: null, message: 'Ready' },
  activeTurns: {},
};
const pending: Message = {
  attachments: [],
  id: 'pending:1',
  sessionId: session.id,
  role: 'user',
  content: 'Hello',
  createdAt: session.createdAt,
};
const loaded = reduceTandem(initialTandemState, {
  type: 'snapshotReceived',
  snapshot,
  protectedIds: new Set(),
});

describe('context usage state', () => {
  const work = { ...session, projectId: 'project-1' };
  const usage = { currentTokens: 153_000, tokenLimit: 1_000_000 };
  const workState = reduceTandem(loaded, { type: 'sessionAdded', session: work });

  it('replaces current usage after compaction and ignores stale snapshots', () => {
    const measured = reduceTandem(workState, {
      type: 'eventReceived',
      event: { sessionId: work.id, kind: 'context_usage', data: usage },
    });
    const compacted = reduceTandem(measured, {
      type: 'eventReceived',
      event: {
        sessionId: work.id,
        kind: 'context_usage',
        data: { ...usage, currentTokens: 20_000 },
      },
    });
    const refreshed = reduceTandem(compacted, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, sessions: [work] },
      protectedIds: new Set([work.id]),
    });
    expect(refreshed.snapshot?.sessions[0]?.contextUsage?.currentTokens).toBe(20_000);
    expect(compacted.conversations).toBe(workState.conversations);
  });

  it('restores persisted usage from a fresh snapshot and clears it after provider changes', () => {
    const restored = reduceTandem(initialTandemState, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, sessions: [{ ...work, contextUsage: usage }] },
      protectedIds: new Set(),
    });
    expect(restored.snapshot?.sessions[0]?.contextUsage).toEqual(usage);
    const cleared = reduceTandem(restored, {
      type: 'eventReceived',
      event: { sessionId: work.id, kind: 'context_usage', data: null },
    });
    expect(cleared.snapshot?.sessions[0]?.contextUsage).toBeNull();
  });

  it('clears usage when the model changes and retains it when confirming the same model', () => {
    const measured = reduceTandem(workState, {
      type: 'eventReceived',
      event: { sessionId: work.id, kind: 'context_usage', data: usage },
    });
    const confirmed = reduceTandem(measured, {
      type: 'modelChanged',
      sessionId: work.id,
      selection: { providerId: work.providerId, modelId: work.modelId },
    });
    expect(confirmed.snapshot?.sessions[0]?.contextUsage).toEqual(usage);
    const changed = reduceTandem(confirmed, {
      type: 'modelChanged',
      sessionId: work.id,
      selection: { providerId: work.providerId, modelId: 'other' },
    });
    expect(changed.snapshot?.sessions[0]?.contextUsage).toBeNull();
  });

  it('ignores usage for Chat and does not resurrect deleted sessions', () => {
    const chat = reduceTandem(loaded, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'context_usage', data: usage },
    });
    expect(chat.snapshot?.sessions[0]?.contextUsage).toBeNull();
    const removed = reduceTandem(workState, { type: 'sessionsDeleted', sessionIds: [work.id] });
    expect(
      reduceTandem(removed, {
        type: 'eventReceived',
        event: { sessionId: work.id, kind: 'context_usage', data: usage },
      }),
    ).toBe(removed);
  });
});

describe('workspace state transitions', () => {
  it.each(['idle', 'error'] as const)(
    'does not revive running activity from stale history after a reconnect reports %s',
    (status) => {
      const activity: Message = {
        attachments: [],
        id: 'activity-1',
        sessionId: session.id,
        role: 'activity',
        content: 'Checking the project',
        createdAt: session.createdAt,
        sequence: 2,
        activity: {
          status: 'running',
          tools: [{ toolCallId: 'tool-1', name: 'read_file', status: 'running' }],
        },
      };
      const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
      const working = reduceTandem(sending, {
        type: 'eventReceived',
        event: { sessionId: session.id, kind: 'message', data: activity },
      });
      const reconnected = reduceTandem(working, {
        type: 'snapshotReceived',
        snapshot: { ...snapshot, sessions: [{ ...session, status }] },
        protectedIds: new Set(),
      });
      const restored = reduceTandem(reconnected, {
        type: 'historyReceived',
        sessionId: session.id,
        messages: [activity],
      });
      expect(restored.conversations[session.id]?.status).toBe(status);
      expect(restored.snapshot?.sessions[0]?.status).toBe(status);
      expect(
        restored.conversations[session.id]?.messages.find((item) => item.id === activity.id),
      ).toMatchObject({
        activity: { status: 'interrupted', tools: [{ status: 'interrupted' }] },
      });
    },
  );

  it('restores active tool activity from history when the session remains running', () => {
    const activity: Message = {
      attachments: [],
      id: 'activity-1',
      sessionId: session.id,
      role: 'activity',
      content: 'Checking the project',
      createdAt: session.createdAt,
      activity: {
        status: 'running',
        tools: [{ toolCallId: 'tool-1', name: 'read_file', status: 'running' }],
      },
    };
    const reconnected = reduceTandem(loaded, {
      type: 'snapshotReceived',
      snapshot: {
        ...snapshot,
        sessions: [{ ...session, status: 'running' }],
        activeTurns: { [session.id]: { stream: '', permissions: [] } },
      },
      protectedIds: new Set(),
    });
    const restored = reduceTandem(reconnected, {
      type: 'historyReceived',
      sessionId: session.id,
      messages: [activity],
    });
    expect(restored.conversations[session.id]?.messages).toEqual([activity]);
  });

  it('applies background titles without touching running output, approvals or status', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const active = reduceTandem(sending, {
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'permission',
        data: { requestId: 'approval', kind: 'write', description: 'Write fixture' },
      },
    });
    const other = { ...session, id: 'other-session' };
    const withOther = reduceTandem(active, { type: 'sessionAdded', session: other });
    const titled = reduceTandem(withOther, {
      type: 'eventReceived',
      event: {
        sessionId: other.id,
        kind: 'title',
        data: { title: '模型生成的简短标题' },
      },
    });
    expect(titled.snapshot?.sessions.find((item) => item.id === other.id)?.title).toBe(
      '模型生成的简短标题',
    );
    expect(titled.conversations).toBe(withOther.conversations);
    expect(titled.snapshot?.sessions.find((item) => item.id === session.id)?.status).toBe(
      'running',
    );
    const refreshed = reduceTandem(titled, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, sessions: [session, other] },
      protectedIds: new Set([other.id, session.id]),
      titleProtectedIds: new Set([other.id]),
    });
    expect(refreshed.snapshot?.sessions.find((item) => item.id === other.id)?.title).toBe(
      '模型生成的简短标题',
    );
    expect(refreshed.conversations[session.id]?.permissions).toHaveLength(1);
  });

  it('reports title failures independently and allows retry without failing the conversation', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const generating = reduceTandem(sending, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    const failed = reduceTandem(generating, {
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'title_error',
        data: { message: 'Title model unavailable' },
      },
    });
    expect(failed.titleGenerations.size).toBe(0);
    expect(failed.titleErrors[session.id]).toBe('Title model unavailable');
    expect(failed.error).toBeNull();
    expect(failed.conversations).toBe(sending.conversations);
    expect(failed.snapshot?.sessions[0]).toEqual(sending.snapshot?.sessions[0]);
    const retried = reduceTandem(failed, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    expect(retried.titleGenerations.has(session.id)).toBe(true);
    expect(retried.titleErrors[session.id]).toBeUndefined();
  });

  it('reconciles title progress after reconnect without losing a newer local request or completion', () => {
    const generating = reduceTandem(loaded, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    const stale = reduceTandem(generating, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, titleGenerations: [] },
      protectedIds: new Set(),
      titleProtectedIds: new Set([session.id]),
    });
    expect(stale.titleGenerations.has(session.id)).toBe(true);
    const completed = reduceTandem(stale, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'title', data: { title: 'Finished' } },
    });
    const oldBusy = reduceTandem(completed, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, titleGenerations: [session.id] },
      protectedIds: new Set(),
      titleProtectedIds: new Set([session.id]),
    });
    expect(oldBusy.titleGenerations.size).toBe(0);
    const reconnected = reduceTandem(generating, {
      type: 'snapshotReceived',
      snapshot: {
        ...snapshot,
        sessions: [{ ...session, title: 'Finished' }],
        titleGenerations: [],
      },
      protectedIds: new Set(),
    });
    expect(reconnected.titleGenerations.size).toBe(0);
    expect(reconnected.snapshot?.sessions[0]?.title).toBe('Finished');
    const stillRunning = reduceTandem(loaded, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, titleGenerations: [session.id] },
      protectedIds: new Set(),
    });
    expect(stillRunning.titleGenerations.has(session.id)).toBe(true);
  });

  it('keeps a successful manual title over a snapshot requested before renaming', () => {
    const generating = reduceTandem(loaded, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    const renamed = reduceTandem(generating, {
      type: 'sessionRenamed',
      sessionId: session.id,
      title: 'My chosen title',
    });
    const stale = reduceTandem(renamed, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, titleGenerations: [session.id] },
      protectedIds: new Set(),
      titleProtectedIds: protectedSessionIds(
        { [session.id]: 1 },
        { [session.id]: 3 },
        new Set(),
        new Set(),
      ),
    });
    expect(stale.snapshot?.sessions[0]?.title).toBe('My chosen title');
    expect(stale.titleGenerations.size).toBe(0);
  });

  it('recovers a missed title completion while permission confirmation keeps sending locked', () => {
    const work = { ...session, projectId: 'project-1' };
    const withWork = reduceTandem(loaded, { type: 'sessionAdded', session: work });
    const uncertain = reduceTandem(withWork, {
      type: 'permissionModeUnconfirmed',
      sessionId: work.id,
      error: '请重新确认权限',
    });
    const generating = reduceTandem(uncertain, {
      type: 'titleGenerationRequested',
      sessionId: work.id,
    });
    const refreshed = reduceTandem(generating, {
      type: 'snapshotReceived',
      snapshot: {
        ...snapshot,
        sessions: [{ ...work, title: '已生成的标题', permissionMode: 'allow-all' }],
        titleGenerations: [],
      },
      protectedIds: new Set([work.id]),
      titleProtectedIds: new Set(),
    });
    expect(refreshed.snapshot?.sessions[0]).toMatchObject({
      title: '已生成的标题',
      permissionMode: 'ask',
    });
    expect(refreshed.titleGenerations.size).toBe(0);
    expect(refreshed.permissionModeUncertain).toBe(generating.permissionModeUncertain);
    expect(refreshed.conversations[work.id]).toBe(generating.conversations[work.id]);
  });

  it('protects a newer title request without blocking conversation completion from a snapshot', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const approval = reduceTandem(sending, {
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'permission',
        data: { requestId: 'approval', kind: 'write', description: 'Write fixture' },
      },
    });
    const generating = reduceTandem(approval, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    const refreshed = reduceTandem(generating, {
      type: 'snapshotReceived',
      snapshot: {
        ...snapshot,
        sessions: [{ ...session, title: 'Older snapshot title' }],
        titleGenerations: [],
      },
      protectedIds: new Set(),
      titleProtectedIds: new Set([session.id]),
    });
    expect(refreshed.snapshot?.sessions[0]).toMatchObject({ title: session.title, status: 'idle' });
    expect(refreshed.titleGenerations.has(session.id)).toBe(true);
    expect(refreshed.conversations[session.id]).toMatchObject({ status: 'idle', permissions: [] });
  });

  it('ignores delayed title success and failure after deletion and clears their progress', () => {
    const generating = reduceTandem(loaded, {
      type: 'titleGenerationRequested',
      sessionId: session.id,
    });
    const deleted = reduceTandem(generating, { type: 'sessionsDeleted', sessionIds: [session.id] });
    expect(deleted.titleGenerations.size).toBe(0);
    for (const event of [
      { sessionId: session.id, kind: 'title', data: { title: 'Late title' } },
      { sessionId: session.id, kind: 'title_error', data: { message: 'Late failure' } },
    ] as const)
      expect(reduceTandem(deleted, { type: 'eventReceived', event })).toBe(deleted);
    const stale = reduceTandem(deleted, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, titleGenerations: [session.id] },
      protectedIds: new Set([session.id]),
    });
    expect(stale.snapshot?.sessions).toEqual([]);
    expect(stale.titleGenerations.size).toBe(0);
  });

  it.each(['session', 'project'] as const)(
    'forgets deleted %s state and ignores its late snapshots, history and permission results',
    (scope) => {
      const work = { ...session, projectId: 'project-1' };
      const other = { ...session, id: 'session-2' };
      const anotherWork = { ...work, id: 'session-3' };
      const original: Snapshot = {
        ...snapshot,
        projects: [
          {
            id: 'project-1',
            name: 'Work',
            path: '/tmp/work',
            connection: { kind: 'local' },
            createdAt: session.createdAt,
          },
        ],
        sessions: [work, other, ...(scope === 'project' ? [anotherWork] : [])],
        activeTurns: { [work.id]: { stream: 'Partial', permissions: [] } },
      };
      let state = reduceTandem(initialTandemState, {
        type: 'snapshotReceived',
        snapshot: original,
        protectedIds: new Set(),
      });
      state = reduceTandem(state, {
        type: 'permissionModeUnconfirmed',
        sessionId: work.id,
        error: 'Unknown mode',
      });
      state = reduceTandem(state, { type: 'permissionModeChangeStarted', sessionId: work.id });
      const otherConversation = state.conversations[other.id];
      const deleted = reduceTandem(state, {
        type: 'sessionsDeleted',
        sessionIds: scope === 'session' ? [work.id] : [],
        ...(scope === 'project' ? { projectId: work.projectId } : {}),
      });
      expect(deleted.conversations[work.id]).toBeUndefined();
      expect(deleted.conversations[anotherWork.id]).toBeUndefined();
      expect(deleted.conversations[other.id]).toBe(otherConversation);
      expect(deleted.permissionModeUpdates.size).toBe(0);
      expect(deleted.permissionModeUncertain[work.id]).toBeUndefined();
      expect(deleted.snapshot?.activeTurns).toEqual({});
      const lateActions: Parameters<typeof reduceTandem>[1][] = [
        { type: 'historyReceived', sessionId: work.id, messages: [pending] },
        {
          type: 'eventReceived',
          event: {
            sessionId: work.id,
            kind: 'status',
            data: { status: 'running' },
          },
        },
        {
          type: 'eventReceived',
          event: { sessionId: work.id, kind: 'title', data: { title: 'Late title' } },
        },
        { type: 'permissionModeUnconfirmed', sessionId: work.id, error: 'Late failure' },
        { type: 'permissionModeChanged', sessionId: work.id, permissionMode: 'allow-all' },
        { type: 'sessionAdded', session: work },
      ];
      for (const action of lateActions) expect(reduceTandem(deleted, action)).toBe(deleted);
      const refreshed = reduceTandem(deleted, {
        type: 'snapshotReceived',
        snapshot: original,
        protectedIds: new Set([work.id]),
      });
      expect(refreshed.snapshot?.sessions.map((item) => item.id)).toEqual([other.id]);
      expect(refreshed.snapshot?.projects).toHaveLength(scope === 'project' ? 0 : 1);
      expect(refreshed.conversations[work.id]).toBeUndefined();
      expect(refreshed.snapshot?.activeTurns).toEqual({});
      if (scope === 'project') {
        const lateCreated = reduceTandem(refreshed, {
          type: 'sessionAdded',
          session: { ...work, id: 'created-late' },
        });
        expect(lateCreated.snapshot?.sessions).toEqual(refreshed.snapshot?.sessions);
        expect(lateCreated.deletedSessionIds.has('created-late')).toBe(true);
      }
    },
  );

  it('keeps the saved permission mode until a change succeeds and preserves it on failure', () => {
    const changing = reduceTandem(loaded, {
      type: 'permissionModeChangeStarted',
      sessionId: session.id,
    });
    expect(changing.permissionModeUpdates.has(session.id)).toBe(true);
    expect(changing.snapshot?.sessions[0]?.permissionMode).toBe('ask');
    const failed = reduceTandem(changing, {
      type: 'permissionModeUnconfirmed',
      sessionId: session.id,
      error: 'Could not save the new permission mode',
    });
    expect(failed.permissionModeUpdates.has(session.id)).toBe(false);
    expect(failed.snapshot?.sessions[0]?.permissionMode).toBe('ask');
    expect(failed.permissionModeUncertain[session.id]).toBe(
      'Could not save the new permission mode',
    );
  });

  it('applies a saved permission mode only to its session without rolling back newer runtime state', () => {
    const other = { ...session, id: 'session-2', projectId: 'project-1' };
    const withOther = reduceTandem(loaded, { type: 'sessionAdded', session: other });
    const changing = reduceTandem(withOther, {
      type: 'permissionModeChangeStarted',
      sessionId: other.id,
    });
    const running = reduceTandem(changing, {
      type: 'eventReceived',
      event: {
        sessionId: other.id,
        kind: 'status',
        data: { status: 'running' },
      },
    });
    const titled = reduceTandem(running, {
      type: 'eventReceived',
      event: { sessionId: other.id, kind: 'title', data: { title: 'A newer task title' } },
    });
    const saved = reduceTandem(titled, {
      type: 'permissionModeChanged',
      sessionId: other.id,
      permissionMode: 'allow-all',
    });
    expect(saved.permissionModeUpdates.size).toBe(0);
    expect(saved.snapshot?.sessions.find((item) => item.id === other.id)).toMatchObject({
      permissionMode: 'allow-all',
      status: 'running',
      title: 'A newer task title',
    });
    expect(saved.snapshot?.sessions.find((item) => item.id === session.id)).toBe(
      loaded.snapshot?.sessions[0],
    );
    const protectedIds = protectedSessionIds(
      { [other.id]: 1 },
      { [other.id]: 3 },
      new Set([other.id]),
      new Set(),
    );
    const refreshed = reduceTandem(saved, {
      type: 'snapshotReceived',
      snapshot: { ...snapshot, sessions: [session, other] },
      protectedIds,
    });
    expect(refreshed.snapshot?.sessions.find((item) => item.id === other.id)?.permissionMode).toBe(
      'allow-all',
    );
  });

  it('updates a first-turn title while the session is already running', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const titled = reduceTandem(sending, {
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'title',
        data: { title: 'Read the project and verify the result' },
      },
    });
    expect(titled.snapshot?.sessions[0]).toMatchObject({
      title: 'Read the project and verify the result',
      status: 'running',
    });
    const approval = reduceTandem(titled, {
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'permission',
        data: { requestId: 'request-1', kind: 'shell', description: 'Verify the result' },
      },
    });
    const protectedIds = protectedSessionIds(
      { [session.id]: 1 },
      { [session.id]: 3 },
      new Set(),
      new Set(),
    );
    const refreshed = reduceTandem(approval, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds,
      titleProtectedIds: protectedIds,
    });
    expect(refreshed.snapshot?.sessions[0]).toEqual(titled.snapshot?.sessions[0]);
    expect(refreshed.conversations[session.id]?.permissions).toHaveLength(1);
  });

  it('updates another session title without requiring its history to be loaded', () => {
    const other = { ...session, id: 'session-2', title: 'New work' };
    const withOther = reduceTandem(loaded, { type: 'sessionAdded', session: other });
    const running = reduceTandem(withOther, {
      type: 'eventReceived',
      event: {
        sessionId: other.id,
        kind: 'status',
        data: { status: 'running' },
      },
    });
    const titled = reduceTandem(running, {
      type: 'eventReceived',
      event: { sessionId: other.id, kind: 'title', data: { title: 'Check another project' } },
    });
    expect(titled.snapshot?.sessions.find((item) => item.id === other.id)).toMatchObject({
      title: 'Check another project',
      status: 'running',
    });
    expect(titled.snapshot?.sessions.find((item) => item.id === session.id)).toBe(
      loaded.snapshot?.sessions[0],
    );
    expect(titled.conversations[session.id]).toBe(loaded.conversations[session.id]);

    const completed = reduceTandem(titled, {
      type: 'eventReceived',
      event: { sessionId: other.id, kind: 'status', data: { status: 'idle' } },
    });
    expect(completed.snapshot?.sessions.find((item) => item.id === other.id)).toMatchObject({
      title: 'Check another project',
      status: 'idle',
    });
  });

  it('keeps session and conversation status consistent through send, failure, and recovery', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    expect(sending.snapshot?.sessions[0]?.status).toBe('running');
    expect(sending.conversations[session.id]?.status).toBe('running');

    const failed = reduceTandem(sending, {
      type: 'sendFailed',
      sessionId: session.id,
      pendingId: pending.id,
      error: 'Provider unavailable',
    });
    expect(failed.snapshot?.sessions[0]?.status).toBe('error');
    expect(failed.conversations[session.id]).toMatchObject({
      status: 'error',
      messages: [],
      error: 'Provider unavailable',
    });

    const recovered = reduceTandem(failed, { type: 'sendStarted', message: pending });
    expect(recovered.snapshot?.sessions[0]?.status).toBe('running');
    expect(recovered.conversations[session.id]?.error).toBeNull();
  });

  it('protects pending sends from an old snapshot, then applies terminal state atomically', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const protectedState = reduceTandem(sending, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: new Set([session.id]),
    });
    expect(protectedState.snapshot?.sessions[0]?.status).toBe('running');
    expect(protectedState.conversations[session.id]).toBe(sending.conversations[session.id]);

    const completed = reduceTandem(protectedState, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: new Set(),
    });
    expect(completed.snapshot?.sessions[0]?.status).toBe('idle');
    expect(completed.conversations[session.id]?.status).toBe('idle');
  });

  it('preserves a live event received before the first snapshot', () => {
    const live = reduceTandem(initialTandemState, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'delta', data: { content: 'In progress' } },
    });
    const restored = reduceTandem(live, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: new Set([session.id]),
    });
    expect(restored.snapshot?.sessions[0]?.status).toBe('running');
    expect(restored.conversations[session.id]?.stream).toBe('In progress');
  });

  it('keeps a global runtime failure visible across automatic snapshots until recovery', () => {
    const failed = reduceTandem(loaded, {
      type: 'eventReceived',
      event: { sessionId: '', kind: 'error', data: { message: 'Runtime disconnected' } },
    });
    const refreshed = reduceTandem(failed, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: new Set(),
    });
    expect(refreshed.runtimeError).toBe('Runtime disconnected');
    expect(refreshed.conversations['']).toBeUndefined();
    expect(reduceTandem(refreshed, { type: 'runtimeRecovered' }).runtimeError).toBeNull();
  });

  it('preserves a pending send while older history arrives, then replaces it on confirmation', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const older = { ...pending, id: 'older', createdAt: '2026-10-16T00:00:00Z' };
    const restored = reduceTandem(sending, {
      type: 'historyReceived',
      sessionId: session.id,
      messages: [older],
    });
    expect(restored.conversations[session.id]?.messages.map((message) => message.id)).toEqual([
      older.id,
      pending.id,
    ]);
    const confirmed = reduceTandem(restored, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'message', data: { ...pending, id: '1' } },
    });
    expect(confirmed.conversations[session.id]?.messages.map((message) => message.id)).toEqual([
      older.id,
      '1',
    ]);
  });

  it('isolates failures and permission responses to the target session', () => {
    const other = { ...session, id: 'session-2' };
    const withOther = reduceTandem(loaded, { type: 'sessionAdded', session: other });
    const otherSending = reduceTandem(withOther, {
      type: 'sendStarted',
      message: { ...pending, id: 'pending:2', sessionId: other.id },
    });
    const awaitingPermission = reduceTandem(otherSending, {
      type: 'eventReceived',
      event: {
        sessionId: other.id,
        kind: 'permission',
        data: { requestId: 'request-2', kind: 'write', description: 'Write file' },
      },
    });
    const failed = reduceTandem(awaitingPermission, {
      type: 'sendFailed',
      sessionId: session.id,
      pendingId: pending.id,
      error: 'Could not send',
    });
    expect(failed.conversations[other.id]).toBe(awaitingPermission.conversations[other.id]);
    expect(failed.snapshot?.sessions.find((item) => item.id === other.id)?.status).toBe('running');
    const answered = reduceTandem(failed, {
      type: 'permissionResponded',
      sessionId: other.id,
      requestId: 'request-2',
    });
    expect(answered.conversations[other.id]?.permissions).toEqual([]);
    expect(answered.conversations[session.id]).toBe(failed.conversations[session.id]);
  });

  it('retains an unconfirmed failed send across refresh so a remounted view can recover it', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const failed = reduceTandem(sending, {
      type: 'sendFailed',
      sessionId: session.id,
      pendingId: pending.id,
      error: 'Could not send',
    });
    const refreshed = reduceTandem(failed, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: new Set(),
    });
    expect(refreshed.conversations[session.id]?.messages).toEqual([]);
    expect(refreshed.conversations[session.id]?.failedSend).toEqual(pending);

    const retried = reduceTandem(refreshed, {
      type: 'sendStarted',
      message: { ...pending, id: 'pending:retry' },
    });
    expect(retried.conversations[session.id]?.failedSend).toBeNull();
  });

  it('retains attachment bytes after a failed send and releases them when history confirms it', () => {
    const attachment = { name: 'screen.png', mimeType: 'image/png', data: 'aGVsbG8=' };
    const metadata = {
      id: 'attachment-1',
      name: attachment.name,
      mimeType: attachment.mimeType,
      size: 5,
    };
    const withAttachment = {
      ...pending,
      content: '',
      attachments: [metadata],
      pendingAttachments: [attachment],
    };
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: withAttachment });
    const failed = reduceTandem(sending, {
      type: 'sendFailed',
      sessionId: session.id,
      pendingId: pending.id,
      error: 'Connection lost',
    });
    expect(failed.conversations[session.id]?.failedSend?.pendingAttachments).toEqual([attachment]);
    const restored = reduceTandem(failed, {
      type: 'historyReceived',
      sessionId: session.id,
      messages: [{ ...pending, id: '1', content: '', attachments: [metadata] }],
    });
    expect(restored.conversations[session.id]?.failedSend).toBeNull();
    expect(restored.conversations[session.id]?.messages[0]?.attachments).toEqual([metadata]);
    expect(restored.conversations[session.id]?.messages[0]?.pendingAttachments).toBeUndefined();
  });

  it.each(['before', 'after'] as const)(
    'does not offer a confirmed send for recovery when its acknowledgement arrives %s the failure',
    (order) => {
      const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
      const confirm = (state: typeof loaded) =>
        reduceTandem(state, {
          type: 'eventReceived',
          event: { sessionId: session.id, kind: 'message', data: { ...pending, id: '1' } },
        });
      const fail = (state: typeof loaded) =>
        reduceTandem(state, {
          type: 'sendFailed',
          sessionId: session.id,
          pendingId: pending.id,
          error: 'Connection lost',
        });
      const result = order === 'before' ? fail(confirm(sending)) : confirm(fail(sending));
      expect(result.conversations[session.id]?.failedSend).toBeNull();
      expect(result.conversations[session.id]?.messages.map((message) => message.id)).toEqual([
        '1',
      ]);
    },
  );

  it('recovers a lost live confirmation through history without duplicating an accepted send', () => {
    const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
    const messages = [
      { ...pending, id: 'older', createdAt: '2026-10-16T00:00:00Z' },
      { ...pending, id: '1' },
    ];
    const restored = reduceTandem(sending, {
      type: 'historyReceived',
      sessionId: session.id,
      messages,
    });
    expect(restored.conversations[session.id]?.messages).toEqual(messages);
    const repeated = reduceTandem(restored, {
      type: 'historyReceived',
      sessionId: session.id,
      messages,
    });
    expect(repeated.conversations[session.id]?.messages).toEqual(messages);
  });

  it('does not let a replayed acknowledgement consume a newer send with the same text', () => {
    const first = reduceTandem(loaded, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'message', data: { ...pending, id: '1' } },
    });
    const second = reduceTandem(first, {
      type: 'sendStarted',
      message: { ...pending, id: 'pending:2' },
    });
    const replayed = reduceTandem(second, {
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'message', data: { ...pending, id: '1' } },
    });
    expect(replayed.conversations[session.id]?.messages.map((message) => message.id)).toEqual([
      '1',
      'pending:2',
    ]);
  });

  it.each([
    { source: 'history', id: 'older', recovered: false },
    { source: 'history', id: '1', recovered: true },
    { source: 'live', id: 'older', recovered: false },
    { source: 'live', id: '1', recovered: true },
  ] as const)(
    'reconciles failed send with $source message $id by identity, not identical text',
    ({ source, id, recovered }) => {
      const sending = reduceTandem(loaded, { type: 'sendStarted', message: pending });
      const failed = reduceTandem(sending, {
        type: 'sendFailed',
        sessionId: session.id,
        pendingId: pending.id,
        error: 'Connection lost',
      });
      const confirmation = { ...pending, id };
      const result = reduceTandem(
        failed,
        source === 'history'
          ? { type: 'historyReceived', sessionId: session.id, messages: [confirmation] }
          : {
              type: 'eventReceived',
              event: { sessionId: session.id, kind: 'message', data: confirmation },
            },
      );
      expect(result.conversations[session.id]?.failedSend).toEqual(recovered ? null : pending);
    },
  );
});
