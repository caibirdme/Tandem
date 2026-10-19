import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';

import { dispatch, errorMessage, subscribe } from '../lib/api';
import { emptyConversation } from '../lib/conversation';
import { createModelChanges } from '../lib/modelChanges';
import { createPermissionModeChanges } from '../lib/permissionModeChanges';
import { protectedSessionIds } from '../lib/reconcile';
import { initialTandemState, reduceTandem } from '../lib/tandemState';
import type { Message, ModelSelection, PermissionMode, RuntimeEvent, Session } from '../lib/types';

export function useTandem() {
  const [
    {
      snapshot,
      conversations,
      error,
      runtimeError,
      permissionModeUpdates,
      permissionModeUncertain,
      modelUpdates,
      modelUncertain,
      titleGenerations,
      titleErrors,
    },
    update,
  ] = useReducer(reduceTandem, initialTandemState);
  const [connected, setConnected] = useState(false);
  const [loading, setLoading] = useState(true);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [historyRevision, setHistoryRevision] = useState(0);
  const historyRequest = useMemo(
    () => ({ sessionId: activeSessionId, connected, revision: historyRevision }),
    [activeSessionId, connected, historyRevision],
  );
  const [settledHistoryRequest, setSettledHistoryRequest] = useState<typeof historyRequest | null>(
    null,
  );
  const [confirmedHistoryRequest, setConfirmedHistoryRequest] = useState<
    typeof historyRequest | null
  >(null);
  const historyLoading = activeSessionId !== null && settledHistoryRequest !== historyRequest;
  const historyLoaded = activeSessionId === null || confirmedHistoryRequest === historyRequest;
  const reloadHistory = useCallback(() => {
    setHistoryRevision((current) => current + 1);
  }, []);

  const revisions = useRef<Record<string, number>>({});
  const pendingRequests = useRef(new Set<string>());
  const sessionProjects = useRef(new Map<string, string | null>());
  const deletedProjectIds = useRef(new Set<string>());
  const titleRequests = useRef(new Map<string, symbol>());
  const titleDispatches = useRef(new Map<string, symbol>());
  const titleRevisions = useRef<Record<string, number>>({});
  const snapshotSequence = useRef(0);
  const appliedSnapshotSequence = useRef(0);
  const advance = useCallback((id: string) => {
    revisions.current[id] = (revisions.current[id] ?? 0) + 1;
  }, []);
  const advanceTitle = useCallback((id: string) => {
    titleRevisions.current[id] = (titleRevisions.current[id] ?? 0) + 1;
  }, []);
  const permissionChanges = useRef<ReturnType<typeof createPermissionModeChanges> | null>(null);
  const modelChanges = useRef<ReturnType<typeof createModelChanges> | null>(null);
  useEffect(() => {
    permissionChanges.current ??= createPermissionModeChanges({
      getRequests: () => pendingRequests.current,
      advance,
      update,
      save: async (sessionId, permissionMode) =>
        (await dispatch('set_permission_mode', { sessionId, permissionMode })).permissionMode,
      read: async (sessionId) => {
        const fresh = await dispatch('snapshot', {});
        const mode = fresh.sessions.find((item) => item.id === sessionId)?.permissionMode;
        if (mode !== 'ask' && mode !== 'allow-all')
          throw new Error('服务未返回此工作的有效权限状态');
        return mode;
      },
    });
    function selectionFrom(session: Session | undefined, sessionId: string): ModelSelection {
      if (session?.id !== sessionId || !session.providerId || !session.modelId)
        throw new Error('服务未返回此对话的有效模型状态');
      return { providerId: session.providerId, modelId: session.modelId };
    }
    modelChanges.current ??= createModelChanges({
      getRequests: () => pendingRequests.current,
      advance,
      update,
      save: async (sessionId, selection) =>
        selectionFrom(await dispatch('set_session_model', { sessionId, ...selection }), sessionId),
      read: async (sessionId) => {
        const fresh = await dispatch('snapshot', {});
        return selectionFrom(
          fresh.sessions.find((item) => item.id === sessionId),
          sessionId,
        );
      },
    });
  }, [advance]);

  const loadSnapshot = useCallback(async (isCurrent: () => boolean) => {
    const requestSequence = ++snapshotSequence.current;
    const requested = { ...revisions.current };
    const requestedTitles = { ...titleRevisions.current };
    const pendingAtRequest = new Set(pendingRequests.current);
    const titlesAtRequest = new Set(titleDispatches.current.keys());
    const next = await dispatch('snapshot', {});
    if (!isCurrent() || requestSequence < appliedSnapshotSequence.current) return next;
    appliedSnapshotSequence.current = requestSequence;
    for (const session of next.sessions) sessionProjects.current.set(session.id, session.projectId);
    const protectedIds = protectedSessionIds(
      requested,
      revisions.current,
      pendingAtRequest,
      pendingRequests.current,
    );
    const titleProtectedIds = protectedSessionIds(
      requestedTitles,
      titleRevisions.current,
      titlesAtRequest,
      new Set(titleDispatches.current.keys()),
    );
    for (const id of titleRequests.current.keys()) {
      if (!titleProtectedIds.has(id) && !next.titleGenerations?.includes(id))
        titleRequests.current.delete(id);
    }
    update({ type: 'snapshotReceived', snapshot: next, protectedIds, titleProtectedIds });
    return next;
  }, []);
  const reload = useCallback(async () => {
    const next = await loadSnapshot(() => true);
    update({ type: 'runtimeRecovered' });
    return next;
  }, [loadSnapshot]);
  const reportError = useCallback((value: string | null) => {
    update({ type: 'errorReported', error: value });
  }, []);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void loadSnapshot(() => !disposed)
      .catch((reason: unknown) => {
        if (!disposed) update({ type: 'errorReported', error: errorMessage(reason) });
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    const handleEvent = (event: RuntimeEvent) => {
      if (disposed) return;
      if (event.kind === 'title' || event.kind === 'title_error') {
        advanceTitle(event.sessionId);
        titleRequests.current.delete(event.sessionId);
      } else if (event.sessionId) {
        advance(event.sessionId);
        if (event.kind === 'status' && event.data.title !== undefined)
          advanceTitle(event.sessionId);
      }
      update({ type: 'eventReceived', event });
      if (
        event.sessionId &&
        (event.kind === 'error' || (event.kind === 'status' && event.data.status !== 'running'))
      ) {
        void loadSnapshot(() => !disposed).catch(() => undefined);
      }
    };
    void subscribe(handleEvent, (value) => {
      if (disposed) return;
      setConnected(value);
      if (value)
        void loadSnapshot(() => !disposed).catch((reason: unknown) => {
          if (!disposed) update({ type: 'errorReported', error: errorMessage(reason) });
        });
    })
      .then((unlisten) => {
        if (disposed) unlisten();
        else cleanup = unlisten;
      })
      .catch((reason: unknown) => {
        if (!disposed) update({ type: 'errorReported', error: errorMessage(reason) });
      });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [advance, advanceTitle, loadSnapshot]);

  useEffect(() => {
    const { sessionId } = historyRequest;
    if (!sessionId) return;
    let disposed = false;
    void dispatch('messages', { sessionId })
      .then((messages) => {
        if (disposed) return;
        update({ type: 'historyReceived', sessionId, messages });
        setConfirmedHistoryRequest(historyRequest);
      })
      .catch((reason: unknown) => {
        if (!disposed) update({ type: 'errorReported', error: errorMessage(reason) });
      })
      .finally(() => {
        if (!disposed) setSettledHistoryRequest(historyRequest);
      });
    return () => {
      disposed = true;
    };
  }, [historyRequest]);

  const addSession = useCallback(
    (session: Session, activate = true) => {
      if (session.projectId && deletedProjectIds.current.has(session.projectId)) {
        permissionChanges.current?.forget(session.id);
        modelChanges.current?.forget(session.id);
        update({ type: 'sessionsDeleted', sessionIds: [session.id] });
        throw new Error('此工作所属的项目已移除。');
      }
      sessionProjects.current.set(session.id, session.projectId);
      advance(session.id);
      update({ type: 'sessionAdded', session });
      if (activate) setActiveSessionId(session.id);
    },
    [advance],
  );

  const send = useCallback(
    async (sessionId: string, content: string) => {
      const changes = permissionChanges.current;
      const models = modelChanges.current;
      if (!changes || !models) throw new Error('工作空间正在初始化，请稍后再试。');
      models.assertCanSend(sessionId);
      changes.assertCanSend(sessionId);
      advance(sessionId);
      pendingRequests.current.add(sessionId);
      const messageId = crypto.randomUUID();
      const pending: Message = {
        id: `pending:${messageId}`,
        sessionId,
        content,
        role: 'user',
        createdAt: new Date().toISOString(),
      };
      update({ type: 'sendStarted', message: pending });
      try {
        await dispatch('send_message', { sessionId, content, messageId });
      } catch (reason) {
        update({
          type: 'sendFailed',
          sessionId,
          pendingId: pending.id,
          error: errorMessage(reason),
        });
        throw reason;
      } finally {
        pendingRequests.current.delete(sessionId);
        advance(sessionId);
      }
    },
    [advance],
  );

  const changePermissionMode = useCallback(
    async (
      sessionId: string,
      permissionMode: PermissionMode,
    ): Promise<PermissionMode | undefined> => {
      const target = snapshot?.sessions.find((item) => item.id === sessionId);
      if (!target?.projectId) {
        update({ type: 'errorReported', error: '只有工作会话可以更改工具权限。' });
        return;
      }
      if (
        pendingRequests.current.has(sessionId) ||
        target.status === 'running' ||
        conversations[sessionId]?.status === 'running' ||
        Boolean(conversations[sessionId]?.permissions.length)
      ) {
        update({ type: 'errorReported', error: '请等待当前任务结束，再更改工具权限。' });
        return;
      }
      if (target.permissionMode === permissionMode) return target.permissionMode;
      modelChanges.current?.assertCanSend(sessionId);
      return permissionChanges.current?.change(sessionId, permissionMode);
    },
    [conversations, snapshot],
  );
  const confirmPermissionMode = useCallback(async (sessionId: string) => {
    await permissionChanges.current?.confirm(sessionId);
  }, []);
  const changeSessionModel = useCallback(
    async (sessionId: string, selection: ModelSelection) => {
      const target = snapshot?.sessions.find((item) => item.id === sessionId);
      if (!target) throw new Error('此对话已不存在，请刷新后重试。');
      if (!connected) throw new Error('请等待服务重新连接，再切换模型。');
      if (activeSessionId === sessionId && historyLoading)
        throw new Error('请等待对话历史载入完成，再切换模型。');
      if (
        target.status === 'running' ||
        conversations[sessionId]?.status === 'running' ||
        Boolean(conversations[sessionId]?.permissions.length)
      )
        throw new Error('请先停止当前回复或等待结束，再切换模型。');
      permissionChanges.current?.assertCanSend(sessionId);
      modelChanges.current?.assertCanSend(sessionId);
      if (target.providerId === selection.providerId && target.modelId === selection.modelId)
        return;
      await modelChanges.current?.change(sessionId, selection);
    },
    [activeSessionId, connected, conversations, historyLoading, snapshot],
  );
  const confirmSessionModel = useCallback(async (sessionId: string) => {
    await modelChanges.current?.confirm(sessionId);
  }, []);
  const deleteSession = useCallback(async (sessionId: string) => {
    await dispatch('delete_session', { sessionId });
    permissionChanges.current?.forget(sessionId);
    modelChanges.current?.forget(sessionId);
    sessionProjects.current.delete(sessionId);
    titleRequests.current.delete(sessionId);
    titleDispatches.current.delete(sessionId);
    update({ type: 'sessionsDeleted', sessionIds: [sessionId] });
  }, []);
  const deleteProject = useCallback(async (projectId: string) => {
    await dispatch('delete_project', { id: projectId });
    deletedProjectIds.current.add(projectId);
    const sessionIds = [...sessionProjects.current]
      .filter(([, owner]) => owner === projectId)
      .map(([sessionId]) => sessionId);
    for (const sessionId of sessionIds) {
      permissionChanges.current?.forget(sessionId);
      modelChanges.current?.forget(sessionId);
      sessionProjects.current.delete(sessionId);
      titleRequests.current.delete(sessionId);
      titleDispatches.current.delete(sessionId);
    }
    update({ type: 'sessionsDeleted', sessionIds, projectId });
  }, []);

  const regenerateTitle = useCallback(
    async (sessionId: string) => {
      if (titleRequests.current.has(sessionId) || titleGenerations.has(sessionId)) return;
      const token = Symbol();
      titleRequests.current.set(sessionId, token);
      titleDispatches.current.set(sessionId, token);
      advanceTitle(sessionId);
      update({ type: 'titleGenerationRequested', sessionId });
      try {
        await dispatch('regenerate_title', { sessionId });
      } catch (reason) {
        if (titleRequests.current.get(sessionId) !== token) return;
        titleRequests.current.delete(sessionId);
        advanceTitle(sessionId);
        update({ type: 'titleGenerationFailed', sessionId, error: errorMessage(reason) });
      } finally {
        if (titleDispatches.current.get(sessionId) === token) {
          titleDispatches.current.delete(sessionId);
          advanceTitle(sessionId);
        }
      }
      void loadSnapshot(() => true).catch(() => undefined);
    },
    [advanceTitle, loadSnapshot, titleGenerations],
  );
  const renameSession = useCallback(
    async (sessionId: string, title: string) => {
      advanceTitle(sessionId);
      await dispatch('rename_session', { sessionId, title });
      advanceTitle(sessionId);
      titleRequests.current.delete(sessionId);
      update({ type: 'sessionRenamed', sessionId, title });
    },
    [advanceTitle],
  );
  const dismissTitleError = useCallback((sessionId: string) => {
    update({ type: 'titleErrorDismissed', sessionId });
  }, []);

  const respondPermission = useCallback(
    async (sessionId: string, requestId: string, approved: boolean) => {
      await dispatch('respond_permission', { sessionId, requestId, approved });
      advance(sessionId);
      update({ type: 'permissionResponded', sessionId, requestId });
    },
    [advance],
  );

  return {
    snapshot,
    loading,
    error: runtimeError ?? error,
    setError: reportError,
    connected,
    reload,
    activeSessionId,
    setActiveSessionId,
    addSession,
    send,
    changePermissionMode,
    confirmPermissionMode,
    changeSessionModel,
    confirmSessionModel,
    deleteSession,
    deleteProject,
    regenerateTitle,
    renameSession,
    dismissTitleError,
    titleGenerations,
    titleError: activeSessionId ? (titleErrors[activeSessionId] ?? null) : null,
    permissionModeSaving: activeSessionId !== null && permissionModeUpdates.has(activeSessionId),
    permissionModeError: activeSessionId
      ? (permissionModeUncertain[activeSessionId] ?? null)
      : null,
    modelSaving: activeSessionId !== null && modelUpdates.has(activeSessionId),
    modelError: activeSessionId ? (modelUncertain[activeSessionId] ?? null) : null,
    respondPermission,
    historyLoading,
    historyLoaded,
    reloadHistory,
    conversation: activeSessionId
      ? (conversations[activeSessionId] ?? emptyConversation)
      : emptyConversation,
  };
}
