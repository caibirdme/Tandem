import type { RefObject } from 'react';

import type { Message, Subagent } from '../lib/types';
import { DetailPanel } from './DetailPanel';
import { AgentOutput, AgentStatus } from './FleetCard';

export function SubagentDetailPanel({
  id,
  agent,
  messages,
  stream,
  onClose,
  returnFocusRef,
}: {
  id: string;
  agent: Subagent;
  messages: Message[];
  stream: string;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null> | undefined;
}) {
  return (
    <DetailPanel
      id={id}
      title={agent.displayName || agent.name}
      onClose={onClose}
      returnFocusRef={returnFocusRef}
    >
      <div className="subagent-panel-summary">
        <AgentStatus status={agent.status} />
        {agent.model && <span className="fleet-agent-model">{agent.model}</span>}
        {agent.description && <p>{agent.description}</p>}
      </div>
      <AgentOutput key={agent.id} agent={agent} messages={messages} stream={stream} />
    </DetailPanel>
  );
}
