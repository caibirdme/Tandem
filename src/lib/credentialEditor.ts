import type { CredentialEdit } from './settingsForm';

export interface CredentialEditorState {
  value: string;
  modified: boolean;
  visible: boolean;
  removing: boolean;
  loadingRequest: number | null;
  error: string | null;
}

export type CredentialEditorAction =
  | { type: 'loadStarted'; request: number }
  | { type: 'loaded'; request: number; value: string | null }
  | { type: 'loadFailed'; request: number; error: string }
  | { type: 'edited'; value: string }
  | { type: 'visibilityToggled' }
  | { type: 'removalToggled' }
  | { type: 'saved'; submitted: CredentialEditorState };

export function initialCredentialEditor(hasSavedKey: boolean): CredentialEditorState {
  return {
    value: '',
    modified: false,
    visible: false,
    removing: false,
    loadingRequest: hasSavedKey ? 0 : null,
    error: null,
  };
}

export function reduceCredentialEditor(
  state: CredentialEditorState,
  action: CredentialEditorAction,
): CredentialEditorState {
  switch (action.type) {
    case 'loadStarted':
      return { ...state, loadingRequest: action.request, error: null, visible: false };
    case 'loaded':
      if (state.loadingRequest !== action.request) return state;
      return {
        ...initialCredentialEditor(false),
        value: action.value ?? '',
        removing: state.removing,
      };
    case 'loadFailed':
      if (state.loadingRequest !== action.request) return state;
      return { ...state, loadingRequest: null, error: action.error };
    case 'edited':
      // A manual edit owns the value even if an older read is still in flight.
      return {
        ...state,
        value: action.value,
        modified: true,
        removing: false,
        loadingRequest: null,
        error: null,
      };
    case 'visibilityToggled':
      return { ...state, visible: !state.visible };
    case 'removalToggled':
      return {
        ...state,
        removing: !state.removing,
        visible: false,
      };
    case 'saved': {
      // A read can finish during removal without changing its save intent.
      // Never reset a newer replacement or a canceled removal.
      if (state !== action.submitted && !(state.removing && action.submitted.removing))
        return state;
      const edit = credentialEdit(state);
      return {
        ...state,
        value:
          edit.kind === 'remove' ? '' : edit.kind === 'replace' ? edit.value.trim() : state.value,
        modified: false,
        removing: false,
        visible: false,
        ...(edit.kind === 'remove' ? { loadingRequest: null, error: null } : {}),
      };
    }
  }
}

export function credentialEdit(state: CredentialEditorState): CredentialEdit {
  if (state.removing) return { kind: 'remove' };
  if (state.modified && state.value.trim()) return { kind: 'replace', value: state.value };
  return { kind: 'keep' };
}
