import { describe, expect, it, vi } from 'vitest';

import { createModelChanges } from './modelChanges';
import { createPermissionModeChanges } from './permissionModeChanges';
import { protectedSessionIds } from './reconcile';
import { initialTandemState, reduceTandem } from './tandemState';
import type { ModelSelection, Session, Snapshot } from './types';

const session: Session = {
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
  },
  runtime: { available: true, version: null, message: 'Ready' },
};
const selection: ModelSelection = { providerId: 'provider-2', modelId: 'model-2' };
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
  const save = vi.fn(async (_id: string, value: ModelSelection) => value);
  const read = vi.fn(async (_id: string): Promise<ModelSelection> => ({
    providerId: session.providerId,
    modelId: session.modelId,
  }));
  const changes = createModelChanges({ ...common, save, read });
  const permissions = createPermissionModeChanges({
    ...common,
    save: async (_id, mode) => mode,
    read: async () => 'ask',
  });
  return {
    requests,
    revisions,
    changes,
    permissions,
    save,
    read,
    update: common.update,
    get state() {
      return state;
    },
  };
}

describe('persisted conversation model changes', () => {
  it('keeps the confirmed model while saving, blocks concurrent settings and changes only its session', async () => {
    const test = setup();
    const saved = deferred<ModelSelection>();
    test.save.mockReturnValueOnce(saved.promise);
    const change = test.changes.change(session.id, selection);
    expect(test.state.modelUpdates.has(session.id)).toBe(true);
    expect(test.state.snapshot?.sessions[0]?.modelId).toBe(session.modelId);
    expect(() => test.changes.assertCanSend(session.id)).toThrow();
    await expect(test.changes.change(session.id, selection)).rejects.toThrow();
    await expect(test.permissions.change(session.id, 'allow-all')).rejects.toThrow();
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();

    test.update({ type: 'sessionRenamed', sessionId: session.id, title: 'New independent title' });
    saved.resolve(selection);
    await change;
    expect(test.state.snapshot?.sessions[0]).toEqual({
      ...session,
      ...selection,
      title: 'New independent title',
    });
    expect(test.state.snapshot?.sessions[1]).toEqual({ ...session, id: 'session-2' });
    expect(test.state.conversations[session.id]?.messages[0]?.content).toBe('Keep this history');
    expect(test.state.modelUpdates.size).toBe(0);
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it.each(['before change', 'during change'])(
    'rejects a stale snapshot requested $0',
    async (timing) => {
      const test = setup();
      const saved = deferred<ModelSelection>();
      test.save.mockReturnValueOnce(saved.promise);
      let requested = { ...test.revisions };
      let pendingAtRequest = new Set(test.requests);
      const change = test.changes.change(session.id, selection);
      if (timing === 'during change') {
        requested = { ...test.revisions };
        pendingAtRequest = new Set(test.requests);
      }
      saved.resolve(selection);
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
      expect(test.state.snapshot?.sessions[0]?.modelId).toBe(selection.modelId);
    },
  );

  it('confirms the actual model after a failed response and releases the lock', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockResolvedValueOnce(selection);
    await test.changes.change(session.id, selection);
    expect(test.state.snapshot?.sessions[0]?.modelId).toBe(selection.modelId);
    expect(test.state.error).toContain('The current model has been confirmed');
    expect(test.requests.has(session.id)).toBe(false);
  });

  it('retains the previous model after rejection when the server confirms it was unchanged', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Invalid model'));
    await test.changes.change(session.id, selection);
    expect(test.state.snapshot?.sessions[0]?.modelId).toBe(session.modelId);
    expect(test.state.modelUncertain[session.id]).toBeUndefined();
    expect(test.state.error).toContain('Invalid model');
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it('blocks sending and permission changes while the model is uncertain, then recovers after retry', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockRejectedValueOnce(new Error('Service offline'));
    await test.changes.change(session.id, selection);
    expect(test.state.modelUpdates.size).toBe(0);
    expect(test.state.modelUncertain[session.id]).toContain('Service offline');
    expect(() => test.changes.assertCanSend(session.id)).toThrow(
      'The current model is unconfirmed',
    );
    await expect(test.permissions.change(session.id, 'allow-all')).rejects.toThrow();
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();
    test.read.mockResolvedValueOnce(selection);
    await test.changes.confirm(session.id);
    expect(test.state.modelUncertain[session.id]).toBeUndefined();
    expect(test.state.snapshot?.sessions[0]?.modelId).toBe(selection.modelId);
    expect(test.requests.size).toBe(0);
  });

  it.each(['save', 'confirmation'])(
    'ignores deleted session results from a late $0',
    async (stage) => {
      const test = setup();
      const pending = deferred<ModelSelection>();
      if (stage === 'save') test.save.mockReturnValueOnce(pending.promise);
      else {
        test.save.mockRejectedValueOnce(new Error('Response lost'));
        test.read.mockReturnValueOnce(pending.promise);
      }
      const change = test.changes.change(session.id, selection);
      await Promise.resolve();
      test.changes.forget(session.id);
      test.update({ type: 'sessionsDeleted', sessionIds: [session.id] });
      const deleted = test.state;
      pending.resolve(selection);
      await change;
      expect(test.state).toBe(deleted);
      expect(test.state.snapshot?.sessions.map((item) => item.id)).toEqual(['session-2']);
      expect(test.requests.size).toBe(0);
      expect(() => test.changes.assertCanSend(session.id)).toThrow(
        'This conversation has been deleted',
      );
    },
  );

  it('ends the confirmation spinner on timeout while keeping the model locked', async () => {
    vi.useFakeTimers();
    try {
      const test = setup();
      test.save.mockRejectedValueOnce(new Error('Response lost'));
      test.read.mockReturnValueOnce(new Promise<ModelSelection>(() => undefined));
      const change = test.changes.change(session.id, selection);
      await vi.advanceTimersByTimeAsync(15_000);
      await change;
      expect(test.state.modelUpdates.size).toBe(0);
      expect(test.state.modelUncertain[session.id]).toContain('Model confirmation timed out');
      expect(() => test.changes.assertCanSend(session.id)).toThrow(
        'The current model is unconfirmed',
      );
    } finally {
      vi.useRealTimers();
    }
  });
});
