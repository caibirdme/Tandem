import { useEffect, useReducer, useState } from 'react';

import { dispatch, errorMessage } from '../lib/api';
import {
  credentialEdit,
  initialCredentialEditor,
  reduceCredentialEditor,
} from '../lib/credentialEditor';
import type { CredentialEditorState } from '../lib/credentialEditor';

/** Credentials are read only while the AgentLens editor is visible. */
export function useTelemetryCredential(active: boolean) {
  const [state, update] = useReducer(reduceCredentialEditor, true, initialCredentialEditor);
  const [request, setRequest] = useState(0);
  const [hasSavedKey, setHasSavedKey] = useState(true);
  useEffect(() => {
    if (!active) return;
    let current = true;
    void dispatch('get_telemetry_api_key', {})
      .then((value) => {
        if (!current) return;
        setHasSavedKey(value !== null && value.length > 0);
        update({ type: 'loaded', request, value });
      })
      .catch((reason: unknown) => {
        if (current) update({ type: 'loadFailed', request, error: errorMessage(reason) });
      });
    return () => {
      current = false;
    };
  }, [active, request]);

  function retry() {
    const next = request + 1;
    update({ type: 'loadStarted', request: next });
    setRequest(next);
  }

  function saved(submitted: CredentialEditorState) {
    const edit = credentialEdit(submitted);
    if (edit.kind !== 'keep') setHasSavedKey(edit.kind === 'replace');
    update({ type: 'saved', submitted });
  }

  return { state, update, retry, saved, hasSavedKey, edit: credentialEdit(state) };
}
