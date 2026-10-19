import type { ComponentProps } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { emptyConversation } from '../lib/conversation';
import type { Project, Session } from '../lib/types';
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

describe('work permission controls', () => {
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
