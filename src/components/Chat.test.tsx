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

describe('Multi-agent composer and transcript', () => {
  it('offers an unpressed Expert team mode button in Work and no mode button in Chat', () => {
    const work = renderToStaticMarkup(<Chat {...props} />);
    const chat = renderToStaticMarkup(<Chat {...props} project={undefined} />);
    const button = work.match(/<button[^>]*class="fleet-toggle"[^>]*>/)?.[0];
    expect(button).toBeDefined();
    expect(button).toContain('type="button"');
    expect(button).toContain('aria-pressed="false"');
    expect(button).not.toContain('disabled');
    expect(work).not.toContain('type="checkbox"');
    expect(work).toContain(
      'Let the AI split your task and run independent parts with multiple agents.',
    );
    expect(work).toContain('Expert team mode');
    expect(chat).not.toContain('fleet-toggle');
  });

  it('disables Expert team mode changes while a turn is running', () => {
    const markup = renderToStaticMarkup(
      <Chat {...props} conversation={{ ...emptyConversation, status: 'running' }} />,
    );
    const button = markup.match(/<button[^>]*class="fleet-toggle"[^>]*>/)?.[0];
    expect(button).toContain('disabled');
    expect(button).toContain('aria-pressed="false"');
  });

  it('groups workers by user turn and keeps their traces out of the main conversation', () => {
    const user: Message = {
      id: 'user-1',
      sessionId: 'session',
      role: 'user',
      content: 'Review the project',
      createdAt: '2026-10-21T10:00:00Z',
      attachments: [],
    };
    const worker = (id: string): Message => ({
      ...user,
      id: `summary-${id}`,
      role: 'agent',
      content: '',
      agent: {
        id,
        toolCallId: `call-${id}`,
        name: id,
        displayName: `Worker ${id}`,
        description: '',
        status: 'running',
      },
    });
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          agentStreams: { a: 'Private child stream' },
          messages: [
            user,
            worker('a'),
            {
              ...user,
              id: 'child-message',
              role: 'assistant',
              agentId: 'a',
              content: 'Private child output',
            },
            {
              ...user,
              id: 'child-tool',
              role: 'activity',
              agentId: 'a',
              content: 'Private child tools',
              activity: { status: 'running', tools: [] },
            },
            worker('b'),
            { ...user, id: 'main-message', role: 'assistant', content: 'Main response' },
            { ...user, id: 'user-2', content: 'Another review' },
            worker('c'),
          ],
        }}
      />,
    );
    expect(markup.match(/class="fleet-card"/g)).toHaveLength(2);
    expect(markup).toContain('2 running');
    expect(markup).toContain('Worker a');
    expect(markup).toContain('Worker b');
    expect(markup).toContain('Worker c');
    expect(markup).toContain('Main response');
    expect(markup).not.toContain('Private child output');
    expect(markup).not.toContain('Private child stream');
    expect(markup).not.toContain('Private child tools');
  });
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

  it.each(['running', 'completed', 'interrupted', 'error'] as const)(
    'combines consecutive thinking in order and retains the %s state',
    (status) => {
      const markup = renderToStaticMarkup(
        <Chat
          {...props}
          conversation={{
            ...emptyConversation,
            status: status === 'running' ? 'running' : 'idle',
            messages: [
              user,
              { ...reasoning, reasoning: { status: 'completed' } },
              {
                ...reasoning,
                id: 'thinking-2',
                content: 'Checking the callers',
                reasoning: { status },
              },
            ],
          }}
        />,
      );
      expect(markup.match(/class="thinking-message /g)).toHaveLength(1);
      expect(markup).toContain(`class="thinking-message ${status}"`);
      expect(markup).toContain(`${reasoning.content}\n\nChecking the callers`);
      expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
    },
  );

  it('keeps the combined thinking disclosure active between reasoning blocks', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          status: 'running',
          messages: [
            user,
            { ...reasoning, reasoning: { status: 'completed' } },
            {
              ...reasoning,
              id: 'thinking-2',
              content: 'Checking the callers',
              reasoning: { status: 'completed' },
            },
          ],
        }}
      />,
    );
    expect(markup.match(/class="thinking-message /g)).toHaveLength(1);
    expect(markup).toContain('class="thinking-message running"');
    expect(markup).toContain(`${reasoning.content}\n\nChecking the callers`);
  });

  it('does not split thinking for worker records already represented by the same card', () => {
    const worker = (id: string): Message => ({
      ...user,
      id,
      role: 'agent',
      content: '',
      agent: {
        id,
        toolCallId: `call-${id}`,
        name: id,
        displayName: id,
        description: '',
        status: 'completed',
      },
    });
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [
            user,
            worker('worker-1'),
            { ...reasoning, reasoning: { status: 'completed' } },
            worker('worker-2'),
            {
              ...reasoning,
              id: 'thinking-2',
              content: 'Checking the callers',
              reasoning: { status: 'completed' },
            },
          ],
        }}
      />,
    );
    expect(markup.match(/class="fleet-card"/g)).toHaveLength(1);
    expect(markup.match(/class="thinking-message /g)).toHaveLength(1);
    expect(markup).toContain(`${reasoning.content}\n\nChecking the callers`);
  });

  it.each(['user', 'assistant', 'activity', 'tool', 'system', 'agent'] as const)(
    'starts a separate thinking disclosure after a %s message',
    (role) => {
      const separator: Message = {
        ...user,
        id: 'separator',
        role,
        content: 'A distinct conversation event',
      };
      if (role === 'activity') separator.activity = { status: 'completed', tools: [] };
      if (role === 'agent')
        separator.agent = {
          id: 'worker',
          toolCallId: 'worker-call',
          name: 'worker',
          displayName: 'Worker',
          description: '',
          status: 'completed',
        };
      const markup = renderToStaticMarkup(
        <Chat
          {...props}
          conversation={{
            ...emptyConversation,
            messages: [
              user,
              { ...reasoning, reasoning: { status: 'completed' } },
              separator,
              {
                ...reasoning,
                id: 'thinking-after-separator',
                content: 'Thinking after the event',
                reasoning: { status: 'completed' },
              },
            ],
          }}
        />,
      );
      expect(markup.match(/class="thinking-message completed"/g)).toHaveLength(2);
      expect(markup).not.toContain(`${reasoning.content}\n\nThinking after the event`);
    },
  );

  it.each([
    { name: 'an assistant stream', stream: 'The answer is arriving', permissions: [] },
    {
      name: 'a permission request',
      stream: '',
      permissions: [{ requestId: 'permission-1', kind: 'write', description: 'Write the file?' }],
    },
  ])('leaves completed thinking settled during $name', ({ stream, permissions }) => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          status: 'running',
          stream,
          permissions,
          messages: [user, { ...reasoning, reasoning: { status: 'completed' } }],
        }}
      />,
    );
    expect(markup.match(/class="thinking-message /g)).toHaveLength(1);
    expect(markup).toContain('class="thinking-message completed"');
    expect(markup).not.toContain('class="thinking-message running"');
    expect(markup).toContain(stream || 'Write the file?');
  });

  it('keeps interleaved worker thinking out of the combined main disclosure', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        conversation={{
          ...emptyConversation,
          messages: [
            user,
            { ...reasoning, reasoning: { status: 'completed' } },
            {
              ...reasoning,
              id: 'worker-thinking',
              agentId: 'worker',
              content: 'Private worker thinking',
            },
            {
              ...reasoning,
              id: 'thinking-2',
              content: 'Checking the callers',
              reasoning: { status: 'completed' },
            },
          ],
        }}
      />,
    );
    expect(markup.match(/class="thinking-message completed"/g)).toHaveLength(1);
    expect(markup).toContain(`${reasoning.content}\n\nChecking the callers`);
    expect(markup).not.toContain('Private worker thinking');
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

  it.each(['low', 'medium', 'high', 'max'] as const)(
    'renders the saved %s effort independently of the provider default',
    (effort) => {
      const markup = renderToStaticMarkup(
        <Chat {...reasoningProps} session={{ ...session, reasoningEffort: effort }} />,
      );
      expect(reasoningControl(markup)).toContain(
        `Reasoning effort: ${effort[0]?.toUpperCase()}${effort.slice(1)}`,
      );
    },
  );

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
    expect(modeControl(uncertain)).toBeUndefined();
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
  it.each([undefined, session])(
    'uses full access without a permission selector or first-message prompt',
    (currentSession) => {
      const markup = renderToStaticMarkup(<Chat {...props} session={currentSession} />);
      expect(modeControl(markup)).toBeUndefined();
      expect(markup).not.toContain('Confirm work permissions');
      expect(markup).not.toContain('Ask each time');
      expect(markup).toContain(
        'Full access: automatically approves file access, commands, and all other tool requests.',
      );
    },
  );

  it.each([
    { status: 'idle' as const, text: 'Full access will apply to your next message.' },
    { status: 'running' as const, text: 'Approvals remain enabled for this task.' },
  ])('accurately describes an explicit ask session while $status', ({ status, text }) => {
    const markup = renderToStaticMarkup(
      <Chat {...props} session={{ ...session, permissionMode: 'ask', status }} />,
    );
    expect(modeControl(markup)).toBeUndefined();
    expect(markup).toContain(text);
    expect(markup).not.toContain('Full access: automatically approves');
  });

  it('keeps Chat within its directory-free scope', () => {
    const markup = renderToStaticMarkup(<Chat {...props} project={undefined} />);
    expect(modeControl(markup)).toBeUndefined();
    expect(markup).not.toContain('Full access:');
    expect(markup).toContain(
      'Chat can read shared attachments, with no directory or terminal access.',
    );
  });

  it('keeps failed permission updates recoverable', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        session={session}
        permissionModeError="Could not confirm current tool permissions"
      />,
    );
    expect(modeControl(markup)).toBeUndefined();
    expect(markup).toContain('Confirm permissions again');
    expect(markup).toContain('Confirm permissions before sending a task.');
  });
});

describe('conversation language', () => {
  it('renders Chinese conversation and Expert team mode after changing locale', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <Chat
        {...props}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
      />,
    );
    expect(markup).toContain('开始处理项目任务');
    expect(markup).toContain('在 Project 中查看文件、解决问题或修改代码。');
    expect(markup).toContain('专家团模式');
    expect(markup).not.toContain('aria-label="工具权限：');
    expect(markup).toContain('aria-label="模型：Model"');
    expect(markup).toContain('描述你想完成的任务…');
    expect(markup).not.toContain('Let’s work on your project');
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
    expect(markup).toContain('Viewing file: Chat.tsx');
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

describe('remote environment preparation', () => {
  const pending: Message = {
    id: 'pending:first-message',
    sessionId: session.id,
    role: 'user',
    content: 'Inspect this project',
    createdAt: session.createdAt,
    attachments: [],
  };
  const preparingProps = {
    ...props,
    session: { ...session, status: 'running' as const },
    project: {
      ...project,
      connection: {
        kind: 'ssh' as const,
        host: 'host',
        port: null,
        user: null,
        identityFile: null,
      },
    },
    providers: [provider],
    selection: { providerId: provider.id, modelId: session.modelId },
    conversation: {
      ...emptyConversation,
      preparing: true,
      status: 'running' as const,
      messages: [pending],
    },
  };

  it('shows preparation instead of thinking and keeps the message pending with cancellation available', () => {
    const markup = renderToStaticMarkup(<Chat {...preparingProps} />);
    expect(markup).toContain(
      'Preparing the remote environment. First-time setup may take around 10 seconds',
    );
    expect(markup).toContain('Not sent yet');
    expect(markup).toContain('aria-label="Cancel environment preparation"');
    expect(markup).not.toContain('thinking-message');
    expect(modelControl(markup)).toContain('disabled');
  });

  it('renders preparation after reconnect even when no local pending message exists', () => {
    const markup = renderToStaticMarkup(
      <Chat {...preparingProps} conversation={{ ...preparingProps.conversation, messages: [] }} />,
    );
    expect(markup).toContain('Preparing the remote environment');
    expect(markup).not.toContain('Not sent yet');
  });

  it('localizes preparation and does not promise an exact setup duration', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(<Chat {...preparingProps} />);
    expect(markup).toContain('正在准备远端环境，首次使用可能需要10秒左右');
    expect(markup).toContain('尚未发送');
    setLocale('en');
  });

  it('removes the stage and pending label only after message confirmation', () => {
    const markup = renderToStaticMarkup(
      <Chat
        {...preparingProps}
        conversation={{
          ...preparingProps.conversation,
          preparing: false,
          messages: [{ ...pending, id: 'first-message' }],
        }}
      />,
    );
    expect(markup).not.toContain('Preparing the remote environment');
    expect(markup).not.toContain('Not sent yet');
    expect(markup).toContain('aria-label="Stop generating"');
  });
});
