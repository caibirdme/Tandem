import { translate as t } from '../i18n';
interface Dependencies<Value, Input> {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  save: (sessionId: string, value: Input) => Promise<Value>;
  read: (sessionId: string) => Promise<Value>;
  started: (sessionId: string) => void;
  confirmed: (sessionId: string, value: Value, notice?: string) => void;
  unconfirmed: (sessionId: string, error: string) => void;
  uncertainMessage: string;
  timeoutMessage: string;
  saveFailure: (reason: unknown) => string;
  readFailure: (reason: unknown, failure?: string) => string;
  recovered: (value: Value, failure: string) => string;
}

/** A setting with a lost save response stays locked until its server value is known. */
export function createSessionSettingChanges<Value, Input = Value>(
  dependencies: Dependencies<Value, Input>,
) {
  const { getRequests, advance } = dependencies;
  const states = new Map<string, { status: 'pending' | 'uncertain' | 'deleted'; token: symbol }>();

  function assertCanSend(sessionId: string) {
    const status = states.get(sessionId)?.status;
    if (status === 'deleted') throw new Error(t('error.sessionDeleted'));
    if (status === 'uncertain') throw new Error(dependencies.uncertainMessage);
    if (getRequests().has(sessionId)) throw new Error(t('error.sessionBusy'));
  }

  function start(sessionId: string) {
    const token = Symbol();
    states.set(sessionId, { status: 'pending', token });
    getRequests().add(sessionId);
    advance(sessionId);
    dependencies.started(sessionId);
    return token;
  }

  function isCurrent(sessionId: string, token: symbol) {
    return states.get(sessionId)?.token === token;
  }

  function confirmed(
    sessionId: string,
    value: Value,
    token: symbol,
    notice?: string,
  ): Value | undefined {
    if (!isCurrent(sessionId, token)) return;
    dependencies.confirmed(sessionId, value, notice);
    states.delete(sessionId);
    getRequests().delete(sessionId);
    advance(sessionId);
    return value;
  }

  async function readAndConfirm(
    sessionId: string,
    token: symbol,
    failure?: string,
  ): Promise<Value | undefined> {
    if (!isCurrent(sessionId, token)) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const value = await Promise.race([
        dependencies.read(sessionId),
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error(dependencies.timeoutMessage)), 15_000);
        }),
      ]);
      return confirmed(
        sessionId,
        value,
        token,
        failure ? dependencies.recovered(value, failure) : undefined,
      );
    } catch (reason) {
      if (!isCurrent(sessionId, token)) return;
      states.set(sessionId, { status: 'uncertain', token });
      advance(sessionId);
      dependencies.unconfirmed(sessionId, dependencies.readFailure(reason, failure));
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
    async change(sessionId: string, value: Input): Promise<Value | undefined> {
      assertCanSend(sessionId);
      const token = start(sessionId);
      try {
        return confirmed(sessionId, await dependencies.save(sessionId, value), token);
      } catch (reason) {
        return readAndConfirm(sessionId, token, dependencies.saveFailure(reason));
      }
    },
    async confirm(sessionId: string): Promise<Value | undefined> {
      if (states.get(sessionId)?.status !== 'uncertain') return;
      const token = start(sessionId);
      return readAndConfirm(sessionId, token);
    },
  };
}
