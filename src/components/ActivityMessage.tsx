import { Check, ChevronRight, CircleAlert, CircleSlash } from 'lucide-react';
import { memo } from 'react';

import { useI18n } from '../i18n';
import type { MessageKey } from '../i18n';
import type { Activity, ActivityStatus, ToolActivity } from '../lib/types';
import { Spinner } from './ui';

function StatusIcon({ status }: { status: ActivityStatus }) {
  if (status === 'running') return <Spinner size={13} />;
  if (status === 'error') return <CircleAlert size={13} />;
  if (status === 'interrupted') return <CircleSlash size={13} />;
  return <Check size={13} />;
}

function ActivityStatusLabel({ status }: { status: ActivityStatus }) {
  const { t } = useI18n();
  const labels = {
    running: 'chat.toolRunning',
    completed: 'chat.toolCompleted',
    error: 'chat.toolFailed',
    interrupted: 'chat.activityStopped',
  } as const;
  return <span className={`activity-status ${status}`}>{t(labels[status])}</span>;
}

function payloadText(value: unknown): string {
  return typeof value === 'string' ? value : (JSON.stringify(value, null, 2) ?? '');
}

function toolCategory(name: string) {
  switch (name.replace(/^builtin:/u, '')) {
    case 'view':
      return 'read';
    case 'grep':
    case 'glob':
      return 'search';
    case 'create':
    case 'edit':
    case 'apply_patch':
      return 'edit';
    case 'bash':
    case 'powershell':
    case 'read_bash':
    case 'write_bash':
    case 'stop_bash':
    case 'read_powershell':
    case 'write_powershell':
    case 'stop_powershell':
      return 'command';
    case 'web_fetch':
      return 'web';
    default:
      return 'unknown';
  }
}

function fallbackHeading(tools: ToolActivity[]): MessageKey {
  const categories = new Set(tools.map((tool) => toolCategory(tool.name)));
  if (categories.size === 1) {
    if (categories.has('read')) return 'chat.activityReadingFiles';
    if (categories.has('search')) return 'chat.activitySearchingProject';
    if (categories.has('edit')) return 'chat.activityUpdatingFiles';
    if (categories.has('command')) return 'chat.activityRunningCommands';
    if (categories.has('web')) return 'chat.activityReadingWeb';
  }
  if (categories.size === 2 && categories.has('read') && categories.has('search'))
    return 'chat.activityInspectingProject';
  return 'chat.activityTitle';
}

function ToolDetails({ tool }: { tool: ToolActivity }) {
  const { t } = useI18n();
  return (
    <details className={`activity-tool ${tool.status}`}>
      <summary>
        <StatusIcon status={tool.status} />
        <span className="activity-title">{tool.title || tool.name}</span>
        <ActivityStatusLabel status={tool.status} />
        <ChevronRight className="activity-chevron" size={13} />
      </summary>
      <div className="activity-tool-detail">
        {tool.title && tool.title !== tool.name && <code>{tool.name}</code>}
        {tool.progress && <p className="activity-progress">{tool.progress}</p>}
        {tool.arguments !== undefined && (
          <div className="activity-payload">
            <strong>{t('chat.toolArguments')}</strong>
            <pre>{payloadText(tool.arguments)}</pre>
          </div>
        )}
        {tool.result !== undefined && (
          <div className="activity-payload">
            <strong>{t('chat.toolResult')}</strong>
            <pre>{payloadText(tool.result)}</pre>
          </div>
        )}
        {tool.error !== undefined && (
          <div className="activity-payload activity-tool-error">
            <strong>{t('chat.toolError')}</strong>
            <pre>{payloadText(tool.error)}</pre>
          </div>
        )}
      </div>
    </details>
  );
}

export const ActivityMessage = memo(function ActivityMessage({
  content,
  activity,
}: {
  content: string;
  activity: Activity | undefined;
}) {
  const { t } = useI18n();
  if (!activity) return null;
  const intent = content.trim();
  const failedTools = activity.tools.filter((tool) => tool.status === 'error');
  return (
    <section
      className={`activity-message ${activity.status}`}
      aria-busy={activity.status === 'running'}
    >
      {intent && <p className="activity-intent">{intent}</p>}
      <details className="activity-group">
        <summary>
          <ChevronRight className="activity-chevron" size={13} aria-hidden="true" />
          <span className="activity-title">{t(fallbackHeading(activity.tools))}</span>
          <span className="activity-group-status">
            <ActivityStatusLabel status={activity.status} />
          </span>
        </summary>
        <div className="activity-tools">
          {activity.tools.map((tool) => (
            <ToolDetails key={tool.toolCallId} tool={tool} />
          ))}
          {activity.tools.length === 0 && (
            <p className="activity-empty">{t('chat.activityNoTools')}</p>
          )}
        </div>
      </details>
      {failedTools.map((tool) => (
        <p className="activity-failure" key={tool.toolCallId}>
          <CircleAlert size={12} />
          {t('chat.activityToolFailed', { name: tool.title || tool.name })}
        </p>
      ))}
    </section>
  );
});
