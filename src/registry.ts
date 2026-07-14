import { invoke } from "@tauri-apps/api/core";

export type AdapterId = "etcd" | "zookeeper" | "nacos";
export type NacosApiVersion = "v2" | "v3";

export type AdapterDescriptor = {
  id: AdapterId;
  status: "available";
  capabilities: Array<"probe" | "browse" | "read">;
};

export type ConnectionProfile = {
  id: string;
  name: string;
  adapter: AdapterId;
  endpoint: string;
  namespace: string;
  nacosApiVersion: NacosApiVersion;
};

export type ConnectionSession = {
  id: string;
  name: string;
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

export const ROOT_ADDRESS: ResourceAddress = { type: "root" };

export function registryCapabilities() {
  return invoke<AdapterDescriptor[]>("registry_capabilities");
}

export function loadConnectionProfiles() {
  return invoke<ConnectionProfile[]>("load_connection_profiles");
}

export function saveConnectionProfiles(profiles: ConnectionProfile[]) {
  return invoke<void>("save_connection_profiles", { profiles });
}

export function openConnection(profile: ConnectionProfile, operationId: string) {
  return invoke<ConnectionSession>("open_connection", { profile, operationId });
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

export function cancelOperation(operationId: string) {
  return invoke<boolean>("cancel_operation", { operationId });
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

export function newConnectionId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `connection-${Date.now()}`;
}
