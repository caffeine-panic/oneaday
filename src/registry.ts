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

export function openConnection(profile: ConnectionProfile) {
  return invoke<ConnectionSession>("open_connection", { profile });
}

export function closeConnection(connectionId: string) {
  return invoke<void>("close_connection", { connectionId });
}

export function listResources(
  connectionId: string,
  parent: ResourceAddress,
  cursor?: string,
) {
  return invoke<ResourcePage>("list_resources", {
    request: { connectionId, parent, cursor, limit: 100 },
  });
}

export function readResource(connectionId: string, address: ResourceAddress) {
  return invoke<ResourceDocument>("read_resource", {
    request: { connectionId, address },
  });
}

export function errorMessage(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason && typeof reason === "object" && "message" in reason) {
    return String((reason as RegistryError).message);
  }
  return String(reason);
}

const STORAGE_KEY = "atlas-registry.connections.v1";

export function loadProfiles(): ConnectionProfile[] {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    return value ? (JSON.parse(value) as ConnectionProfile[]) : [];
  } catch {
    return [];
  }
}

export function saveProfiles(profiles: ConnectionProfile[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(profiles));
}

export function newConnectionId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `connection-${Date.now()}`;
}
