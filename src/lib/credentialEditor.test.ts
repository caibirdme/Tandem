import { describe, expect, it } from 'vitest';

import {
  credentialEdit,
  initialCredentialEditor,
  reduceCredentialEditor,
} from './credentialEditor';

const saved = reduceCredentialEditor(initialCredentialEditor(true), {
  type: 'loaded',
  request: 0,
  value: 'fixture-saved-key',
});

describe('saved provider key editing', () => {
  it('loads the actual saved value masked without requesting a credential rewrite', () => {
    expect(saved.value).toBe('fixture-saved-key');
    expect(saved.visible).toBe(false);
    expect(saved.loadingRequest).toBeNull();
    expect(credentialEdit(saved)).toEqual({ kind: 'keep' });
  });

  it('toggles visibility without changing the saved value or save intent', () => {
    const shown = reduceCredentialEditor(saved, { type: 'visibilityToggled' });
    expect(shown.visible).toBe(true);
    expect(shown.value).toBe(saved.value);
    expect(credentialEdit(shown)).toEqual({ kind: 'keep' });
    expect(reduceCredentialEditor(shown, { type: 'visibilityToggled' }).visible).toBe(false);
    const reopened = initialCredentialEditor(true);
    expect(reopened.visible).toBe(false);
    expect(reopened.value).toBe('');
  });

  it.each(['', '   ', '\t\n'])(
    'preserves the saved key when the edited input is blank %j',
    (value) => {
      const edited = reduceCredentialEditor(saved, { type: 'edited', value });
      expect(credentialEdit(edited)).toEqual({ kind: 'keep' });
    },
  );

  it.each(['loaded', 'loadFailed'] as const)(
    'ignores a late %s result after a manual replacement',
    (type) => {
      const edited = reduceCredentialEditor(initialCredentialEditor(true), {
        type: 'edited',
        value: 'fixture-manual-key',
      });
      const result = reduceCredentialEditor(
        edited,
        type === 'loaded'
          ? { type, request: 0, value: 'fixture-old-key' }
          : { type, request: 0, error: 'Old request failed' },
      );
      expect(result).toBe(edited);
      expect(result.loadingRequest).toBeNull();
      expect(credentialEdit(result)).toEqual({ kind: 'replace', value: 'fixture-manual-key' });
    },
  );

  it('requires explicit removal and lets cancellation restore the previous edit', () => {
    const edited = reduceCredentialEditor(saved, { type: 'edited', value: 'fixture-edited-key' });
    const shown = reduceCredentialEditor(edited, { type: 'visibilityToggled' });
    const removed = reduceCredentialEditor(shown, { type: 'removalToggled' });
    expect(removed.visible).toBe(false);
    expect(credentialEdit(removed)).toEqual({ kind: 'remove' });
    const cancelled = reduceCredentialEditor(removed, { type: 'removalToggled' });
    expect(credentialEdit(cancelled)).toEqual({ kind: 'replace', value: 'fixture-edited-key' });
    expect(cancelled.value).toBe('fixture-edited-key');
  });

  it('does not let a delayed read undo an explicit removal', () => {
    const removed = reduceCredentialEditor(initialCredentialEditor(true), {
      type: 'removalToggled',
    });
    expect(
      reduceCredentialEditor(removed, { type: 'loaded', request: 0, value: 'fixture-old-key' }),
    ).toBe(removed);
    expect(credentialEdit(removed)).toEqual({ kind: 'remove' });
  });

  it('keeps an unread credential unchanged and retries without accepting an older result', () => {
    const failed = reduceCredentialEditor(initialCredentialEditor(true), {
      type: 'loadFailed',
      request: 0,
      error: 'Fixture read unavailable',
    });
    expect(credentialEdit(failed)).toEqual({ kind: 'keep' });
    expect(failed.loadingRequest).toBeNull();
    const retry = reduceCredentialEditor(failed, { type: 'loadStarted', request: 1 });
    expect(retry.error).toBeNull();
    expect(
      reduceCredentialEditor(retry, { type: 'loaded', request: 0, value: 'fixture-old-key' }),
    ).toBe(retry);
    const received = reduceCredentialEditor(retry, {
      type: 'loaded',
      request: 1,
      value: 'fixture-current-key',
    });
    expect(received.value).toBe('fixture-current-key');
    expect(received.visible).toBe(false);
    expect(credentialEdit(received)).toEqual({ kind: 'keep' });
  });

  it('starts new providers empty and handles a missing saved key without deleting anything', () => {
    expect(initialCredentialEditor(false)).toMatchObject({
      value: '',
      visible: false,
      loadingRequest: null,
    });
    const missing = reduceCredentialEditor(initialCredentialEditor(true), {
      type: 'loaded',
      request: 0,
      value: null,
    });
    expect(missing.value).toBe('');
    expect(credentialEdit(missing)).toEqual({ kind: 'keep' });
  });

  it.each(['replace', 'remove'] as const)(
    'masks a successfully saved %s and returns to preserving the stored credential',
    (kind) => {
      const edited = reduceCredentialEditor(saved, { type: 'edited', value: ' fixture-new-key ' });
      const submitted =
        kind === 'remove'
          ? reduceCredentialEditor(edited, { type: 'removalToggled' })
          : { ...edited, visible: true };
      const completed = reduceCredentialEditor(submitted, { type: 'saved', submitted });
      expect(completed.value).toBe(kind === 'remove' ? '' : 'fixture-new-key');
      expect(completed.visible).toBe(false);
      expect(completed.removing).toBe(false);
      expect(credentialEdit(completed)).toEqual({ kind: 'keep' });
    },
  );

  it('preserves a key draft entered while an earlier save was in flight', () => {
    const submitted = reduceCredentialEditor(saved, {
      type: 'edited',
      value: 'fixture-submitted-key',
    });
    const newer = reduceCredentialEditor(submitted, { type: 'edited', value: 'fixture-next-key' });
    const completed = reduceCredentialEditor(newer, { type: 'saved', submitted });
    expect(completed).toBe(newer);
    expect(credentialEdit(completed)).toEqual({ kind: 'replace', value: 'fixture-next-key' });
  });

  it('keeps an in-flight credential read valid when saving preserves that credential', () => {
    const submitted = initialCredentialEditor(true);
    const completed = reduceCredentialEditor(submitted, { type: 'saved', submitted });
    const loaded = reduceCredentialEditor(completed, {
      type: 'loaded',
      request: 0,
      value: 'fixture-saved-key',
    });
    expect(loaded.value).toBe('fixture-saved-key');
    expect(credentialEdit(loaded)).toEqual({ kind: 'keep' });
  });
});
