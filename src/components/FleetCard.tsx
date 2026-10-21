import { ArrowLeft, Check, ChevronRight, CircleAlert, CircleSlash, Network } from 'lucide-react';
import { lazy, Suspense, useId, useLayoutEffect, useRef } from 'react';

import { useI18n } from '../i18n';
import { groupThinkingMessages } from '../lib/thinkingMessages';
import type { ActivityStatus, Message, Subagent } from '../lib/types';
import { ActivityMessage } from './ActivityMessage';
import { ThinkingMessage } from './ThinkingMessage';
import { Spinner } from './ui';

import './FleetCard.css';

const Markdown = lazy(() => import('./Markdown'));

function AgentStatus({ status }: { status: ActivityStatus }) {
  const { t } = useI18n();
  const label = {
    running: 'chat.toolRunning',
    completed: 'chat.toolCompleted',
    error: 'chat.toolFailed',
    interrupted: 'chat.activityStopped',
  } as const;
  return (
    <span className={`fleet-agent-status ${status}`}>
      {status === 'running' ? (
        <Spinner size={12} />
      ) : status === 'error' ? (
        <CircleAlert size={12} />
      ) : status === 'interrupted' ? (
        <CircleSlash size={12} />
      ) : (
        <Check size={12} />
      )}
      {t(label[status])}
    </span>
  );
}

function AgentText({ content }: { content: string }) {
  return (
    <Suspense fallback={<div className="markdown">{content}</div>}>
      <Markdown content={content} />
    </Suspense>
  );
}

export function AgentOutput({
  agent,
  messages,
  stream,
}: {
  agent: Subagent;
  messages: Message[];
  stream: string;
}) {
  const { t } = useI18n();
  const viewport = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  const output = groupThinkingMessages(
    messages.filter((message) => message.agentId === agent.id && message.role !== 'agent'),
  );

  useLayoutEffect(() => {
    if (followLatest.current && viewport.current)
      viewport.current.scrollTop = viewport.current.scrollHeight;
  }, [messages, stream]);

  return (
    <div
      ref={viewport}
      className="fleet-agent-output"
      role="log"
      aria-label={t('fleet.outputLabel', { name: agent.displayName || agent.name })}
      onScroll={(event) => {
        const element = event.currentTarget;
        followLatest.current = element.scrollHeight - element.scrollTop - element.clientHeight < 40;
      }}
    >
      {output.map((message) =>
        message.role === 'activity' ? (
          <ActivityMessage key={message.id} content={message.content} activity={message.activity} />
        ) : message.role === 'reasoning' ? (
          <ThinkingMessage
            key={message.id}
            content={message.content}
            status={message.reasoning?.status ?? 'completed'}
          />
        ) : message.role === 'assistant' ? (
          <article className="fleet-agent-message" key={message.id}>
            <AgentText content={message.content} />
            {message.interrupted && (
              <span className="interrupted-label">{t('chat.interrupted')}</span>
            )}
          </article>
        ) : (
          <pre className="fleet-agent-tool-output" key={message.id}>
            {message.content}
          </pre>
        ),
      )}
      {stream && (
        <article className="fleet-agent-message streaming">
          <AgentText content={stream} />
        </article>
      )}
      {!output.length && !stream && !agent.error && (
        <p className="fleet-empty-output">
          {agent.status === 'running' ? <Spinner size={13} /> : <Network size={14} />}
          {t(agent.status === 'running' ? 'fleet.waitingOutput' : 'fleet.noOutput')}
        </p>
      )}
      {agent.error && (
        <p className="fleet-agent-error">
          <CircleAlert size={14} />
          {agent.error}
        </p>
      )}
    </div>
  );
}

export function FleetCard({
  agents,
  messages,
  streams,
  selectedAgentId,
  onSelect,
}: {
  agents: Subagent[];
  messages: Message[];
  streams: Record<string, string>;
  selectedAgentId: string | null;
  onSelect: (id: string | null) => void;
}) {
  const { t } = useI18n();
  const inspectorId = useId();
  const selectedButton = useRef<HTMLButtonElement>(null);
  const selected = agents.find((agent) => agent.id === selectedAgentId);
  const running = agents.filter((agent) => agent.status === 'running').length;
  if (!agents.length) return null;

  return (
    <section className="fleet-card" aria-label={t('fleet.agentsLabel')}>
      <div className="fleet-card-heading">
        <Network size={15} />
        <strong>{t('fleet.title')}</strong>
        <span>
          {t(running ? 'fleet.runningCount' : 'fleet.totalCount', {
            count: running || agents.length,
          })}
        </span>
      </div>
      <div className="fleet-agent-list">
        {agents.map((agent) => (
          <button
            type="button"
            className={`fleet-agent-row ${selected?.id === agent.id ? 'selected' : ''}`}
            key={agent.id}
            ref={selected?.id === agent.id ? selectedButton : undefined}
            aria-pressed={selected?.id === agent.id}
            aria-controls={selected?.id === agent.id ? inspectorId : undefined}
            aria-label={t('fleet.viewAgent', { name: agent.displayName || agent.name })}
            onClick={() => onSelect(selected?.id === agent.id ? null : agent.id)}
          >
            <span className="fleet-agent-avatar">
              <Network size={14} />
            </span>
            <span className="fleet-agent-identity">
              <strong>{agent.displayName || agent.name}</strong>
              {agent.description && agent.description !== agent.displayName && (
                <span title={agent.description}>{agent.description}</span>
              )}
            </span>
            <AgentStatus status={agent.status} />
            <ChevronRight className="fleet-agent-chevron" size={14} />
          </button>
        ))}
      </div>
      {selected && (
        <section className="fleet-inspector" id={inspectorId} aria-label={t('fleet.agentDetails')}>
          <div className="fleet-inspector-heading">
            <button
              type="button"
              className="fleet-back-button"
              onClick={() => {
                selectedButton.current?.focus();
                onSelect(null);
              }}
            >
              <ArrowLeft size={14} />
              {t('fleet.backToConversation')}
            </button>
            {selected.model && <span className="fleet-agent-model">{selected.model}</span>}
          </div>
          <AgentOutput
            key={selected.id}
            agent={selected}
            messages={messages}
            stream={streams[selected.id] ?? ''}
          />
        </section>
      )}
    </section>
  );
}
