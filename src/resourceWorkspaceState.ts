import type { ResourceAddress, ResourceDocument } from "./registry";
import type { TreeRow } from "./resourceTree";

export type ActiveSearch = {
  scope: ResourceAddress;
  query: string;
  scanned: number;
  exhaustive: boolean;
};

export type ResourceWorkspaceState = {
  rows: TreeRow[];
  document?: ResourceDocument;
  draftValue: string;
  selectedAddress?: ResourceAddress;
  filter: string;
  resourceQuery: string;
  activeSearch?: ActiveSearch;
};

export const initialResourceWorkspaceState: ResourceWorkspaceState = {
  rows: [],
  draftValue: "",
  filter: "",
  resourceQuery: "",
};

export type ResourceWorkspaceAction =
  | { type: "rows"; update: (rows: TreeRow[]) => TreeRow[] }
  | { type: "document"; document?: ResourceDocument }
  | { type: "draft"; value: string }
  | { type: "address"; address?: ResourceAddress }
  | { type: "filter"; value: string }
  | { type: "query"; value: string }
  | {
      type: "search";
      update: (search?: ActiveSearch) => ActiveSearch | undefined;
    }
  | { type: "clearView" };

export function reduceResourceWorkspace(
  state: ResourceWorkspaceState,
  action: ResourceWorkspaceAction,
): ResourceWorkspaceState {
  switch (action.type) {
    case "rows":
      return { ...state, rows: action.update(state.rows) };
    case "document":
      return {
        ...state,
        document: action.document,
        draftValue: action.document?.value.content ?? "",
      };
    case "draft":
      return { ...state, draftValue: action.value };
    case "address":
      return { ...state, selectedAddress: action.address };
    case "filter":
      return { ...state, filter: action.value };
    case "query":
      return { ...state, resourceQuery: action.value };
    case "search":
      return { ...state, activeSearch: action.update(state.activeSearch) };
    case "clearView":
      return {
        ...state,
        rows: [],
        document: undefined,
        draftValue: "",
        selectedAddress: undefined,
        activeSearch: undefined,
      };
  }
}
