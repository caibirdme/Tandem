import type { ActivityStatus, Message } from './types';

const statusPriority: Record<ActivityStatus, number> = {
  completed: 0,
  interrupted: 1,
  error: 2,
  running: 3,
};

/** Combine adjacent thinking blocks for display without changing stored messages. */
export function groupThinkingMessages<T extends Message>(messages: readonly T[]): T[] {
  const groups: T[][] = [];
  for (const message of messages) {
    const group = groups.at(-1);
    const previous = group?.[0];
    if (
      group &&
      previous?.role === 'reasoning' &&
      message.role === 'reasoning' &&
      previous.sessionId === message.sessionId &&
      previous.agentId === message.agentId
    )
      group.push(message);
    else groups.push([message]);
  }

  return groups.flatMap((group) => {
    const first = group[0];
    if (!first) return [];
    if (group.length === 1) return [first];
    const status = group.reduce<ActivityStatus>((current, message) => {
      const next = message.reasoning?.status ?? 'completed';
      return statusPriority[next] > statusPriority[current] ? next : current;
    }, 'completed');
    return [
      {
        ...first,
        content: group
          .map((message) => message.content)
          .filter(Boolean)
          .join('\n\n'),
        reasoning: { status },
      },
    ];
  });
}
