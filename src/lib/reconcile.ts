import { emptyConversation, reduceEvent } from './conversation';
import type { Conversation } from './conversation';
import type { Session, Snapshot } from './types';

/** Requests that overlap a local send or a newer runtime event cannot replace that session. */
export function protectedSessionIds(
  requested: Readonly<Record<string, number>>,
  current: Readonly<Record<string, number>>,
  pendingAtRequest: ReadonlySet<string>,
  pendingNow: ReadonlySet<string>,
): ReadonlySet<string> {
  return new Set([
    ...Object.keys(current).filter((id) => current[id] !== (requested[id] ?? 0)),
    ...pendingAtRequest,
    ...pendingNow,
  ]);
}

export function reconcileSessions(
  current: Session[],
  incoming: Session[],
  protectedIds: ReadonlySet<string>,
  titleProtectedIds: ReadonlySet<string> = new Set(),
): Session[] {
  const byId = new Map(current.map((session) => [session.id, session]));
  return [
    ...incoming.map((session) => {
      const previous = byId.get(session.id);
      if (!previous) return session;
      return {
        ...(protectedIds.has(session.id) ? previous : session),
        title: titleProtectedIds.has(session.id) ? previous.title : session.title,
      };
    }),
    ...current.filter(
      (session) => protectedIds.has(session.id) && !incoming.some((item) => item.id === session.id),
    ),
  ];
}

export function reconcileConversations(
  current: Record<string, Conversation>,
  snapshot: Pick<Snapshot, 'sessions' | 'activeTurns'>,
  protectedIds: ReadonlySet<string>,
): Record<string, Conversation> {
  const next = { ...current };
  for (const session of snapshot.sessions) {
    if (protectedIds.has(session.id)) continue;
    const state = current[session.id] ?? emptyConversation;
    const turn = snapshot.activeTurns[session.id];
    const reconciled = reduceEvent(state, {
      sessionId: session.id,
      kind: 'status',
      data: { status: session.status },
    });
    next[session.id] = {
      ...reconciled,
      stream: turn?.stream ?? '',
      agentStreams: turn?.agentStreams ?? {},
      permissions: turn?.permissions ?? [],
    };
  }
  return next;
}
