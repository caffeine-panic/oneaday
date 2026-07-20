import { useReducer } from "react";
import type { SetStateAction } from "react";
import type { ResourceAddress, ResourceDocument } from "./registry";
import type { TreeRow } from "./resourceTree";
import {
  initialResourceWorkspaceState,
  reduceResourceWorkspace,
  type ActiveSearch,
} from "./resourceWorkspaceState";

function resolve<T>(update: SetStateAction<T>, current: T): T {
  return typeof update === "function"
    ? (update as (value: T) => T)(current)
    : update;
}

export function useResourceWorkspace() {
  const [state, dispatch] = useReducer(
    reduceResourceWorkspace,
    initialResourceWorkspaceState,
  );

  return {
    state,
    clearView: () => dispatch({ type: "clearView" }),
    showDocument: (document?: ResourceDocument) => dispatch({ type: "document", document }),
    setRows: (update: SetStateAction<TreeRow[]>) => dispatch({
      type: "rows",
      update: (current) => resolve(update, current),
    }),
    setDraftValue: (value: string) => dispatch({ type: "draft", value }),
    setSelectedAddress: (address?: ResourceAddress) => dispatch({ type: "address", address }),
    setFilter: (value: string) => dispatch({ type: "filter", value }),
    setResourceQuery: (value: string) => dispatch({ type: "query", value }),
    setActiveSearch: (update: SetStateAction<ActiveSearch | undefined>) => dispatch({
      type: "search",
      update: (current) => resolve(update, current),
    }),
  };
}
