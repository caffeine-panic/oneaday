import { Channel, invoke } from "@tauri-apps/api/core";

export type AdapterId = "etcd" | "zookeeper" | "nacos";
export type NacosApiVersion = "v2" | "v3";
export type ConnectionEnvironment =
  | "unspecified"
  | "development"
  | "testing"
  | "staging"
  | "production";
export type AuthenticationMode = "none" | "usernamePassword" | "digest" | "custom";

export type AdapterDescriptor = {
  id: AdapterId;
  status: "available";
  capabilities: Array<"probe" | "browse" | "search" | "read" | "watch" | "create" | "update" | "delete" | "history">;
};

export type ConnectionProfile = {
  id: string;
  name: string;
  adapter: AdapterId;
  endpoint: string;
  namespace: string;
  nacosApiVersion: NacosApiVersion;
  environment: ConnectionEnvironment;
  auth: {
    mode: AuthenticationMode;
    username: string;
    customKey: string;
  };
  tls: {
    enabled: boolean;
    caCertificatePath: string;
    clientCertificatePath: string;
    clientKeyPath: string;
    serverName: string;
  };
};

export type CredentialUpdate = {
  operation: "preserve";
} | {
  operation: "replace";
  secret: string;
} | {
  operation: "clear";
};

export const connectionEnvironmentLabels: Record<ConnectionEnvironment, string> = {
  unspecified: "未指定",
  development: "开发",
  testing: "测试",
  staging: "预发",
  production: "生产",
};

export type ConnectionSession = {
  id: string;
  name: string;
  adapter: AdapterId;
  endpoint: string;
};

export type ConnectionProbe = {
  adapter: AdapterId;
  endpoint: string;
};

export type ResourceAddress =
  | { type: "root" }
  | { type: "etcd"; keyBase64: string }
  | { type: "etcdPrefix"; prefixBase64: string }
  | { type: "zookeeper"; path: string }
  | { type: "nacosConfig"; group: string; dataId: string };

export type ResourceNode = {
  address: ResourceAddress;
  name: string;
  readable: boolean;
  hasChildren: boolean | null;
};

export type ResourcePage = {
  parent: ResourceAddress;
  items: ResourceNode[];
  nextCursor?: string;
};

export type ResourceSearchPage = {
  scope: ResourceAddress;
  items: ResourceNode[];
  nextCursor?: string;
  scanned: number;
  exhaustive: boolean;
};

export type ResourceHistoryEntry = {
  revisionId: string;
  address: ResourceAddress;
  md5?: string;
  operation?: string;
  sourceUser?: string;
  sourceIp?: string;
  createdAt?: string;
  modifiedAt?: string;
  publishType?: string;
  contentType?: string;
};

export type ResourceHistoryPage = {
  address: ResourceAddress;
  items: ResourceHistoryEntry[];
  nextCursor?: string;
};

export type ResourceHistoryDocument = {
  entry: ResourceHistoryEntry;
  value: ResourceDocument["value"];
};

export type ResourceDocument = {
  address: ResourceAddress;
  name: string;
  value: {
    content: string;
    encoding: "utf8" | "base64";
    sizeBytes: number;
  };
  contentType?: string;
  version?: string;
  metadata: Record<string, string>;
};

export type RegistryError = {
  code: string;
  message: string;
  retryable: boolean;
};

export type MutationValue = {
  content: string;
  encoding: "utf8" | "base64";
};

export type ResourceMutation =
  | {
      operation: "create";
      address: ResourceAddress;
      value: MutationValue;
      contentType?: string;
    }
  | {
      operation: "update";
      address: ResourceAddress;
      value: MutationValue;
      contentType?: string;
      expectedVersion: string;
    }
  | {
      operation: "delete";
      address: ResourceAddress;
      expectedVersion: string;
    };

export type ResourceSnapshot = {
  version?: string;
  sha256: string;
  sizeBytes: number;
  encoding: "utf8" | "base64";
};

export type MutationResult = {
  operation: "create" | "update" | "delete";
  address: ResourceAddress;
  previous?: ResourceSnapshot;
  current?: ResourceSnapshot;
  consistency: "atomic" | "checkedBeforeMutation";
};

export type ExportReceipt = {
  fileName: string;
  includeValue: boolean;
  snapshot: ResourceSnapshot;
};

export type ImportAction = "create" | "update" | "skippedNoValue";

export type ImportPreviewItem = {
  address: ResourceAddress;
  name: string;
  action: ImportAction;
  sizeBytes: number;
  sha256: string;
};

export type ImportPreview = {
  planId: string;
  fileName: string;
  resources: ImportPreviewItem[];
  creates: number;
  updates: number;
  skipped: number;
  expiresInSeconds: number;
};

export type ImportApplyResult = {
  applied: Array<{
    item: ImportPreviewItem;
    consistency: "atomic" | "checkedBeforeMutation";
  }>;
  failed?: {
    item: ImportPreviewItem;
    error: RegistryError;
  };
  remaining: number;
};

export type AuditHistoryKind = "started" | "applied" | "failed" | "outcomeUnknown";

export type AuditHistoryItem = {
  kind: AuditHistoryKind;
  timestampMs: number;
  connectionId: string;
  operationId: string;
  operation?: "create" | "update" | "delete";
  address?: ResourceAddress;
  expectedVersion?: string;
  previous?: ResourceSnapshot;
  current?: ResourceSnapshot;
  consistency?: "atomic" | "checkedBeforeMutation";
  errorCode?: string;
};

export type AuditHistoryPage = {
  items: AuditHistoryItem[];
  nextCursor?: string;
  scannedBytes: number;
  exhaustive: boolean;
};

export type WatchStatusState =
  | "starting"
  | "live"
  | "reconnecting"
  | "compacted"
  | "sessionExpired"
  | "stopped"
  | "failed";

export type WatchEvent =
  | {
      kind: "status";
      subscriptionId: string;
      state: WatchStatusState;
      message?: string;
      retryInMs?: number;
    }
  | {
      kind: "change";
      subscriptionId: string;
      change: "created" | "updated" | "deleted" | "childrenChanged";
      address: ResourceAddress;
      version?: string;
    };

export type WatchHandle = {
  subscriptionId: string;
  // Retaining the channel keeps the JavaScript callback alive for the subscription lifetime.
  channel: Channel<WatchEvent>;
};

export const ROOT_ADDRESS: ResourceAddress = { type: "root" };

export function registryCapabilities() {
  return invoke<AdapterDescriptor[]>("registry_capabilities");
}

export function loadConnectionProfiles() {
  return invoke<ConnectionProfile[]>("load_connection_profiles");
}

export function upsertConnectionProfile(
  profile: ConnectionProfile,
  credentialUpdate: CredentialUpdate,
) {
  return invoke<ConnectionProfile[]>("upsert_connection_profile", { profile, credentialUpdate });
}

export function deleteConnectionProfile(connectionId: string) {
  return invoke<ConnectionProfile[]>("delete_connection_profile", { connectionId });
}

export function probeConnection(
  profile: ConnectionProfile,
  operationId: string,
  secret?: string,
) {
  return invoke<ConnectionProbe>("probe_connection", {
    profile,
    operationId,
    transientCredential: secret === undefined ? null : { secret },
  });
}

export function openConnection(
  profile: ConnectionProfile,
  operationId: string,
  secret?: string,
) {
  return invoke<ConnectionSession>("open_connection", {
    profile,
    operationId,
    transientCredential: secret === undefined ? null : { secret },
  });
}

export function closeConnection(connectionId: string) {
  return invoke<void>("close_connection", { connectionId });
}

export function listResources(
  connectionId: string,
  parent: ResourceAddress,
  operationId: string,
  cursor?: string,
) {
  return invoke<ResourcePage>("list_resources", {
    request: {
      connectionId,
      operationId,
      page: { parent, cursor, limit: 100 },
    },
  });
}

export function readResource(
  connectionId: string,
  address: ResourceAddress,
  operationId: string,
) {
  return invoke<ResourceDocument>("read_resource", {
    request: { connectionId, address, operationId },
  });
}

export function searchResources(
  connectionId: string,
  scope: ResourceAddress,
  query: string,
  operationId: string,
  cursor?: string,
) {
  return invoke<ResourceSearchPage>("search_resources", {
    request: {
      connectionId,
      operationId,
      search: { scope, query, cursor, limit: 100 },
    },
  });
}

export function listResourceHistory(
  connectionId: string,
  address: ResourceAddress,
  operationId: string,
  cursor?: string,
) {
  return invoke<ResourceHistoryPage>("list_resource_history", {
    request: {
      connectionId,
      operationId,
      history: { address, cursor, limit: 50 },
    },
  });
}

export function readResourceHistory(
  connectionId: string,
  address: ResourceAddress,
  revisionId: string,
  operationId: string,
) {
  return invoke<ResourceHistoryDocument>("read_resource_history", {
    request: { connectionId, address, revisionId, operationId },
  });
}

export function mutateResource(
  connectionId: string,
  mutation: ResourceMutation,
  operationId: string,
) {
  return invoke<MutationResult>("mutate_resource", {
    request: { connectionId, mutation, operationId },
  });
}

export function exportResource(
  connectionId: string,
  address: ResourceAddress,
  includeValue: boolean,
) {
  return invoke<ExportReceipt | null>("export_resource", {
    request: { connectionId, address, includeValue },
  });
}

export function chooseImport(connectionId: string) {
  return invoke<ImportPreview | null>("choose_import", {
    request: { connectionId },
  });
}

export function applyImport(
  connectionId: string,
  planId: string,
  operationId: string,
) {
  return invoke<ImportApplyResult>("apply_import", {
    request: { connectionId, planId, operationId, confirmed: true },
  });
}

export function loadAuditHistory(connectionId?: string, cursor?: string) {
  return invoke<AuditHistoryPage>("load_audit_history", {
    request: { connectionId, cursor, limit: 50 },
  });
}

export function cancelOperation(operationId: string) {
  return invoke<boolean>("cancel_operation", { operationId });
}

export async function startWatch(
  connectionId: string,
  subscriptionId: string,
  address: ResourceAddress,
  onEvent: (event: WatchEvent) => void,
  startVersion?: string,
): Promise<WatchHandle> {
  const channel = new Channel<WatchEvent>(onEvent);
  await invoke<void>("start_watch", {
    request: {
      connectionId,
      subscriptionId,
      watch: { address, startVersion },
    },
    onEvent: channel,
  });
  return { subscriptionId, channel };
}

export function stopWatch(subscriptionId: string) {
  return invoke<boolean>("stop_watch", { subscriptionId });
}

export function errorMessage(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason && typeof reason === "object" && "message" in reason) {
    return String((reason as RegistryError).message);
  }
  return String(reason);
}

export function isCancelled(reason: unknown): boolean {
  return Boolean(reason && typeof reason === "object" && "code" in reason && reason.code === "cancelled");
}

export function isOutcomeUnknown(reason: unknown): boolean {
  return Boolean(
    reason && typeof reason === "object" && "code" in reason && reason.code === "outcomeUnknown",
  );
}

export function isNotFound(reason: unknown): boolean {
  return Boolean(reason && typeof reason === "object" && "code" in reason && reason.code === "notFound");
}

export function newConnectionId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `connection-${Date.now()}`;
}
