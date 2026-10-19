import type { ComponentProps } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { emptyConversation } from '../lib/conversation';
import type { Project, Provider, Session } from '../lib/types';
import { Chat } from './Chat';

const project: Project = {
  id: 'project-1',
  name: 'Project',
  path: '/project',
  connection: { kind: 'local' },
  createdAt: '2026-10-17T00:00:00Z',
};
const session: Session = {
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
      contextWindow: null,
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
  return markup.match(/<button[^>]*aria-label="工具权限：[^"]+"[^>]*>/)?.[0];
}
function modelControl(markup: string) {
  return markup.match(/<button[^>]*aria-label="模型：[^"]+"[^>]*>/)?.[0];
}

describe('conversation model controls', () => {
  const modelProps = {
    ...props,
    session,
    providers: [provider],
    selection: { providerId: provider.id, modelId: session.modelId },
  };

  it('allows choosing a model in an existing idle conversation', () => {
    const markup = renderToStaticMarkup(<Chat {...modelProps} />);
    expect(modelControl(markup)).toContain('模型：Model');
    expect(modelControl(markup)).not.toContain('disabled=""');
    expect(markup).toContain('下一条消息将使用新模型');
  });

  it.each([
    { name: 'creating a session', override: { sending: true } },
    { name: 'running', override: { session: { ...session, status: 'running' as const } } },
    { name: 'loading history', override: { historyLoading: true } },
    { name: 'saving the model', override: { modelSaving: true } },
    { name: 'model is unconfirmed', override: { modelError: '无法确认当前模型' } },
    { name: 'saving permissions', override: { permissionModeSaving: true } },
    { name: 'permissions are unconfirmed', override: { permissionModeError: '无法确认权限' } },
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
    expect(modelControl(pending)).toContain('模型：Model');
    expect(modelControl(pending)).toContain('aria-busy="true"');
    const uncertain = renderToStaticMarkup(<Chat {...modelProps} modelError="无法确认当前模型" />);
    expect(modelControl(uncertain)).toContain('模型：待确认');
    expect(modelControl(uncertain)).toContain('aria-busy="false"');
    expect(uncertain).toContain('重新确认模型');
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
        runtime={{ available: false, version: null, message: '本机运行环境尚未初始化' }}
      />,
    );
    expect(markup).toContain('本机运行环境尚未初始化');
    expect(markup).toContain('准备运行环境');
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
        runtime={{ available: false, version: null, message: '本机运行环境尚未初始化' }}
      />,
    );
    expect(markup).not.toContain('本机运行环境尚未初始化');
    expect(markup).not.toContain('准备运行环境');
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
    expect(markup.includes('重新载入对话')).toBe(expected);
  });

  it('defaults each new work view to ask and shows no permission selector in chat', () => {
    expect(modeControl(renderToStaticMarkup(<Chat {...props} />))).toContain('工具权限：逐次询问');
    expect(
      modeControl(renderToStaticMarkup(<Chat {...props} project={undefined} />)),
    ).toBeUndefined();
    expect(modeControl(renderToStaticMarkup(<Chat {...props} session={session} />))).toContain(
      '工具权限：完全授权',
    );
    expect(modeControl(renderToStaticMarkup(<Chat {...props} />))).toContain('工具权限：逐次询问');
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
    expect(modeControl(markup)).toContain('工具权限：完全授权');
    expect(modeControl(markup)).toContain('aria-busy="true"');
    expect(markup).toContain('自动批准文件读写、命令等所有工具请求');
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
    expect(modeControl(markup)).toContain('工具权限：完全授权');
  });

  it('shows an actionable uncertain state without an endless spinner', () => {
    const markup = renderToStaticMarkup(
      <Chat {...props} session={session} permissionModeError="无法确认当前工具权限" />,
    );
    expect(modeControl(markup)).toContain('工具权限：待确认');
    expect(modeControl(markup)).toContain('disabled=""');
    expect(modeControl(markup)).toContain('aria-busy="false"');
    expect(markup).toContain('重新确认权限');
    expect(markup).toContain('权限状态尚未确认，暂不能发送任务');
  });
});
