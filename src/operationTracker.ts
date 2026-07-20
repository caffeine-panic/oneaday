export class OperationTracker<Scope extends string = string> {
  private readonly active = new Map<Scope, string>();

  constructor(private readonly createId: () => string) {}

  start(scope: Scope): string {
    const operationId = this.createId();
    this.active.set(scope, operationId);
    return operationId;
  }

  current(scope: Scope): string | undefined {
    return this.active.get(scope);
  }

  isCurrent(scope: Scope, operationId: string): boolean {
    return this.active.get(scope) === operationId;
  }

  finish(scope: Scope, operationId: string): boolean {
    if (!this.isCurrent(scope, operationId)) return false;
    this.active.delete(scope);
    return true;
  }

  invalidate(scope: Scope): string | undefined {
    const operationId = this.active.get(scope);
    if (operationId) this.active.delete(scope);
    return operationId;
  }

  snapshot(): Partial<Record<Scope, string>> {
    return Object.fromEntries(this.active) as Partial<Record<Scope, string>>;
  }
}
