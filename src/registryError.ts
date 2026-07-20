import type { RegistryError } from "./generated/RegistryError";
import type { RegistryErrorCode } from "./generated/RegistryErrorCode";

export type { RegistryError, RegistryErrorCode };

const registryErrorCodes = new Set<RegistryErrorCode>([
  "validation", "notConnected", "unsupported", "notFound", "network",
  "invalidResponse", "timeout", "valueTooLarge", "conflict", "outcomeUnknown",
  "permissionDenied", "resourceExhausted", "auditIncomplete", "credentialMissing",
  "credentialStore", "tlsConfiguration", "storage", "cancelled",
]);

export function isRegistryError(
  reason: unknown,
  code?: RegistryErrorCode,
): reason is RegistryError {
  if (!reason || typeof reason !== "object") return false;
  if (!("code" in reason) || typeof reason.code !== "string") return false;
  if (!("message" in reason) || typeof reason.message !== "string") return false;
  if (!("retryable" in reason) || typeof reason.retryable !== "boolean") return false;
  return registryErrorCodes.has(reason.code as RegistryErrorCode)
    && (code === undefined || reason.code === code);
}

export function registryErrorMessage(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason && typeof reason === "object" && "message" in reason) {
    return String(reason.message);
  }
  return String(reason);
}

export type MutationFailureRecovery = "unknownOutcome" | "conflict" | "report";

export function mutationFailureRecovery(reason: unknown): MutationFailureRecovery {
  if (isRegistryError(reason, "outcomeUnknown") || isRegistryError(reason, "auditIncomplete")) {
    return "unknownOutcome";
  }
  if (isRegistryError(reason, "conflict")) return "conflict";
  return "report";
}
