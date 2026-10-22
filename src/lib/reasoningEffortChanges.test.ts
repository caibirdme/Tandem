import { describe, expect, it, vi } from 'vitest';

import { createReasoningEffortChanges } from './reasoningEffortChanges';
import { protectedSessionIds } from './reconcile';
import { initialTandemState, reduceTandem } from './tandemState';
import type { ReasoningEffort, Session, Snapshot } from './types';

const session: Session = {
  reasoningEffort: null,
  contextUsage: null,
  titleState: 'preserved',
  id: 'session-1',
  title: 'Existing conversation',
  projectId: 'project-1',
  providerId: 'provider-1',
  modelId: 'model-1',
  permissionMode: 'ask',
  status: 'idle',
  createdAt: '2026-10-19T00:00:00Z',
  updatedAt: '2026-10-19T00:00:00Z',
};
const snapshot: Snapshot = {
  titleGenerations: [],
  activeTurns: {},
  sessions: [session, { ...session, id: 'session-2' }],
  projects: [],
  settings: {
    providers: [],
    titleModel: null,
    telemetry: { enabled: false, endpoint: '', protocol: 'http/json', captureContent: false },
    cliPath: null,
    fontSize: 'medium',
  },
  runtime: { available: true, version: null, message: 'Ready' },
};
const effort: ReasoningEffort = 'max';
const noop = () => undefined;

function deferred<T>() {
  let resolve: (value: T) => void = noop;
  let reject: (reason: unknown) => void = noop;
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function setup() {
  let state = reduceTandem(initialTandemState, {
    type: 'snapshotReceived',
    snapshot,
    protectedIds: new Set(),
  });
  state = reduceTandem(state, {
    type: 'historyReceived',
    sessionId: session.id,
    messages: [
      {
        id: 'message-1',
        sessionId: session.id,
        role: 'user',
        content: 'Keep this history',
        attachments: [],
        createdAt: session.createdAt,
      },
    ],
  });
  const requests = new Set<string>();
  const revisions: Record<string, number> = {};
  const common = {
    getRequests: () => requests,
    advance: (id: string) => {
      revisions[id] = (revisions[id] ?? 0) + 1;
    },
    update: (event: Parameters<typeof reduceTandem>[1]) => {
      state = reduceTandem(state, event);
    },
  };
  const save = vi.fn(async (_id: string, value: ReasoningEffort) => value);
  const read = vi.fn(async (_id: string): Promise<ReasoningEffort> => session.reasoningEffort);
  const changes = createReasoningEffortChanges({ ...common, save, read });
  return {
    requests,
    revisions,
    changes,
    save,
    read,
    update: common.update,
    get state() {
      return state;
    },
  };
}

describe('persisted conversation reasoning effort changes', () => {
  it('changes effort during a response while retaining streamed output, status, and context usage', async () => {
    const test = setup();
    test.update({
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'status', data: { status: 'running' } },
    });
    const saved = deferred<ReasoningEffort>();
    test.save.mockReturnValueOnce(saved.promise);
    const change = test.changes.change(session.id, 'low');
    test.update({
      type: 'eventReceived',
      event: { sessionId: session.id, kind: 'delta', data: { content: 'Still responding' } },
    });
    test.update({
      type: 'eventReceived',
      event: {
        sessionId: session.id,
        kind: 'context_usage',
        data: { currentTokens: 120, tokenLimit: 1000 },
      },
    });
    saved.resolve('low');
    await change;
    expect(test.state.snapshot?.sessions[0]).toMatchObject({
      reasoningEffort: 'low',
      status: 'running',
      contextUsage: { currentTokens: 120, tokenLimit: 1000 },
    });
    expect(test.state.conversations[session.id]?.stream).toBe('Still responding');
    expect(test.state.conversations[session.id]?.status).toBe('running');
    await test.changes.change(session.id, null);
    expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBeNull();
    expect(test.state.conversations[session.id]?.stream).toBe('Still responding');
  });

  it('keeps the confirmed effort while saving, blocks the next send and changes only its session', async () => {
    const test = setup();
    const saved = deferred<ReasoningEffort>();
    test.save.mockReturnValueOnce(saved.promise);
    const change = test.changes.change(session.id, effort);
    expect(test.state.reasoningEffortUpdates.has(session.id)).toBe(true);
    expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBe(session.reasoningEffort);
    expect(() => test.changes.assertCanSend(session.id)).toThrow();
    await expect(test.changes.change(session.id, effort)).rejects.toThrow();
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();

    test.update({ type: 'sessionRenamed', sessionId: session.id, title: 'New independent title' });
    saved.resolve(effort);
    await change;
    expect(test.state.snapshot?.sessions[0]).toEqual({
      ...session,
      reasoningEffort: effort,
      title: 'New independent title',
    });
    expect(test.state.snapshot?.sessions[1]).toEqual({ ...session, id: 'session-2' });
    expect(test.state.conversations[session.id]?.messages[0]?.content).toBe('Keep this history');
    expect(test.state.reasoningEffortUpdates.size).toBe(0);
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it.each(['before change', 'during change'])(
    'rejects a stale snapshot requested $0',
    async (timing) => {
      const test = setup();
      const saved = deferred<ReasoningEffort>();
      test.save.mockReturnValueOnce(saved.promise);
      let requested = { ...test.revisions };
      let pendingAtRequest = new Set(test.requests);
      const change = test.changes.change(session.id, effort);
      if (timing === 'during change') {
        requested = { ...test.revisions };
        pendingAtRequest = new Set(test.requests);
      }
      saved.resolve(effort);
      await change;
      test.update({
        type: 'snapshotReceived',
        snapshot,
        protectedIds: protectedSessionIds(
          requested,
          test.revisions,
          pendingAtRequest,
          test.requests,
        ),
      });
      expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBe(effort);
    },
  );

  it('confirms the actual effort after a failed response and releases the next-send lock', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockResolvedValueOnce(effort);
    await test.changes.change(session.id, effort);
    expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBe(effort);
    expect(test.state.error).toContain('The current reasoning effort has been confirmed');
    expect(test.requests.has(session.id)).toBe(false);
  });

  it('retains the previous effort after rejection when the server confirms it was unchanged', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Invalid reasoning effort'));
    await test.changes.change(session.id, effort);
    expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBe(session.reasoningEffort);
    expect(test.state.reasoningEffortUncertain[session.id]).toBeUndefined();
    expect(test.state.error).toContain('Invalid reasoning effort');
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it('blocks the next send while effort is uncertain, then recovers after retry', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockRejectedValueOnce(new Error('Service offline'));
    await test.changes.change(session.id, effort);
    expect(test.state.reasoningEffortUpdates.size).toBe(0);
    expect(test.state.reasoningEffortUncertain[session.id]).toContain('Service offline');
    expect(() => test.changes.assertCanSend(session.id)).toThrow(
      'The current reasoning effort is unconfirmed',
    );
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();
    test.read.mockResolvedValueOnce(effort);
    await test.changes.confirm(session.id);
    expect(test.state.reasoningEffortUncertain[session.id]).toBeUndefined();
    expect(test.state.snapshot?.sessions[0]?.reasoningEffort).toBe(effort);
    expect(test.requests.size).toBe(0);
  });

  it.each(['save', 'confirmation'])(
    'ignores deleted session results from a late $0',
    async (stage) => {
      const test = setup();
      const pending = deferred<ReasoningEffort>();
      if (stage === 'save') test.save.mockReturnValueOnce(pending.promise);
      else {
        test.save.mockRejectedValueOnce(new Error('Response lost'));
        test.read.mockReturnValueOnce(pending.promise);
      }
      const change = test.changes.change(session.id, effort);
      await Promise.resolve();
      test.changes.forget(session.id);
      test.update({ type: 'sessionsDeleted', sessionIds: [session.id] });
      const deleted = test.state;
      pending.resolve(effort);
      await change;
      expect(test.state).toBe(deleted);
      expect(test.state.snapshot?.sessions.map((item) => item.id)).toEqual(['session-2']);
      expect(test.requests.size).toBe(0);
      expect(() => test.changes.assertCanSend(session.id)).toThrow(
        'This conversation has been deleted',
      );
    },
  );

  it('ends the confirmation spinner on timeout while keeping the next send locked', async () => {
    vi.useFakeTimers();
    try {
      const test = setup();
      test.save.mockRejectedValueOnce(new Error('Response lost'));
      test.read.mockReturnValueOnce(new Promise<ReasoningEffort>(() => undefined));
      const change = test.changes.change(session.id, effort);
      await vi.advanceTimersByTimeAsync(15_000);
      await change;
      expect(test.state.reasoningEffortUpdates.size).toBe(0);
      expect(test.state.reasoningEffortUncertain[session.id]).toContain(
        'Reasoning effort confirmation timed out',
      );
      expect(() => test.changes.assertCanSend(session.id)).toThrow(
        'The current reasoning effort is unconfirmed',
      );
    } finally {
      vi.useRealTimers();
    }
  });
});
