import { describe, expect, it, vi } from 'vitest';

import { createPermissionModeChanges } from './permissionModeChanges';
import { protectedSessionIds } from './reconcile';
import { initialTandemState, reduceTandem } from './tandemState';
import type { PermissionMode, Session, Snapshot } from './types';

const session: Session = {
  contextUsage: null,
  titleState: 'preserved',
  id: 'session-1',
  title: 'Work',
  projectId: 'project-1',
  providerId: 'provider-1',
  modelId: 'model-1',
  createdAt: '2026-10-17T00:00:00Z',
  updatedAt: '2026-10-17T00:00:00Z',
  status: 'idle',
  permissionMode: 'ask',
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
  const requests = new Set<string>();
  const revisions: Record<string, number> = {};
  const save = vi.fn(async (_id: string, mode: PermissionMode) => mode);
  const read = vi.fn(async (_id: string): Promise<PermissionMode> => 'ask');
  const changes = createPermissionModeChanges({
    getRequests: () => requests,
    save,
    read,
    advance: (id) => {
      revisions[id] = (revisions[id] ?? 0) + 1;
    },
    update: (event) => {
      state = reduceTandem(state, event);
    },
  });
  return {
    requests,
    revisions,
    save,
    read,
    changes,
    deleteSession(id: string) {
      changes.forget(id);
      state = reduceTandem(state, { type: 'sessionsDeleted', sessionIds: [id] });
    },
    get state() {
      return state;
    },
  };
}

describe('permission changes with uncertain transport outcomes', () => {
  it.each([
    { source: 'save', actual: 'ask' },
    { source: 'save', actual: 'allow-all' },
    { source: 'read', actual: 'ask' },
    { source: 'read', actual: 'allow-all' },
  ] as const)(
    'returns the actual $actual mode confirmed by $source',
    async ({ source, actual }) => {
      const test = setup();
      if (source === 'save') test.save.mockResolvedValueOnce(actual);
      else {
        test.save.mockRejectedValueOnce(new Error('Response lost'));
        test.read.mockResolvedValueOnce(actual);
      }
      await expect(test.changes.change(session.id, 'allow-all')).resolves.toBe(actual);
      expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe(actual);
      expect(test.requests.has(session.id)).toBe(false);
    },
  );

  it('returns no confirmed mode when there is no uncertain setting to retry', async () => {
    const test = setup();
    await expect(test.changes.confirm(session.id)).resolves.toBeUndefined();
    expect(test.read).not.toHaveBeenCalled();
  });

  it('returns no mode when a confirmation retry also fails', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockRejectedValue(new Error('Confirmation unavailable'));
    await expect(test.changes.change(session.id, 'allow-all')).resolves.toBeUndefined();
    await expect(test.changes.confirm(session.id)).resolves.toBeUndefined();
    expect(() => test.changes.assertCanSend(session.id)).toThrow(
      'Tool permissions are unconfirmed',
    );
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('ask');
  });

  it.each(['resolve', 'reject'] as const)(
    'returns no mode when a deleted session’s confirmation retry later %s',
    async (outcome) => {
      const test = setup();
      test.save.mockRejectedValueOnce(new Error('Response lost'));
      test.read.mockRejectedValueOnce(new Error('Confirmation unavailable'));
      await test.changes.change(session.id, 'allow-all');
      const pending = deferred<PermissionMode>();
      test.read.mockReturnValueOnce(pending.promise);
      const retry = test.changes.confirm(session.id);
      test.deleteSession(session.id);
      const deleted = test.state;
      if (outcome === 'resolve') pending.resolve('allow-all');
      else pending.reject(new Error('Late failure'));
      await expect(retry).resolves.toBeUndefined();
      expect(test.state).toBe(deleted);
      expect(test.requests.has(session.id)).toBe(false);
    },
  );

  it('releases an uncertain session when deleted without preserving it through refresh', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Response lost'));
    test.read.mockRejectedValueOnce(new Error('Confirmation unavailable'));
    await test.changes.change(session.id, 'allow-all');
    expect(test.requests.has(session.id)).toBe(true);
    test.deleteSession(session.id);
    expect(test.requests.has(session.id)).toBe(false);
    expect(test.state.permissionModeUncertain[session.id]).toBeUndefined();
    expect(() => test.changes.assertCanSend(session.id)).toThrow(
      'This conversation has been deleted',
    );
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();
    await test.changes.confirm(session.id);
    expect(test.read).toHaveBeenCalledTimes(1);
    const refreshed = reduceTandem(test.state, {
      type: 'snapshotReceived',
      snapshot: {
        ...snapshot,
        sessions: snapshot.sessions.filter((item) => item.id !== session.id),
      },
      protectedIds: new Set(test.requests),
    });
    expect(refreshed.snapshot?.sessions.map((item) => item.id)).toEqual(['session-2']);
  });

  it.each([
    { stage: 'save', outcome: 'resolve' },
    { stage: 'save', outcome: 'reject' },
    { stage: 'confirmation', outcome: 'resolve' },
    { stage: 'confirmation', outcome: 'reject' },
  ] as const)('ignores a deleted session’s late $stage $outcome', async ({ stage, outcome }) => {
    const test = setup();
    const pendingResult = deferred<PermissionMode>();
    if (stage === 'save') test.save.mockReturnValueOnce(pendingResult.promise);
    else {
      test.save.mockRejectedValueOnce(new Error('Response lost'));
      test.read.mockReturnValueOnce(pendingResult.promise);
    }
    const change = test.changes.change(session.id, 'allow-all');
    await Promise.resolve();
    test.deleteSession(session.id);
    const deleted = test.state;
    const revision = test.revisions[session.id];
    if (outcome === 'resolve') pendingResult.resolve('allow-all');
    else pendingResult.reject(new Error('Late failure'));
    await expect(change).resolves.toBeUndefined();
    expect(test.state).toBe(deleted);
    expect(test.revisions[session.id]).toBe(revision);
    expect(test.requests.has(session.id)).toBe(false);
    expect(test.state.permissionModeUpdates.size).toBe(0);
    expect(test.state.permissionModeUncertain[session.id]).toBeUndefined();
    expect(test.read).toHaveBeenCalledTimes(stage === 'save' ? 0 : 1);
  });

  it('holds the send lock after a lost save response until a direct read confirms the saved mode', async () => {
    const test = setup();
    const saved = deferred<PermissionMode>();
    const confirmed = deferred<PermissionMode>();
    test.save.mockReturnValue(saved.promise);
    test.read.mockReturnValue(confirmed.promise);
    const requestedRevision = { ...test.revisions };
    const change = test.changes.change(session.id, 'allow-all');
    saved.reject(new Error('Response lost after saving'));
    await Promise.resolve();
    expect(test.read).toHaveBeenCalledWith(session.id);
    expect(test.requests.has(session.id)).toBe(true);
    expect(() => test.changes.assertCanSend(session.id)).toThrow();
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('ask');
    confirmed.resolve('allow-all');
    await expect(change).resolves.toBe('allow-all');
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('allow-all');
    expect(test.state.error).toContain('Current permissions have been confirmed as “Full access”');
    expect(test.state.permissionModeUpdates.size).toBe(0);
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
    const refreshed = reduceTandem(test.state, {
      type: 'snapshotReceived',
      snapshot,
      protectedIds: protectedSessionIds(
        requestedRevision,
        test.revisions,
        new Set(),
        test.requests,
      ),
    });
    expect(refreshed.snapshot?.sessions[0]?.permissionMode).toBe('allow-all');
  });

  it('keeps only the uncertain session blocked, then applies the actual mode and unlocks after retry', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Connection lost'));
    test.read.mockRejectedValueOnce(new Error('Confirmation unavailable'));
    await expect(test.changes.change(session.id, 'allow-all')).resolves.toBeUndefined();
    expect(test.state.permissionModeUpdates.size).toBe(0);
    expect(test.state.permissionModeUncertain[session.id]).toContain('Confirmation unavailable');
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('ask');
    expect(() => test.changes.assertCanSend(session.id)).toThrow(
      'Tool permissions are unconfirmed',
    );
    expect(() => test.changes.assertCanSend('session-2')).not.toThrow();
    await expect(test.changes.change(session.id, 'ask')).rejects.toThrow(
      'Tool permissions are unconfirmed',
    );
    await test.changes.change('session-2', 'allow-all');
    expect(test.state.snapshot?.sessions[1]?.permissionMode).toBe('allow-all');
    expect(test.requests.has(session.id)).toBe(true);

    const confirmed = deferred<PermissionMode>();
    test.read.mockReturnValueOnce(confirmed.promise);
    const retry = test.changes.confirm(session.id);
    expect(test.state.permissionModeUpdates.has(session.id)).toBe(true);
    expect(() => test.changes.assertCanSend(session.id)).toThrow();
    await test.changes.confirm(session.id);
    expect(test.read).toHaveBeenCalledTimes(2);
    confirmed.resolve('allow-all');
    await expect(retry).resolves.toBe('allow-all');
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('allow-all');
    expect(test.state.permissionModeUncertain[session.id]).toBeUndefined();
    expect(test.state.permissionModeUpdates.size).toBe(0);
    expect(test.requests.has(session.id)).toBe(false);
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it('retains the server mode when a rejected change is confirmed as not applied', async () => {
    const test = setup();
    test.save.mockRejectedValueOnce(new Error('Change rejected'));
    await expect(test.changes.change(session.id, 'allow-all')).resolves.toBe('ask');
    expect(test.state.snapshot?.sessions[0]?.permissionMode).toBe('ask');
    expect(test.state.permissionModeUncertain[session.id]).toBeUndefined();
    expect(() => test.changes.assertCanSend(session.id)).not.toThrow();
  });

  it('ends the confirmation spinner on timeout while retaining the lock', async () => {
    vi.useFakeTimers();
    try {
      const test = setup();
      test.save.mockRejectedValueOnce(new Error('Response lost'));
      test.read.mockReturnValueOnce(new Promise<PermissionMode>(() => undefined));
      const change = test.changes.change(session.id, 'allow-all');
      await vi.advanceTimersByTimeAsync(15_000);
      await expect(change).resolves.toBeUndefined();
      expect(test.state.permissionModeUpdates.size).toBe(0);
      expect(test.state.permissionModeUncertain[session.id]).toContain(
        'Permission confirmation timed out',
      );
      expect(() => test.changes.assertCanSend(session.id)).toThrow(
        'Tool permissions are unconfirmed',
      );
    } finally {
      vi.useRealTimers();
    }
  });
});
