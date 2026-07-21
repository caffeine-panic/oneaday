import { useCallback, useRef, useState } from "react";
import { OperationTracker } from "./operationTracker";

export function useRegistryOperations<Scope extends string>(
  createId: () => string,
  cancelRemote: (operationId: string) => Promise<unknown>,
) {
  const tracker = useRef<OperationTracker<Scope> | null>(null);
  if (tracker.current === null)
    tracker.current = new OperationTracker(createId);
  const [active, setActive] = useState<Partial<Record<Scope, string>>>({});

  const start = useCallback(
    (scope: Scope) => {
      const previous = tracker.current!.current(scope);
      const operationId = tracker.current!.start(scope);
      setActive(tracker.current!.snapshot());
      if (previous) void cancelRemote(previous).catch(() => undefined);
      return operationId;
    },
    [cancelRemote],
  );

  const finish = useCallback((scope: Scope, operationId: string) => {
    if (tracker.current!.finish(scope, operationId)) {
      setActive(tracker.current!.snapshot());
    }
  }, []);

  const run = useCallback(
    async <T>(
      scope: Scope,
      operation: (operationId: string) => Promise<T>,
    ): Promise<T> => {
      const operationId = start(scope);
      try {
        return await operation(operationId);
      } finally {
        finish(scope, operationId);
      }
    },
    [finish, start],
  );

  const cancel = useCallback(
    async (scope: Scope) => {
      const operationId = tracker.current!.invalidate(scope);
      if (!operationId) return false;
      setActive(tracker.current!.snapshot());
      await cancelRemote(operationId);
      return true;
    },
    [cancelRemote],
  );

  const isCurrent = useCallback(
    (scope: Scope, operationId: string) =>
      tracker.current!.isCurrent(scope, operationId),
    [],
  );

  return { active, start, finish, run, cancel, isCurrent };
}
