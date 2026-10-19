import { useEffect, useReducer, useState } from 'react';

import { dispatch, errorMessage } from '../lib/api';
import {
  credentialEdit,
  initialCredentialEditor,
  reduceCredentialEditor,
} from '../lib/credentialEditor';

/** Each editor instance is keyed separately and unmounted when settings close. */
export function useProviderCredential(providerId: string, hasSavedKey: boolean) {
  const [state, update] = useReducer(reduceCredentialEditor, hasSavedKey, initialCredentialEditor);
  const [request, setRequest] = useState(0);
  useEffect(() => {
    if (!providerId || !hasSavedKey) return;
    let active = true;
    void dispatch('get_provider_api_key', { id: providerId })
      .then((value) => {
        if (active) update({ type: 'loaded', request, value });
      })
      .catch((reason: unknown) => {
        if (active) update({ type: 'loadFailed', request, error: errorMessage(reason) });
      });
    return () => {
      active = false;
    };
  }, [providerId, hasSavedKey, request]);

  function retry() {
    const next = request + 1;
    update({ type: 'loadStarted', request: next });
    setRequest(next);
  }

  return { state, update, retry, edit: credentialEdit(state) };
}
