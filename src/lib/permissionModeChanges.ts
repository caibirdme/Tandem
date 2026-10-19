import { errorMessage } from './api';
import type { PermissionMode } from './types';

export type PermissionModeChangeEvent =
  | { type: 'permissionModeChangeStarted'; sessionId: string }
  | {
      type: 'permissionModeChanged';
      sessionId: string;
      permissionMode: PermissionMode;
      notice?: string;
    }
  | { type: 'permissionModeUnconfirmed'; sessionId: string; error: string };

interface Dependencies {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  update: (event: PermissionModeChangeEvent) => void;
  save: (sessionId: string, mode: PermissionMode) => Promise<PermissionMode>;
  read: (sessionId: string) => Promise<PermissionMode>;
}

/** A lost save response cannot release the send lock until the server's mode is known. */
export function createPermissionModeChanges(dependencies: Dependencies) {
  const { getRequests, advance, update, save, read } = dependencies;
  const states = new Map<string, { status: 'pending' | 'uncertain' | 'deleted'; token: symbol }>();

  function assertCanSend(sessionId: string) {
    const status = states.get(sessionId)?.status;
    if (status === 'deleted') throw new Error('此对话已删除。');
    if (status === 'uncertain') throw new Error('工具权限尚未确认，请先重新确认权限。');
    if (getRequests().has(sessionId)) throw new Error('此对话正在保存设置或发送消息，请稍后再试。');
  }

  function start(sessionId: string) {
    const token = Symbol();
    states.set(sessionId, { status: 'pending', token });
    getRequests().add(sessionId);
    advance(sessionId);
    update({ type: 'permissionModeChangeStarted', sessionId });
    return token;
  }

  function isCurrent(sessionId: string, token: symbol) {
    return states.get(sessionId)?.token === token;
  }

  function confirmed(
    sessionId: string,
    permissionMode: PermissionMode,
    token: symbol,
    notice?: string,
  ) {
    if (!isCurrent(sessionId, token)) return;
    update({
      type: 'permissionModeChanged',
      sessionId,
      permissionMode,
      ...(notice ? { notice } : {}),
    });
    states.delete(sessionId);
    getRequests().delete(sessionId);
    advance(sessionId);
  }

  async function readAndConfirm(sessionId: string, token: symbol, failure?: string) {
    if (!isCurrent(sessionId, token)) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const permissionMode = await Promise.race([
        read(sessionId),
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error('权限确认超时，请重试。')), 15_000);
        }),
      ]);
      const label = permissionMode === 'allow-all' ? '完全授权' : '逐次询问';
      confirmed(
        sessionId,
        permissionMode,
        token,
        failure ? `${failure} 已重新确认当前权限为「${label}」。` : undefined,
      );
    } catch (reason) {
      if (!isCurrent(sessionId, token)) return;
      states.set(sessionId, { status: 'uncertain', token });
      advance(sessionId);
      update({
        type: 'permissionModeUnconfirmed',
        sessionId,
        error: `${failure ? `${failure} ` : ''}无法确认当前工具权限：${errorMessage(reason)}。确认前暂不能发送或切换权限。`,
      });
    } finally {
      clearTimeout(timer);
    }
  }

  return {
    assertCanSend,
    /** Invalidate in-flight results as well as the persistent uncertainty lock. */
    forget(sessionId: string) {
      states.set(sessionId, { status: 'deleted', token: Symbol() });
      getRequests().delete(sessionId);
      advance(sessionId);
    },
    async change(sessionId: string, permissionMode: PermissionMode) {
      assertCanSend(sessionId);
      const token = start(sessionId);
      try {
        confirmed(sessionId, await save(sessionId, permissionMode), token);
      } catch (reason) {
        await readAndConfirm(sessionId, token, `权限更新未获确认：${errorMessage(reason)}。`);
      }
    },
    async confirm(sessionId: string) {
      if (states.get(sessionId)?.status !== 'uncertain') return;
      const token = start(sessionId);
      await readAndConfirm(sessionId, token);
    },
  };
}
