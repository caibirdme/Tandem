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
  | { type: 'removalToggled' };

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
      return { ...initialCredentialEditor(false), value: action.value ?? '' };
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
        loadingRequest: null,
        error: null,
      };
  }
}

export function credentialEdit(state: CredentialEditorState): CredentialEdit {
  if (state.removing) return { kind: 'remove' };
  if (state.modified && state.value.trim()) return { kind: 'replace', value: state.value };
  return { kind: 'keep' };
}
