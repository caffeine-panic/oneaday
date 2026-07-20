import { Channel, invoke } from "@tauri-apps/api/core";
import {
  isRegistryError,
  registryErrorMessage,
  type RegistryError,
} from "./registryError";

export type { RegistryError, RegistryErrorCode } from "./registryError";
export { mutationFailureRecovery } from "./registryError";
export type { AdapterId } from "./generated/AdapterId";
export type { AuthenticationMode } from "./generated/AuthenticationMode";
export type { ConnectionEnvironment } from "./generated/ConnectionEnvironment";
export type { ConnectionProfile } from "./generated/ConnectionProfile";
export type { NacosApiVersion } from "./generated/NacosApiVersion";
export type { NacosNativeAction } from "./generated/NacosNativeAction";
export type { ResourceAddress } from "./generated/ResourceAddress";
export type { WatchEvent } from "./generated/WatchEvent";
export type { WatchStatusState } from "./generated/WatchStatusState";

import type { AdapterId } from "./generated/AdapterId";
import type { ConnectionEnvironment } from "./generated/ConnectionEnvironment";
import type { ConnectionProfile } from "./generated/ConnectionProfile";
import type { NacosNativeAction } from "./generated/NacosNativeAction";
import type { ResourceAddress } from "./generated/ResourceAddress";
import type { WatchEvent } from "./generated/WatchEvent";

export type AdapterDescriptor = {
  id: AdapterId;
  status: "available";
  capabilities: Array<
    | "probe"
    | "browse"
    | "search"
    | "read"
    | "watch"
    | "create"
    | "update"
    | "delete"
    | "history"
    | "lease"
    | "transaction"
    | "acl"
    | "ephemeral"
    | "namespace"
    | "service"
    | "instance"
  >;
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

export type DiagnosticExportReceipt = {
  fileName: string;
  connectionCount: number;
};

export type AppUpdateInfo = {
  version: string;
  currentVersion: string;
  notes?: string;
  publishedAt?: string;
};

export type AppUpdateEvent =
  | { event: "started"; data: { contentLength?: number } }
  | { event: "progress"; data: { downloaded: number; contentLength?: number } }
  | { event: "finished" };

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

export type NativeResourceInfo =
  | {
      kind: "etcdLease";
      address: ResourceAddress;
      leaseId: string;
      remainingTtlSeconds: number;
      grantedTtlSeconds: number;
    }
  | {
      kind: "zookeeperAcl";
      address: ResourceAddress;
      aclVersion: number;
      entries: ZookeeperAclEntry[];
    };

export type ZookeeperAclPermission = "read" | "write" | "create" | "delete" | "admin";

export type ZookeeperAclEntry = {
  scheme: string;
  id: string;
  permissions: ZookeeperAclPermission[];
};

export type ZookeeperCreateMode =
  | "persistentSequential"
  | "ephemeral"
  | "ephemeralSequential";

export type ZookeeperNativeAction =
  | {
      action: "setAcl";
      address: ResourceAddress;
      expectedAclVersion: number;
      entries: ZookeeperAclEntry[];
    }
  | {
      action: "create";
      address: ResourceAddress;
      value: MutationValue;
      mode: ZookeeperCreateMode;
    };

export type ZookeeperNativeActionResult =
  | {
      action: "setAcl";
      address: ResourceAddress;
      previousAclVersion: number;
      currentAclVersion: number;
      previousEntries: ZookeeperAclEntry[];
      currentEntries: ZookeeperAclEntry[];
      consistency: "atomic";
    }
  | {
      action: "create";
      requestedAddress: ResourceAddress;
      address: ResourceAddress;
      mode: ZookeeperCreateMode;
      sequence?: string;
      current: ResourceSnapshot;
      consistency: "atomic";
    };

export type NacosNamespace = {
  id: string;
  name: string;
  description: string;
  configCount: number;
  fingerprint: string;
};

export type NacosService = {
  namespaceId: string;
  group: string;
  name: string;
  protectThreshold: number;
  ephemeral: boolean;
  metadata: Record<string, string>;
  fingerprint: string;
};

export type NacosServicePage = {
  items: NacosService[];
  nextCursor?: string;
};

export type NacosInstance = {
  namespaceId: string;
  group: string;
  serviceName: string;
  cluster: string;
  ip: string;
  port: number;
  weight: number;
  healthy: boolean;
  enabled: boolean;
  ephemeral: boolean;
  metadata: Record<string, string>;
  fingerprint: string;
};

export type NacosNativeOperation = NacosNativeAction["action"];

export type NacosNativeActionResult = {
  operation: NacosNativeOperation;
  target: string;
  consistency: "checkedBeforeMutation";
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

export type ValueEncoding = "utf8" | "base64";

export type MutationValue = {
  content: string;
  encoding: ValueEncoding;
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

export type EtcdTransaction = {
  mutations: ResourceMutation[];
};

export type EtcdTransactionResult = {
  revision: string;
  results: MutationResult[];
};

export type EtcdLeaseAction =
  | {
      action: "grantAndAttach";
      address: ResourceAddress;
      expectedVersion: string;
      ttlSeconds: number;
    }
  | {
      action: "attach";
      address: ResourceAddress;
      expectedVersion: string;
      leaseId: string;
    }
  | {
      action: "detach";
      address: ResourceAddress;
      expectedVersion: string;
    }
  | {
      action: "keepAlive";
      address: ResourceAddress;
      leaseId: string;
    }
  | {
      action: "revoke";
      address: ResourceAddress;
      expectedVersion: string;
      leaseId: string;
    };

export type EtcdLeaseActionResult =
  | {
      action: "grantAndAttach" | "attach";
      address: ResourceAddress;
      leaseId: string;
      remainingTtlSeconds: number;
      grantedTtlSeconds: number;
      previous: ResourceSnapshot;
      current: ResourceSnapshot;
      consistency: "atomic";
    }
  | {
      action: "detach";
      address: ResourceAddress;
      previousLeaseId: string;
      previous: ResourceSnapshot;
      current: ResourceSnapshot;
      consistency: "atomic";
    }
  | {
      action: "keepAlive";
      address: ResourceAddress;
      leaseId: string;
      remainingTtlSeconds: number;
    }
  | {
      action: "revoke";
      address: ResourceAddress;
      leaseId: string;
      previous: ResourceSnapshot;
      consistency: "checkedBeforeMutation";
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
  nativeOperation?:
    | "etcdLeaseGrantAndAttach"
    | "etcdLeaseAttach"
    | "etcdLeaseDetach"
    | "etcdLeaseKeepAlive"
    | "etcdLeaseRevoke"
    | "zookeeperAclSet"
    | "zookeeperPersistentSequentialCreate"
    | "zookeeperEphemeralCreate"
    | "zookeeperEphemeralSequentialCreate"
    | "nacosCreateNamespace"
    | "nacosUpdateNamespace"
    | "nacosDeleteNamespace"
    | "nacosCreateService"
    | "nacosUpdateService"
    | "nacosDeleteService"
    | "nacosRegisterInstance"
    | "nacosUpdateInstance"
    | "nacosDeregisterInstance";
  address?: ResourceAddress;
  nativeTarget?: string;
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

export type WatchHandle = {
  subscriptionId: string;
  // Retaining the channel keeps the JavaScript callback alive for the subscription lifetime.
  channel: Channel<WatchEvent>;
};

export const ROOT_ADDRESS: ResourceAddress = { type: "root" };

export function registryCapabilities() {
  return invoke<AdapterDescriptor[]>("registry_capabilities");
}

export function exportDiagnosticBundle() {
  return invoke<DiagnosticExportReceipt | null>("export_diagnostic_bundle");
}

export function checkForAppUpdate() {
  return invoke<AppUpdateInfo | null>("check_for_app_update");
}

export function installAppUpdate(onEvent: (event: AppUpdateEvent) => void) {
  const channel = new Channel<AppUpdateEvent>(onEvent);
  return invoke<void>("install_app_update", { onEvent: channel });
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

export function inspectNativeResource(
  connectionId: string,
  address: ResourceAddress,
  operationId: string,
) {
  return invoke<NativeResourceInfo>("inspect_native_resource", {
    request: { connectionId, address, operationId },
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

export function executeEtcdTransaction(
  connectionId: string,
  transaction: EtcdTransaction,
  operationId: string,
) {
  return invoke<EtcdTransactionResult>("execute_etcd_transaction", {
    request: { connectionId, transaction, operationId, confirmed: true },
  });
}

export function executeEtcdLeaseAction(
  connectionId: string,
  leaseAction: EtcdLeaseAction,
  operationId: string,
) {
  return invoke<EtcdLeaseActionResult>("execute_etcd_lease_action", {
    request: { connectionId, leaseAction, operationId, confirmed: true },
  });
}

export function executeZookeeperNativeAction(
  connectionId: string,
  nativeAction: ZookeeperNativeAction,
  operationId: string,
) {
  return invoke<ZookeeperNativeActionResult>("execute_zookeeper_native_action", {
    request: { connectionId, nativeAction, operationId, confirmed: true },
  });
}

export function listNacosNamespaces(connectionId: string, operationId: string) {
  return invoke<NacosNamespace[]>("list_nacos_namespaces", {
    request: { connectionId, operationId },
  });
}

export function listNacosServices(
  connectionId: string,
  group: string,
  operationId: string,
  cursor?: string,
) {
  return invoke<NacosServicePage>("list_nacos_services", {
    request: { connectionId, operationId, group, cursor, limit: 50 },
  });
}

export function readNacosService(
  connectionId: string,
  group: string,
  serviceName: string,
  operationId: string,
) {
  return invoke<NacosService>("read_nacos_service", {
    request: { connectionId, operationId, group, serviceName },
  });
}

export function listNacosInstances(
  connectionId: string,
  group: string,
  serviceName: string,
  operationId: string,
) {
  return invoke<NacosInstance[]>("list_nacos_instances", {
    request: { connectionId, operationId, group, serviceName },
  });
}

export function executeNacosNativeAction(
  connectionId: string,
  nativeAction: NacosNativeAction,
  operationId: string,
) {
  return invoke<NacosNativeActionResult>("execute_nacos_native_action", {
    request: { connectionId, nativeAction, operationId, confirmed: true },
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
  return registryErrorMessage(reason);
}

export function isCancelled(reason: unknown): boolean {
  return isRegistryError(reason, "cancelled");
}

export function isOutcomeUnknown(reason: unknown): boolean {
  return isRegistryError(reason, "outcomeUnknown");
}

export function isNotFound(reason: unknown): boolean {
  return isRegistryError(reason, "notFound");
}

export function isConflict(reason: unknown): boolean {
  return isRegistryError(reason, "conflict");
}

export function isAuditIncomplete(reason: unknown): boolean {
  return isRegistryError(reason, "auditIncomplete");
}

export function newConnectionId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `connection-${Date.now()}`;
}
