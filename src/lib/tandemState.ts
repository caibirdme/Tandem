import { emptyConversation, receiveMessages, reduceEvent } from './conversation';
import type { Conversation } from './conversation';
import type { ModelChangeEvent } from './modelChanges';
import type { PermissionModeChangeEvent } from './permissionModeChanges';
import { reconcileConversations, reconcileSessions } from './reconcile';
import type { Message, RuntimeEvent, Session, Snapshot } from './types';

export interface TandemState {
  snapshot: Snapshot | null;
  conversations: Record<string, Conversation>;
  error: string | null;
  runtimeError: string | null;
  permissionModeUpdates: ReadonlySet<string>;
  permissionModeUncertain: Readonly<Record<string, string>>;
  modelUpdates: ReadonlySet<string>;
  modelUncertain: Readonly<Record<string, string>>;
  deletedSessionIds: ReadonlySet<string>;
  deletedProjectIds: ReadonlySet<string>;
  titleGenerations: ReadonlySet<string>;
  titleErrors: Readonly<Record<string, string>>;
}

export const initialTandemState: TandemState = {
  snapshot: null,
  conversations: {},
  error: null,
  runtimeError: null,
  permissionModeUpdates: new Set(),
  permissionModeUncertain: {},
  modelUpdates: new Set(),
  modelUncertain: {},
  deletedSessionIds: new Set(),
  deletedProjectIds: new Set(),
  titleGenerations: new Set(),
  titleErrors: {},
};

type TandemAction =
  | {
      type: 'snapshotReceived';
      snapshot: Snapshot;
      protectedIds: ReadonlySet<string>;
      titleProtectedIds?: ReadonlySet<string>;
    }
  | { type: 'eventReceived'; event: RuntimeEvent }
  | { type: 'historyReceived'; sessionId: string; messages: Message[] }
  | { type: 'sessionAdded'; session: Session }
  | { type: 'sessionsDeleted'; sessionIds: string[]; projectId?: string }
  | { type: 'titleGenerationRequested'; sessionId: string }
  | { type: 'titleGenerationFailed'; sessionId: string; error: string }
  | { type: 'titleErrorDismissed'; sessionId: string }
  | { type: 'sessionRenamed'; sessionId: string; title: string }
  | { type: 'sendStarted'; message: Message }
  | { type: 'sendFailed'; sessionId: string; pendingId: string; error: string }
  | { type: 'permissionResponded'; sessionId: string; requestId: string }
  | PermissionModeChangeEvent
  | ModelChangeEvent
  | { type: 'errorReported'; error: string | null }
  | { type: 'runtimeRecovered' };

function withoutDeletedItems(snapshot: Snapshot, state: TandemState): Snapshot {
  const sessions = snapshot.sessions.filter(
    (session) =>
      !state.deletedSessionIds.has(session.id) &&
      !(session.projectId && state.deletedProjectIds.has(session.projectId)),
  );
  const retainedIds = new Set(sessions.map((session) => session.id));
  return {
    ...snapshot,
    projects: snapshot.projects.filter((project) => !state.deletedProjectIds.has(project.id)),
    sessions,
    titleGenerations: (snapshot.titleGenerations ?? []).filter((id) => retainedIds.has(id)),
    ...(snapshot.activeTurns
      ? {
          activeTurns: Object.fromEntries(
            Object.entries(snapshot.activeTurns).filter(([sessionId]) =>
              retainedIds.has(sessionId),
            ),
          ),
        }
      : {}),
  };
}

function finishTitle(
  state: TandemState,
  sessionId: string,
  result: { title: string } | { error: string },
): TandemState {
  if (state.deletedSessionIds.has(sessionId)) return state;
  const titleGenerations = new Set(state.titleGenerations);
  titleGenerations.delete(sessionId);
  const titleErrors = { ...state.titleErrors };
  if ('error' in result) titleErrors[sessionId] = result.error;
  else delete titleErrors[sessionId];
  return {
    ...state,
    titleGenerations,
    titleErrors,
    snapshot:
      'title' in result && state.snapshot
        ? {
            ...state.snapshot,
            sessions: state.snapshot.sessions.map((session) =>
              session.id === sessionId ? { ...session, title: result.title } : session,
            ),
          }
        : state.snapshot,
  };
}

/** Conversation status and its sidebar session are updated together at this boundary. */
function updateConversation(
  state: TandemState,
  sessionId: string,
  update: (conversation: Conversation) => Conversation,
): TandemState {
  if (state.deletedSessionIds.has(sessionId)) return state;
  const previous = state.conversations[sessionId];
  const conversation = update(previous ?? emptyConversation);
  return {
    ...state,
    conversations: { ...state.conversations, [sessionId]: conversation },
    snapshot:
      state.snapshot && previous?.status !== conversation.status
        ? {
            ...state.snapshot,
            sessions: state.snapshot.sessions.map((session) =>
              session.id === sessionId ? { ...session, status: conversation.status } : session,
            ),
          }
        : state.snapshot,
  };
}

export function reduceTandem(state: TandemState, action: TandemAction): TandemState {
  switch (action.type) {
    case 'snapshotReceived': {
      const titleProtectedIds = action.titleProtectedIds ?? new Set<string>();
      const snapshot = withoutDeletedItems(action.snapshot, state);
      const conversations = reconcileConversations(
        state.conversations,
        snapshot,
        action.protectedIds,
      );
      const titleGenerations = new Set(snapshot.titleGenerations ?? []);
      for (const sessionId of titleProtectedIds) {
        if (state.titleGenerations.has(sessionId)) titleGenerations.add(sessionId);
        else titleGenerations.delete(sessionId);
      }
      for (const sessionId of state.deletedSessionIds) titleGenerations.delete(sessionId);
      return {
        ...state,
        titleGenerations,
        snapshot: {
          ...snapshot,
          sessions: reconcileSessions(
            state.snapshot?.sessions ?? [],
            snapshot.sessions,
            action.protectedIds,
            titleProtectedIds,
          ).map((session) => ({
            ...session,
            status: conversations[session.id]?.status ?? session.status,
          })),
        },
        conversations,
        error: null,
      };
    }
    case 'eventReceived': {
      const { event } = action;
      if (state.deletedSessionIds.has(event.sessionId)) return state;
      if (!event.sessionId) {
        return event.kind === 'error' ? { ...state, runtimeError: event.data.message } : state;
      }
      if (event.kind === 'title')
        return finishTitle(state, event.sessionId, { title: event.data.title });
      if (event.kind === 'title_error')
        return finishTitle(state, event.sessionId, { error: event.data.message });
      const next = updateConversation(state, event.sessionId, (conversation) =>
        reduceEvent(conversation, event),
      );
      if (event.kind !== 'status' || !next.snapshot) return next;
      const { title } = event.data;
      if (title === undefined) return next;
      return {
        ...next,
        snapshot: {
          ...next.snapshot,
          sessions: next.snapshot.sessions.map((session) =>
            session.id === event.sessionId ? { ...session, title } : session,
          ),
        },
      };
    }
    case 'historyReceived':
      return updateConversation(state, action.sessionId, (conversation) =>
        receiveMessages(conversation, action.messages),
      );
    case 'sessionAdded':
      if (state.deletedSessionIds.has(action.session.id)) return state;
      if (action.session.projectId && state.deletedProjectIds.has(action.session.projectId))
        return {
          ...state,
          deletedSessionIds: new Set([...state.deletedSessionIds, action.session.id]),
        };
      return {
        ...state,
        snapshot: state.snapshot && {
          ...state.snapshot,
          sessions: [
            action.session,
            ...state.snapshot.sessions.filter((session) => session.id !== action.session.id),
          ],
        },
        conversations: {
          ...state.conversations,
          [action.session.id]: {
            ...(state.conversations[action.session.id] ?? emptyConversation),
            status: action.session.status,
          },
        },
      };
    case 'sessionsDeleted': {
      const sessionIds = new Set([
        ...action.sessionIds,
        ...(action.projectId
          ? (state.snapshot?.sessions
              .filter((session) => session.projectId === action.projectId)
              .map((session) => session.id) ?? [])
          : []),
      ]);
      const conversations = { ...state.conversations };
      const permissionModeUpdates = new Set(state.permissionModeUpdates);
      const permissionModeUncertain = { ...state.permissionModeUncertain };
      const modelUpdates = new Set(state.modelUpdates);
      const modelUncertain = { ...state.modelUncertain };
      const titleGenerations = new Set(state.titleGenerations);
      const titleErrors = { ...state.titleErrors };
      for (const sessionId of sessionIds) {
        delete conversations[sessionId];
        permissionModeUpdates.delete(sessionId);
        delete permissionModeUncertain[sessionId];
        modelUpdates.delete(sessionId);
        delete modelUncertain[sessionId];
        titleGenerations.delete(sessionId);
        delete titleErrors[sessionId];
      }
      const next = {
        ...state,
        conversations,
        permissionModeUpdates,
        permissionModeUncertain,
        modelUpdates,
        modelUncertain,
        titleGenerations,
        titleErrors,
        deletedSessionIds: new Set([...state.deletedSessionIds, ...sessionIds]),
        deletedProjectIds: new Set([
          ...state.deletedProjectIds,
          ...(action.projectId ? [action.projectId] : []),
        ]),
      };
      return {
        ...next,
        snapshot: state.snapshot && withoutDeletedItems(state.snapshot, next),
      };
    }
    case 'titleGenerationRequested': {
      if (state.deletedSessionIds.has(action.sessionId)) return state;
      const titleErrors = { ...state.titleErrors };
      delete titleErrors[action.sessionId];
      return {
        ...state,
        titleGenerations: new Set([...state.titleGenerations, action.sessionId]),
        titleErrors,
      };
    }
    case 'titleGenerationFailed':
      return finishTitle(state, action.sessionId, { error: action.error });
    case 'sessionRenamed':
      return finishTitle(state, action.sessionId, { title: action.title });
    case 'titleErrorDismissed': {
      const titleErrors = { ...state.titleErrors };
      delete titleErrors[action.sessionId];
      return { ...state, titleErrors };
    }
    case 'sendStarted':
      return updateConversation(state, action.message.sessionId, (conversation) => ({
        ...conversation,
        messages: [...conversation.messages, action.message],
        status: 'running',
        stream: '',
        tools: [],
        permissions: [],
        error: null,
        failedSend: null,
      }));
    case 'sendFailed':
      return updateConversation(state, action.sessionId, (conversation) => ({
        ...reduceEvent(conversation, {
          sessionId: action.sessionId,
          kind: 'error',
          data: { message: action.error },
        }),
        messages: conversation.messages.filter((message) => message.id !== action.pendingId),
        failedSend:
          conversation.messages.find((message) => message.id === action.pendingId) ?? null,
      }));
    case 'permissionResponded':
      return updateConversation(state, action.sessionId, (conversation) => ({
        ...conversation,
        permissions: conversation.permissions.filter(
          (permission) => permission.requestId !== action.requestId,
        ),
      }));
    case 'permissionModeChangeStarted':
      if (state.deletedSessionIds.has(action.sessionId)) return state;
      return {
        ...state,
        permissionModeUpdates: new Set([...state.permissionModeUpdates, action.sessionId]),
        error: null,
      };
    case 'modelChangeStarted':
      if (state.deletedSessionIds.has(action.sessionId)) return state;
      return {
        ...state,
        modelUpdates: new Set([...state.modelUpdates, action.sessionId]),
        error: null,
      };
    case 'modelChanged':
    case 'modelUnconfirmed': {
      if (state.deletedSessionIds.has(action.sessionId)) return state;
      const modelUpdates = new Set(state.modelUpdates);
      modelUpdates.delete(action.sessionId);
      const modelUncertain = { ...state.modelUncertain };
      if (action.type === 'modelUnconfirmed') modelUncertain[action.sessionId] = action.error;
      else delete modelUncertain[action.sessionId];
      return {
        ...state,
        modelUpdates,
        modelUncertain,
        error: action.type === 'modelChanged' ? (action.notice ?? state.error) : state.error,
        snapshot:
          action.type === 'modelChanged' && state.snapshot
            ? {
                ...state.snapshot,
                sessions: state.snapshot.sessions.map((session) =>
                  session.id === action.sessionId ? { ...session, ...action.selection } : session,
                ),
              }
            : state.snapshot,
      };
    }
    case 'permissionModeChanged':
    case 'permissionModeUnconfirmed': {
      if (state.deletedSessionIds.has(action.sessionId)) return state;
      const permissionModeUpdates = new Set(state.permissionModeUpdates);
      permissionModeUpdates.delete(action.sessionId);
      const permissionModeUncertain = { ...state.permissionModeUncertain };
      if (action.type === 'permissionModeUnconfirmed')
        permissionModeUncertain[action.sessionId] = action.error;
      else delete permissionModeUncertain[action.sessionId];
      return {
        ...state,
        permissionModeUpdates,
        permissionModeUncertain,
        error:
          action.type === 'permissionModeChanged' ? (action.notice ?? state.error) : state.error,
        snapshot:
          action.type === 'permissionModeChanged' && state.snapshot
            ? {
                ...state.snapshot,
                sessions: state.snapshot.sessions.map((session) =>
                  session.id === action.sessionId
                    ? { ...session, permissionMode: action.permissionMode }
                    : session,
                ),
              }
            : state.snapshot,
      };
    }
    case 'errorReported':
      return {
        ...state,
        error: action.error,
        runtimeError: action.error === null ? null : state.runtimeError,
      };
    case 'runtimeRecovered':
      return { ...state, runtimeError: null };
  }
}
