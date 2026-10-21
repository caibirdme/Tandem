import type { ComponentProps } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { emptyConversation } from '../lib/conversation';
import { initialTandemState, reduceTandem } from '../lib/tandemState';
import type { Message, Project, Provider, Session } from '../lib/types';
import { Chat } from './Chat';

const project: Project = {
  id: 'project-1',
  name: 'Project',
  path: '/project',
  connection: { kind: 'local' },
  createdAt: '2026-10-17T00:00:00Z',
};
const session: Session = {
  reasoningEffort: null,
  contextUsage: null,
  titleState: 'preserved',
  id: 'session-1',
  title: 'Work',
  projectId: project.id,
  providerId: 'provider-1',
  modelId: 'model-1',
  createdAt: project.createdAt,
  updatedAt: project.createdAt,
  status: 'idle',
  permissionMode: 'allow-all',
};
const provider: Provider = {
  id: session.providerId,
  name: 'Provider',
  protocol: 'openai-completions',
  baseUrl: 'http://localhost:8080',
  hasApiKey: true,
  models: [
    {
      id: session.modelId,
      name: 'Model',
      reasoningEffort: null,
      contextWindow: 1_000_000,
      maxOutputTokens: null,
    },
  ],
};
const props: ComponentProps<typeof Chat> = {
  session: undefined,
  project,
  providers: [],
  selection: null,
  onSelection: () => undefined,
  conversation: emptyConversation,
  historyLoading: false,
  historyLoaded: true,
  onReloadHistory: () => undefined,
  sending: false,
  permissionModeSaving: false,
  permissionModeError: null,
  modelSaving: false,
  modelError: null,
  reasoningEffortSaving: false,
  reasoningEffortError: null,
  onReasoningEffort: () => undefined,
  onConfirmReasoningEffort: () => undefined,
  onConfirmModel: () => undefined,
  onPermissionMode: () => undefined,
  onConfirmPermissionMode: () => undefined,
  runtime: undefined,
  installing: false,
  onInstall: () => undefined,
  onSettings: () => undefined,
  onSend: async () => undefined,
  onAbort: () => undefined,
  onPermission: async () => undefined,
  connected: true,
};
function modeControl(markup: string) {
  return markup.match(/<button[^>]*aria-label="Tool permissions: [^"]+"[^>]*>/)?.[0];
}
function modelControl(markup: string) {
  return markup.match(/<button[^>]*aria-label="Model: [^"]+"[^>]*>/)?.[0];
}
function reasoningControl(markup: string) {
  return markup.match(/<button[^>]*aria-label="Reasoning effort: [^"]+"[^>]*>/)?.[0];
}

afterEach(() => {
  setLocale('en');
});

describe('skill and thinking transcript rendering', () => {
  const user: Message = {
    id: 'user-skill',
    sessionId: session.id,
    role: 'user',
    content: 'Use $review-docs with a $25 budget',
    selectedSkills: [{ name: 'review-docs', path: '/private/skills/review-docs/SKILL.md' }],
    attachments: [],
    createdAt: session.createdAt,
  };
  const reasoning: Message = {
    ...user,
    id: 'thinking-1',
    role: 'reasoning',
    content: 'Reviewing the entry point and its references',
    reasoning: { status: 'running' },
  };

  it('renders selected skill identities from confirmed user history as chips', () => {
    const markup = renderToStaticMarkup(
      <Chat {...props} conversation={{ ...emptyConversation, messages: [user] }} />,
    );
    expect(markup).toContain('class="sent-skill-mention-name">$review-docs</span>');
    expect(markup).toContain(' with a $25 budget');
    expect(markup).not.toContain('/private/skills');
  });

  it('starts a waiting thinking disclosure collapsed before the model shares text', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{ ...emptyConversation, status: 'running', messages: [user] }}
      />,
    );
    expect(markup.match(/class="thinking-message running"/g)).toHaveLength(1);
    expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
    expect(markup).toContain('Waiting for the model to share its thinking…');
  });

  it('shows one live thinking disclosure without duplicating the empty waiting indicator', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{ ...emptyConversation, status: 'running', messages: [user, reasoning] }}
      />,
    );
    expect(markup.match(/class="thinking-message running"/g)).toHaveLength(1);
    expect(markup).toContain(reasoning.content);
    expect(markup).not.toContain('class="tool-message"');
    expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
  });

  it.each(['completed', 'interrupted'] as const)(
    'keeps $status thinking readable without a running indicator after the turn ends',
    (status) => {
      const markup = renderToStaticMarkup(
        <Chat
          {...props}
          conversation={{
            ...emptyConversation,
            messages: [user, { ...reasoning, reasoning: { status } }],
          }}
        />,
      );
      expect(markup).toContain(`class="thinking-message ${status}"`);
      expect(markup).toContain(reasoning.content);
      expect(markup).not.toContain('thinking-message-dots');
      expect(markup).not.toContain('class="thinking-message running"');
    },
  );
});

describe('Work skill discovery entry point', () => {
  it.each(['local', 'ssh'] as const)('offers skills in a new %s Work composer', (kind) => {
    const currentProject: Project = {
      ...project,
      connection:
        kind === 'local'
          ? { kind }
          : { kind, host: 'example.test', port: 22, user: 'dev', identityFile: null },
    };
    const markup = renderToStaticMarkup(<Chat {...props} project={currentProject} />);
    expect(markup).toContain('role="combobox"');
    expect(markup).toContain('aria-autocomplete="list"');
    expect(markup).toContain('$ for skills');
  });

  it('keeps Chat as a plain message input without project skill controls', () => {
    const markup = renderToStaticMarkup(<Chat {...props} project={undefined} />);
    expect(markup).not.toContain('role="combobox"');
    expect(markup).not.toContain('$ for skills');
    expect(markup).toContain('Enter to send · Shift Enter for a new line');
  });
});

describe('composer attachments', () => {
  it.each([
    { name: 'Chat', project: undefined },
    { name: 'local Work', project },
    {
      name: 'remote Work',
      project: {
        ...project,
        connection: {
          kind: 'ssh' as const,
          host: 'example.test',
          port: 22,
          user: 'dev',
          identityFile: null,
        },
      },
    },
  ])('offers multi-file attachments in $name', (view) => {
    const markup = renderToStaticMarkup(<Chat {...props} project={view.project} />);
    expect(markup).toContain('type="file"');
    expect(markup).toContain('multiple=""');
    expect(markup).toContain('aria-label="Add attachments"');
    expect(markup).toContain('Paste an image or drop files');
  });

  it('renders an attachment-only message without fabricating text', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [
            {
              id: 'message-1',
              sessionId: session.id,
              role: 'user',
              content: '',
              createdAt: session.createdAt,
              attachments: [
                { id: 'attachment-1', name: 'notes.txt', mimeType: 'text/plain', size: 5 },
              ],
            },
          ],
        }}
      />,
    );
    expect(markup).toContain('Download notes.txt');
    expect(markup).not.toContain('<p></p>');
  });
});

describe('conversation reasoning effort controls', () => {
  const reasoningProps = {
    ...props,
    session,
    providers: [provider],
    selection: { providerId: provider.id, modelId: session.modelId },
  };

  it.each([undefined, project])(
    'offers a model default in every new Chat and Work',
    (draftProject) => {
      const markup = renderToStaticMarkup(
        <Chat {...reasoningProps} project={draftProject} session={undefined} />,
      );
      expect(reasoningControl(markup)).toContain('Reasoning effort: Default');
      expect(reasoningControl(markup)).not.toContain('disabled');
    },
  );

  it('shows the effective provider default without storing it as an override', () => {
    const configured: Provider = {
      ...provider,
      models: provider.models.map((model) => ({ ...model, reasoningEffort: 'high' })),
    };
    const inherited = renderToStaticMarkup(<Chat {...reasoningProps} providers={[configured]} />);
    const overridden = renderToStaticMarkup(
      <Chat
        {...reasoningProps}
        providers={[configured]}
        session={{ ...session, reasoningEffort: 'low' }}
      />,
    );
    expect(reasoningControl(inherited)).toContain('Reasoning effort: Default (High)');
    expect(reasoningControl(overridden)).toContain('Reasoning effort: Low');
  });

  it.each([
    {
      name: 'active response',
      override: {
        session: { ...session, status: 'running' as const },
        conversation: { ...emptyConversation, status: 'running' as const },
      },
    },
    { name: 'send in progress', override: { sending: true } },
    { name: 'history loading', override: { historyLoading: true } },
    {
      name: 'tool approval pending',
      override: {
        conversation: {
          ...emptyConversation,
          permissions: [{ requestId: 'permission-1', kind: 'shell', description: 'Run a command' }],
        },
      },
    },
  ])('allows changing the next turn during $name', ({ override }) => {
    const markup = renderToStaticMarkup(<Chat {...reasoningProps} {...override} />);
    expect(reasoningControl(markup)).not.toContain('disabled');
    expect(markup).toContain('The current response continues unchanged.');
  });

  it.each([
    { name: 'disconnected', override: { connected: false } },
    { name: 'saving reasoning effort', override: { reasoningEffortSaving: true } },
    {
      name: 'reasoning effort unconfirmed',
      override: { reasoningEffortError: 'Cannot confirm reasoning effort' },
    },
    { name: 'saving the model', override: { modelSaving: true } },
    { name: 'model unconfirmed', override: { modelError: 'Cannot confirm model' } },
  ])('locks the selector while $name', ({ override }) => {
    const markup = renderToStaticMarkup(<Chat {...reasoningProps} {...override} />);
    expect(reasoningControl(markup)).toContain('disabled');
  });

  it('offers confirmation recovery without disabling the stop button', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...reasoningProps}
        session={{ ...session, status: 'running' }}
        reasoningEffortError="Could not confirm reasoning effort"
      />,
    );
    expect(markup).toContain('Confirm reasoning effort again');
    const stop = markup.match(/<button[^>]*aria-label="Stop generating"[^>]*>/)?.[0];
    expect(stop).toBeDefined();
    expect(stop).not.toContain('disabled');
  });

  it('translates the session control', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <Chat {...reasoningProps} session={{ ...session, reasoningEffort: 'high' }} />,
    );
    expect(markup).toContain('思考强度：高');
  });
});

describe('conversation model controls', () => {
  const modelProps = {
    ...props,
    session,
    providers: [provider],
    selection: { providerId: provider.id, modelId: session.modelId },
  };

  it('allows choosing a model in an existing idle conversation', () => {
    const markup = renderToStaticMarkup(<Chat {...modelProps} />);
    expect(modelControl(markup)).toContain('Model: Model');
    expect(modelControl(markup)).not.toContain('disabled=""');
    expect(markup).toContain('Choose a model for your next message.');
  });

  it.each([
    { name: 'creating a session', override: { sending: true } },
    { name: 'running', override: { session: { ...session, status: 'running' as const } } },
    { name: 'loading history', override: { historyLoading: true } },
    { name: 'saving the model', override: { modelSaving: true } },
    {
      name: 'model is unconfirmed',
      override: { modelError: 'Could not confirm the current model' },
    },
    { name: 'saving permissions', override: { permissionModeSaving: true } },
    {
      name: 'permissions are unconfirmed',
      override: { permissionModeError: 'Could not confirm permissions' },
    },
    { name: 'disconnected', override: { connected: false } },
    {
      name: 'awaiting approval',
      override: {
        conversation: {
          ...emptyConversation,
          permissions: [{ requestId: 'permission-1', kind: 'shell', description: 'Run command' }],
        },
      },
    },
  ])('blocks model changes while $name', ({ override }) => {
    expect(modelControl(renderToStaticMarkup(<Chat {...modelProps} {...override} />))).toContain(
      'disabled=""',
    );
  });

  it('keeps the confirmed model visible while saving and offers confirmation after failure', () => {
    const pending = renderToStaticMarkup(<Chat {...modelProps} modelSaving />);
    expect(modelControl(pending)).toContain('Model: Model');
    expect(modelControl(pending)).toContain('aria-busy="true"');
    const uncertain = renderToStaticMarkup(
      <Chat {...modelProps} modelError="Could not confirm the current model" />,
    );
    expect(modelControl(uncertain)).toContain('Model: Unconfirmed');
    expect(modelControl(uncertain)).toContain('aria-busy="false"');
    expect(uncertain).toContain('Confirm model again');
    expect(modeControl(uncertain)).toContain('disabled=""');
  });
});

describe('runtime preparation notices', () => {
  it.each([
    { name: 'chat', project: undefined },
    { name: 'local work', project },
  ])('offers local runtime preparation for $name before the first session', (view) => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        project={view.project}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
        runtime={{
          available: false,
          version: null,
          message: 'The local runtime is not initialized',
        }}
      />,
    );
    expect(markup).toContain('The local runtime is not initialized');
    expect(markup).toContain('Prepare runtime');
  });

  it('does not offer local runtime preparation for SSH work', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        project={{
          ...project,
          connection: {
            kind: 'ssh',
            host: 'server.example',
            port: 22,
            user: 'tandem',
            identityFile: null,
          },
        }}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
        runtime={{
          available: false,
          version: null,
          message: 'The local runtime is not initialized',
        }}
      />,
    );
    expect(markup).not.toContain('The local runtime is not initialized');
    expect(markup).not.toContain('Prepare runtime');
    expect(markup).toContain('Model');
  });
});

describe('work permission controls', () => {
  it.each([
    {
      name: 'unread existing ask work',
      currentSession: { ...session, permissionMode: 'ask' as const },
      historyLoaded: false,
      expected: true,
    },
    {
      name: 'confirmed empty ask work',
      currentSession: { ...session, permissionMode: 'ask' as const },
      historyLoaded: true,
      expected: false,
    },
    { name: 'new work draft', currentSession: undefined, historyLoaded: false, expected: false },
    {
      name: 'explicit full authorization',
      currentSession: session,
      historyLoaded: false,
      expected: false,
    },
  ])('offers history retry for $name', ({ currentSession, historyLoaded, expected }) => {
    const markup = renderToStaticMarkup(
      <Chat {...props} session={currentSession} historyLoaded={historyLoaded} />,
    );
    expect(markup.includes('Reload conversation')).toBe(expected);
  });

  it('defaults each new work view to ask and shows no permission selector in chat', () => {
    expect(modeControl(renderToStaticMarkup(<Chat {...props} />))).toContain(
      'Tool permissions: Ask each time',
    );
    expect(
      modeControl(renderToStaticMarkup(<Chat {...props} project={undefined} />)),
    ).toBeUndefined();
    expect(modeControl(renderToStaticMarkup(<Chat {...props} session={session} />))).toContain(
      'Tool permissions: Full access',
    );
    expect(modeControl(renderToStaticMarkup(<Chat {...props} />))).toContain(
      'Tool permissions: Ask each time',
    );
  });

  it.each([
    { name: 'initializing', override: { sending: true } },
    { name: 'running', override: { session: { ...session, status: 'running' as const } } },
    {
      name: 'awaiting approval',
      override: {
        conversation: {
          ...emptyConversation,
          status: 'running' as const,
          permissions: [{ requestId: 'permission-1', kind: 'shell', description: 'Run command' }],
        },
      },
    },
    { name: 'saving a new mode', override: { permissionModeSaving: true } },
  ])('disables the mode selector while $name', ({ override }) => {
    const markup = renderToStaticMarkup(<Chat {...props} session={session} {...override} />);
    expect(modeControl(markup)).toContain('disabled=""');
  });

  it('keeps the approved mode visible with its scope explanation', () => {
    const markup = renderToStaticMarkup(<Chat {...props} session={session} permissionModeSaving />);
    expect(modeControl(markup)).toContain('Tool permissions: Full access');
    expect(modeControl(markup)).toContain('aria-busy="true"');
    expect(markup).toContain(
      'automatically approves file access, commands, and all other tool requests.',
    );
  });

  it('allows revoking authorization after an error when no task or approval remains active', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        session={{ ...session, status: 'error' }}
        conversation={{ ...emptyConversation, status: 'error' }}
      />,
    );
    expect(modeControl(markup)).not.toContain('disabled=""');
    expect(modeControl(markup)).toContain('Tool permissions: Full access');
  });

  it('shows an actionable uncertain state without an endless spinner', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        session={session}
        permissionModeError="Could not confirm current tool permissions"
      />,
    );
    expect(modeControl(markup)).toContain('Tool permissions: Unconfirmed');
    expect(modeControl(markup)).toContain('disabled=""');
    expect(modeControl(markup)).toContain('aria-busy="false"');
    expect(markup).toContain('Confirm permissions again');
    expect(markup).toContain('Confirm permissions before sending a task.');
  });
});

describe('conversation language', () => {
  it('renders Chinese conversation and permission controls after changing locale', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
      />,
    );
    expect(markup).toContain('今天，一起构建什么？');
    expect(markup).toContain('Agent 已准备好在 Project 中和你一起工作。');
    expect(markup).toContain('aria-label="工具权限：逐次询问"');
    expect(markup).toContain('aria-label="模型：Model"');
    expect(markup).toContain('描述你想完成的任务…');
    expect(markup).not.toContain('What will we build today?');
  });
});

describe('work context usage', () => {
  const contextProps = {
    ...props,
    providers: [provider],
    selection: { providerId: provider.id, modelId: session.modelId },
    session: { ...session, contextUsage: { currentTokens: 153_000, tokenLimit: 1_000_000 } },
  };

  it.each(['local', 'ssh'] as const)('shows current context below the %s Work composer', (kind) => {
    const currentProject: Project = {
      ...project,
      connection:
        kind === 'local'
          ? { kind }
          : { kind, host: 'example.com', port: 22, user: 'dev', identityFile: null },
    };
    const markup = renderToStaticMarkup(<Chat {...contextProps} project={currentProject} />);
    expect(markup.indexOf('class="context-usage ')).toBeGreaterThan(markup.indexOf('</form>'));
    expect(markup).toContain('153k / 1M');
    expect(markup).toContain('>15.3%</span>');
    expect(markup).not.toContain('153,000 / 1,000,000 tokens');
    expect(markup).toContain('role="meter"');
    expect(markup).toContain('aria-valuenow="153000"');
    expect(markup).toContain('aria-valuemax="1000000"');
    expect(Number(markup.match(/stroke-dasharray="([\d.]+) 100"/)?.[1])).toBeCloseTo(15.3);
  });

  it('does not show a context indicator in Chat', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...contextProps}
        project={undefined}
        session={{ ...contextProps.session, projectId: null }}
      />,
    );
    expect(markup).not.toContain('context-usage');
  });

  it('shows unknown usage instead of a fabricated zero before the first report', () => {
    const markup = renderToStaticMarkup(<Chat {...contextProps} session={session} />);
    expect(markup).toContain('— / 1M');
    expect(markup).toContain('Usage will appear when the model reports it.');
    expect(markup).not.toContain('aria-valuenow');
  });

  it('bounds the ring while preserving the actual over-limit count', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...contextProps}
        session={{ ...session, contextUsage: { currentTokens: 300_000, tokenLimit: 256_000 } }}
      />,
    );
    expect(markup).toContain('300k / 256k');
    expect(markup).toContain('aria-valuenow="256000"');
    expect(markup).toContain('stroke-dasharray="100 100"');
    expect(markup).toContain('117.2% used');
  });

  it('translates the context tooltip', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(<Chat {...contextProps} />);
    expect(markup).toContain('aria-label="上下文窗口"');
    expect(markup).toContain('已使用 15.3%');
  });
});

describe('grouped activity timeline', () => {
  const activity: Message = {
    attachments: [],
    id: 'activity-1',
    sessionId: session.id,
    role: 'activity',
    content: 'Inspecting the conversation layout',
    createdAt: session.createdAt,
    activity: {
      status: 'completed',
      tools: [
        {
          toolCallId: 'read-1',
          name: 'read_file',
          title: 'Read the chat component',
          status: 'completed',
          arguments: { path: 'src/components/Chat.tsx' },
          result: { lines: 40 },
          progress: 'Read 40 lines',
        },
        {
          toolCallId: 'read-2',
          name: 'read_file',
          title: 'Read the conversation state',
          status: 'completed',
        },
      ],
    },
  };
  const answer: Message = {
    attachments: [],
    id: 'answer',
    sessionId: session.id,
    role: 'assistant',
    content: 'The conversation now groups related tool calls.',
    createdAt: session.createdAt,
  };

  it('collapses each stage and its tools while keeping final replies in order', () => {
    const nextActivity = {
      ...activity,
      id: 'activity-2',
      content: 'Checking the updated layout',
    };
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [activity, nextActivity, answer],
          stream: 'One more detail',
        }}
      />,
    );
    expect(markup.match(/class="activity-group"/g)).toHaveLength(2);
    expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
    const summary = markup.match(/<details class="activity-group"><summary>(.*?)<\/summary>/)?.[1];
    expect(summary).not.toContain(activity.content);
    expect(markup).toContain(
      `<p class="activity-intent">${activity.content}</p><details class="activity-group">`,
    );
    expect(summary).not.toContain('tool calls');
    expect(summary).not.toContain('activity-count');
    expect(summary).not.toContain('Read the chat component');
    expect(markup).toContain('Read the chat component');
    expect(markup).toContain('Read the conversation state');
    expect(markup).toContain('Arguments');
    expect(markup).toContain('src/components/Chat.tsx');
    expect(markup).toContain('Result');
    expect(markup).toContain('Read 40 lines');
    expect(markup.indexOf(nextActivity.content)).toBeGreaterThan(markup.indexOf(activity.content));
    expect(markup.indexOf(answer.content)).toBeGreaterThan(markup.indexOf(nextActivity.content));
    expect(markup.indexOf('One more detail')).toBeGreaterThan(markup.indexOf(answer.content));
  });

  it('keeps failures and pending approvals visible outside collapsed tool details', () => {
    const failed: Message = {
      ...activity,
      activity: {
        status: 'error',
        tools: [
          {
            toolCallId: 'check-1',
            name: 'run_check',
            title: 'Check the layout',
            status: 'error',
            error: { message: 'The check failed' },
          },
        ],
      },
    };
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [failed, answer],
          permissions: [
            { requestId: 'approval', kind: 'shell', description: 'Run the verification command' },
          ],
        }}
      />,
    );
    expect(markup).toContain('class="activity-status error">Failed');
    const summary = markup.match(/<details class="activity-group"><summary>(.*?)<\/summary>/)?.[1];
    expect(summary).toContain(
      '<span class="activity-group-status"><span class="activity-status error">Failed</span></span>',
    );
    expect(summary).not.toContain('activity-failed-count');
    expect(markup).toContain('Check the layout failed. Expand this step for details.');
    expect(markup.indexOf('class="activity-failure"')).toBeGreaterThan(
      markup.lastIndexOf('</details>'),
    );
    expect(markup).toContain('The check failed');
    expect(markup).toContain('Your approval is required');
    expect(markup).toContain('Run the verification command');
    expect(markup).toContain('Allow once');
    expect(markup).toContain(answer.content);
  });

  it('uses translated fallback headings and distinguishes stopped activities', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [
            {
              ...activity,
              content: '',
              activity: {
                status: 'interrupted',
                tools: [{ toolCallId: 'stopped-1', name: 'read_file', status: 'interrupted' }],
              },
            },
          ],
        }}
      />,
    );
    expect(markup).toContain('Agent 活动');
    expect(markup).toContain('class="activity-message interrupted"');
    expect(markup).not.toContain('次工具调用');
    expect(markup).toContain('已停止');
    expect(markup).not.toContain('已完成');
  });

  it('keeps failures visible while another call is still running without showing counts', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          status: 'running',
          messages: [
            {
              ...activity,
              activity: {
                status: 'running',
                tools: [
                  { toolCallId: 'failed-1', name: 'check_layout', status: 'error' },
                  { toolCallId: 'running-1', name: 'read_file', status: 'running' },
                ],
              },
            },
          ],
        }}
      />,
    );
    const summary = markup.match(/<details class="activity-group"><summary>(.*?)<\/summary>/)?.[1];
    expect(summary).not.toContain('1 failed');
    expect(summary).not.toContain('activity-failed-count');
    expect(summary).not.toContain('activity-count');
    expect(summary).toContain(
      '<span class="activity-group-status"><span class="activity-status running">Running</span></span>',
    );
    expect(markup).toContain('class="activity-message running"');
    expect(markup).toContain('check_layout failed. Expand this step for details.');
    expect(markup).not.toContain('class="thinking"');
  });

  it('keeps next-turn reasoning editable through an active group and pending approval', () => {
    const conversation = {
      ...emptyConversation,
      status: 'running' as const,
      messages: [
        {
          ...activity,
          activity: {
            status: 'running' as const,
            tools: [{ toolCallId: 'run-1', name: 'bash', status: 'running' as const }],
          },
        },
      ],
      stream: 'Reviewing the command output',
      permissions: [
        { requestId: 'approval-1', kind: 'shell', description: 'Run the verification command' },
      ],
    };
    const saving = reduceTandem(
      { ...initialTandemState, conversations: { [session.id]: conversation } },
      { type: 'reasoningEffortChangeStarted', sessionId: session.id },
    );
    const changed = reduceTandem(saving, {
      type: 'reasoningEffortChanged',
      sessionId: session.id,
      reasoningEffort: 'high',
    });
    expect(changed.conversations[session.id]).toBe(conversation);
    expect(changed.reasoningEffortUpdates.has(session.id)).toBe(false);
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        session={{ ...session, status: 'running', reasoningEffort: 'high' }}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
        conversation={changed.conversations[session.id] ?? emptyConversation}
      />,
    );
    expect(reasoningControl(markup)).toContain('Reasoning effort: High');
    expect(reasoningControl(markup)).not.toContain('disabled');
    expect(modelControl(markup)).toContain('disabled');
    expect(markup).toContain('class="activity-status running">Running');
    expect(markup).toContain(activity.content);
    expect(markup).toContain(conversation.stream);
    expect(markup).toContain('Run the verification command');
    expect(markup).toContain('Allow once');
  });
});
