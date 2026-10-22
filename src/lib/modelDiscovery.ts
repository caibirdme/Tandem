import type { DiscoveredModel } from './types';

export interface DiscoveryState {
  request: number;
  loading: boolean;
  models: DiscoveredModel[] | null;
  selected: string[];
  error: string | null;
}
export const emptyDiscovery: DiscoveryState = {
  request: 0,
  loading: false,
  models: null,
  selected: [],
  error: null,
};
export type DiscoveryAction =
  | { type: 'reset'; request: number }
  | { type: 'start'; request: number }
  | { type: 'loaded'; request: number; models: DiscoveredModel[] }
  | { type: 'failed'; request: number; error: string }
  | { type: 'toggle'; id: string }
  | { type: 'added' };

export function reduceDiscovery(state: DiscoveryState, action: DiscoveryAction): DiscoveryState {
  switch (action.type) {
    case 'reset':
      return { ...emptyDiscovery, request: action.request };
    case 'start':
      return { ...emptyDiscovery, request: action.request, loading: true };
    case 'loaded':
      if (action.request !== state.request) return state;
      return {
        ...state,
        loading: false,
        models: [...new Map(action.models.map((model) => [model.id, model])).values()],
        selected: [],
      };
    case 'failed':
      if (action.request !== state.request) return state;
      return { ...state, loading: false, error: action.error };
    case 'toggle':
      if (!state.models?.some((model) => model.id === action.id)) return state;
      return {
        ...state,
        selected: state.selected.includes(action.id)
          ? state.selected.filter((id) => id !== action.id)
          : [...state.selected, action.id],
      };
    case 'added':
      return { ...state, selected: [] };
  }
}

export function selectedDiscoveryModels(state: DiscoveryState, existingIds: string[]) {
  return (state.models ?? []).filter(
    (model) => state.selected.includes(model.id) && !existingIds.includes(model.id),
  );
}
