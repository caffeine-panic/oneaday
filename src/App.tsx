import { useEffect, useMemo, useRef, useState } from "react";
import {
  MutationConfirmationDialog,
  NewResourceDialog,
  type NewResourceDraft,
} from "./MutationDialogs";
import { ConnectionDialog, type ConnectionDialogMode } from "./ConnectionDialog";
import { HistoryDialog } from "./HistoryDialog";
import { NacosHistoryDialog } from "./NacosHistoryDialog";
import { NacosNativeDialog } from "./NacosNativeDialog";
import { EtcdLeaseDialog } from "./EtcdLeaseDialog";
import {
  ZookeeperAclDialog,
  ZookeeperCreateConfirmationDialog,
} from "./ZookeeperNativeDialogs";
import { ExportDialog, ImportPreviewDialog } from "./TransferDialogs";
import {
  EtcdTransactionDialog,
  emptyEtcdTransactionItem,
  type EtcdTransactionDraftItem,
} from "./EtcdTransactionDialog";
import {
  ROOT_ADDRESS,
  applyImport,
  cancelOperation,
  chooseImport,
  closeConnection,
  connectionEnvironmentLabels,
  deleteConnectionProfile,
  errorMessage,
  executeEtcdLeaseAction,
  executeEtcdTransaction,
  executeZookeeperNativeAction,
  exportDiagnosticBundle,
  exportResource,
  inspectNativeResource,
  isCancelled,
  isNotFound,
  isOutcomeUnknown,
  listResources,
  listResourceHistory,
  loadAuditHistory,
  loadConnectionProfiles,
  newConnectionId,
  mutateResource,
  openConnection,
  probeConnection,
  readResource,
  readResourceHistory,
  registryCapabilities,
  searchResources,
  startWatch,
  stopWatch,
  upsertConnectionProfile,
  type AdapterDescriptor,
  type AdapterId,
  type AuditHistoryItem,
  type ConnectionProfile,
  type ConnectionSession,
  type EtcdLeaseAction,
  type EtcdTransaction,
  type ImportPreview,
  type NativeResourceInfo,
  type ResourceAddress,
  type ResourceDocument,
  type ResourceHistoryDocument,
  type ResourceHistoryEntry,
  type ResourceNode,
  type ResourceMutation,
  type WatchEvent,
  type WatchHandle,
  type WatchStatusState,
  type ZookeeperNativeAction,
} from "./registry";

type ResourceRow = {
  kind: "resource";
  node: ResourceNode;
  depth: number;
  expanded: boolean;
};

type MoreRow = {
  kind: "more";
  parent: ResourceAddress;
  cursor: string;
  depth: number;
  search?: {
    scope: ResourceAddress;
    query: string;
  };
};

type TreeRow = ResourceRow | MoreRow;
type WatchChangeEvent = Extract<WatchEvent, { kind: "change" }>;

type ResourceWatchView = {
  subscriptionId: string;
  address: ResourceAddress;
  state: WatchStatusState;
  message?: string;
  retryInMs?: number;
  changeCount: number;
  lastChange?: WatchChangeEvent;
  remoteChanged: boolean;
};

type ActiveSearch = {
  scope: ResourceAddress;
  query: string;
  scanned: number;
  exhaustive: boolean;
};

const watchStatusLabels: Record<WatchStatusState, string> = {
  starting: "正在建立监听",
  live: "实时监听中",
  reconnecting: "连接中断，正在恢复",
  compacted: "历史事件已压缩，需要刷新",
  sessionExpired: "会话已过期，需要重新连接",
  stopped: "监听已停止",
  failed: "监听失败",
};

const watchChangeLabels: Record<WatchChangeEvent["change"], string> = {
  created: "已创建",
  updated: "已更新",
  deleted: "已删除",
  childrenChanged: "子节点已变化",
};

const emptyForm = (): ConnectionProfile => ({
  id: newConnectionId(),
  name: "",
  adapter: "etcd",
  endpoint: "127.0.0.1:2379",
  namespace: "",
  nacosApiVersion: "v2",
  environment: "unspecified",
  auth: { mode: "none", username: "", customKey: "" },
  tls: {
    enabled: false,
    caCertificatePath: "",
    clientCertificatePath: "",
    clientKeyPath: "",
    serverName: "",
  },
});

const emptyResourceDraft = (adapter: AdapterId): NewResourceDraft => ({
  keyOrPath: adapter === "zookeeper" ? "/" : "",
  group: "DEFAULT_GROUP",
  dataId: "",
  content: "",
  contentType: "text",
  zookeeperMode: "persistent",
});

function pageRows(
  items: ResourceNode[],
  depth: number,
  parent: ResourceAddress,
  nextCursor?: string,
): TreeRow[] {
  const rows: TreeRow[] = items.map((node) => ({
    kind: "resource",
    node,
    depth,
    expanded: false,
  }));
  if (nextCursor) {
    rows.push({ kind: "more", parent, cursor: nextCursor, depth });
  }
  return rows;
}

function searchPageRows(
  items: ResourceNode[],
  scope: ResourceAddress,
  query: string,
  nextCursor?: string,
): TreeRow[] {
  const rows: TreeRow[] = items.map((node) => ({
    kind: "resource",
    node,
    depth: 0,
    expanded: false,
  }));
  if (nextCursor) {
    rows.push({ kind: "more", parent: scope, cursor: nextCursor, depth: 0, search: { scope, query } });
  }
  return rows;
}

function connectionLabel(adapter: AdapterId) {
  return adapter === "zookeeper" ? "ZK" : adapter;
}

function normalizedProfile(profile: ConnectionProfile): ConnectionProfile {
  return {
    ...profile,
    name: profile.name.trim(),
    endpoint: profile.endpoint.trim(),
    namespace: profile.namespace.trim(),
    auth: {
      ...profile.auth,
      username: profile.auth.username.trim(),
      customKey: profile.auth.customKey.trim(),
    },
    tls: {
      ...profile.tls,
      caCertificatePath: profile.tls.caCertificatePath.trim(),
      clientCertificatePath: profile.tls.clientCertificatePath.trim(),
      clientKeyPath: profile.tls.clientKeyPath.trim(),
      serverName: profile.tls.serverName.trim(),
    },
  };
}

function addressLabel(address: ResourceAddress) {
  switch (address.type) {
    case "root":
      return "/";
    case "etcd":
      return "etcd key";
    case "etcdPrefix":
      return "etcd prefix";
    case "zookeeper":
      return address.path;
    case "nacosConfig":
      return `${address.group} / ${address.dataId}`;
  }
}

function sameAddress(left: ResourceAddress, right: ResourceAddress) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function utf8Base64(value: string) {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function etcdAddressFromInput(rawInput: string): Extract<ResourceAddress, { type: "etcd" }> {
  const input = rawInput.trim();
  if (!input) throw new Error("etcd transaction key 不能为空");
  if (!input.startsWith("base64:")) return { type: "etcd", keyBase64: utf8Base64(input) };
  const keyBase64 = input.slice("base64:".length).trim();
  try {
    if (!keyBase64 || atob(keyBase64).length === 0) throw new Error();
  } catch {
    throw new Error("base64: 后面的 etcd transaction key 不是有效 Base64");
  }
  return { type: "etcd", keyBase64 };
}

function locateAddress(adapter: AdapterId, rawInput: string): ResourceAddress {
  const input = rawInput.trim();
  if (!input) throw new Error("请输入要定位的资源标识");
  if (adapter === "etcd") {
    if (!input.startsWith("base64:")) return { type: "etcd", keyBase64: utf8Base64(input) };
    const keyBase64 = input.slice("base64:".length).trim();
    try {
      atob(keyBase64);
    } catch {
      throw new Error("base64: 后面的 etcd key 不是有效 Base64");
    }
    if (!keyBase64) throw new Error("etcd key 不能为空");
    return { type: "etcd", keyBase64 };
  }
  if (adapter === "zookeeper") {
    if (!input.startsWith("/") || input.includes("//") || (input.length > 1 && input.endsWith("/"))) {
      throw new Error("ZooKeeper 路径必须是规范的绝对路径");
    }
    return { type: "zookeeper", path: input };
  }
  const separator = input.indexOf(" / ");
  if (separator < 1) throw new Error("Nacos 定位格式为 GROUP / dataId");
  const group = input.slice(0, separator).trim();
  const dataId = input.slice(separator + 3).trim();
  if (!group || !dataId) throw new Error("Nacos 定位需要 group 和 dataId");
  return { type: "nacosConfig", group, dataId };
}

function searchScope(adapter: AdapterId, selected?: ResourceAddress): ResourceAddress {
  if (adapter === "etcd" && selected?.type === "etcdPrefix") return selected;
  if (adapter === "zookeeper" && selected?.type === "zookeeper") return selected;
  return ROOT_ADDRESS;
}

export function App() {
  const [capabilities, setCapabilities] = useState<AdapterDescriptor[]>();
  const [profiles, setProfiles] = useState<ConnectionProfile[]>([]);
  const [sessions, setSessions] = useState<Record<string, ConnectionSession>>({});
  const [selectedId, setSelectedId] = useState<string>();
  const [rows, setRows] = useState<TreeRow[]>([]);
  const [document, setDocument] = useState<ResourceDocument>();
  const [draftValue, setDraftValue] = useState("");
  const [selectedAddress, setSelectedAddress] = useState<ResourceAddress>();
  const [filter, setFilter] = useState("");
  const [resourceQuery, setResourceQuery] = useState("");
  const [activeSearch, setActiveSearch] = useState<ActiveSearch>();
  const [busy, setBusy] = useState(false);
  const [activeOperation, setActiveOperation] = useState<string>();
  const [message, setMessage] = useState<string>();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [dialogMode, setDialogMode] = useState<ConnectionDialogMode>("new");
  const [testingConnection, setTestingConnection] = useState(false);
  const [form, setForm] = useState<ConnectionProfile>(emptyForm);
  const [connectionSecret, setConnectionSecret] = useState("");
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [resourceDraft, setResourceDraft] = useState<NewResourceDraft>(() => emptyResourceDraft("etcd"));
  const [pendingMutation, setPendingMutation] = useState<ResourceMutation>();
  const [confirmationText, setConfirmationText] = useState("");
  const [exportDialogOpen, setExportDialogOpen] = useState(false);
  const [exportIncludeValue, setExportIncludeValue] = useState(false);
  const [importPreview, setImportPreview] = useState<ImportPreview>();
  const [importConfirmationText, setImportConfirmationText] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyScope, setHistoryScope] = useState("all");
  const [historyItems, setHistoryItems] = useState<AuditHistoryItem[]>([]);
  const [historyCursor, setHistoryCursor] = useState<string>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const [serverHistoryOpen, setServerHistoryOpen] = useState(false);
  const [serverHistoryAddress, setServerHistoryAddress] = useState<ResourceAddress>();
  const [serverHistoryItems, setServerHistoryItems] = useState<ResourceHistoryEntry[]>([]);
  const [serverHistoryCursor, setServerHistoryCursor] = useState<string>();
  const [serverHistoryDetail, setServerHistoryDetail] = useState<ResourceHistoryDocument>();
  const [serverHistoryLoading, setServerHistoryLoading] = useState(false);
  const [nativeInfoOpen, setNativeInfoOpen] = useState(false);
  const [nativeInfo, setNativeInfo] = useState<NativeResourceInfo>();
  const [nativeInfoLoading, setNativeInfoLoading] = useState(false);
  const [etcdTransactionOpen, setEtcdTransactionOpen] = useState(false);
  const [etcdTransactionItems, setEtcdTransactionItems] = useState<EtcdTransactionDraftItem[]>([]);
  const [etcdTransactionConfirmation, setEtcdTransactionConfirmation] = useState("");
  const [pendingZookeeperAction, setPendingZookeeperAction] = useState<Extract<ZookeeperNativeAction, { action: "create" }>>();
  const [zookeeperConfirmation, setZookeeperConfirmation] = useState("");
  const [nacosNativeOpen, setNacosNativeOpen] = useState(false);
  const [resourceWatch, setResourceWatch] = useState<ResourceWatchView>();
  const watchHandle = useRef<WatchHandle | undefined>(undefined);
  const watchGeneration = useRef(0);
  const historyGeneration = useRef(0);

  const selectedProfile = profiles.find((profile) => profile.id === selectedId);
  const selectedSession = selectedId ? sessions[selectedId] : undefined;

  useEffect(() => {
    registryCapabilities()
      .then(setCapabilities)
      .catch((reason: unknown) => setMessage(errorMessage(reason)));
    loadConnectionProfiles()
      .then(setProfiles)
      .catch((reason: unknown) => setMessage(errorMessage(reason)));
  }, []);

  useEffect(() => () => {
    watchGeneration.current += 1;
    const active = watchHandle.current;
    if (active) void stopWatch(active.subscriptionId);
  }, []);

  const visibleRows = useMemo(() => {
    const query = filter.trim().toLocaleLowerCase();
    if (!query) return rows;
    return rows.filter(
      (row) => row.kind === "resource" && row.node.name.toLocaleLowerCase().includes(query),
    );
  }, [filter, rows]);

  const startOperation = () => {
    const operationId = newConnectionId();
    setActiveOperation(operationId);
    return operationId;
  };

  const finishOperation = (operationId: string) => {
    setActiveOperation((current) => current === operationId ? undefined : current);
  };

  const runList = async (
    connectionId: string,
    parent: ResourceAddress,
    cursor?: string,
  ) => {
    const operationId = startOperation();
    try {
      return await listResources(connectionId, parent, operationId, cursor);
    } finally {
      finishOperation(operationId);
    }
  };

  const runRead = async (connectionId: string, address: ResourceAddress) => {
    const operationId = startOperation();
    try {
      return await readResource(connectionId, address, operationId);
    } finally {
      finishOperation(operationId);
    }
  };

  const runSearch = async (
    connectionId: string,
    scope: ResourceAddress,
    query: string,
    cursor?: string,
  ) => {
    const operationId = startOperation();
    try {
      return await searchResources(connectionId, scope, query, operationId, cursor);
    } finally {
      finishOperation(operationId);
    }
  };

  const showDocument = (nextDocument: ResourceDocument | undefined) => {
    setDocument(nextDocument);
    setDraftValue(nextDocument?.value.content ?? "");
  };

  const releaseActiveWatch = async (clearView = true) => {
    const active = watchHandle.current;
    watchHandle.current = undefined;
    if (clearView) setResourceWatch(undefined);
    if (!active) return;
    try {
      await stopWatch(active.subscriptionId);
    } catch (reason) {
      if (!clearView) setMessage(errorMessage(reason));
    }
  };

  const stopActiveWatch = async (clearView = true) => {
    watchGeneration.current += 1;
    await releaseActiveWatch(clearView);
  };

  const handleWatchEvent = (event: WatchEvent) => {
    setResourceWatch((current) => {
      if (!current || current.subscriptionId !== event.subscriptionId) return current;
      if (event.kind === "status") {
        if (event.state === "stopped" || event.state === "failed") {
          if (watchHandle.current?.subscriptionId === event.subscriptionId) {
            watchHandle.current = undefined;
          }
        }
        return {
          ...current,
          state: event.state,
          message: event.message,
          retryInMs: event.retryInMs,
          remoteChanged: event.state === "compacted" ? true : current.remoteChanged,
        };
      }
      return {
        ...current,
        changeCount: current.changeCount + 1,
        lastChange: event,
        remoteChanged: true,
      };
    });
  };

  const startResourceWatch = async () => {
    if (!selectedSession || !selectedProfile || !document || busy) return;
    const generation = watchGeneration.current + 1;
    watchGeneration.current = generation;
    await releaseActiveWatch();
    if (watchGeneration.current !== generation) return;
    const subscriptionId = newConnectionId();
    const address = document.address;
    setResourceWatch({
      subscriptionId,
      address,
      state: "starting",
      changeCount: 0,
      remoteChanged: false,
    });
    try {
      const handle = await startWatch(
        selectedSession.id,
        subscriptionId,
        address,
        handleWatchEvent,
        selectedProfile.adapter === "etcd" ? document.version : undefined,
      );
      if (watchGeneration.current !== generation) {
        await stopWatch(handle.subscriptionId);
        return;
      }
      watchHandle.current = handle;
    } catch (reason) {
      if (watchGeneration.current !== generation) return;
      setResourceWatch((current) => current?.subscriptionId === subscriptionId
        ? { ...current, state: "failed", message: errorMessage(reason) }
        : current);
      setMessage(errorMessage(reason));
    }
  };

  const stopResourceWatch = async () => {
    await stopActiveWatch(false);
    setResourceWatch((current) => current
      ? { ...current, state: "stopped", message: undefined, retryInMs: undefined }
      : current);
  };

  const refreshWatchedResource = async () => {
    if (!selectedSession || !document || busy) return;
    if (draftValue !== document.value.content
      && !globalThis.confirm("远端资源已变化。刷新会丢弃当前未保存的编辑，是否继续？")) {
      return;
    }
    setBusy(true);
    setMessage(undefined);
    try {
      showDocument(await runRead(selectedSession.id, document.address));
      setResourceWatch((current) => current ? { ...current, remoteChanged: false } : current);
      setMessage("已读取远端最新版本");
    } catch (reason) {
      if (isNotFound(reason)) {
        await stopActiveWatch();
        showDocument(undefined);
        setSelectedAddress(undefined);
        try {
          await reloadRoot(selectedSession.id);
        } catch {
          // The deletion is already known; a tree refresh failure should not restore stale content.
        }
        setMessage("远端资源已删除，已移除本地旧内容");
      } else {
        setMessage(`刷新监听资源失败：${errorMessage(reason)}`);
      }
    } finally {
      setBusy(false);
    }
  };

  const reloadRoot = async (connectionId: string) => {
    const page = await runList(connectionId, ROOT_ADDRESS);
    setRows(pageRows(page.items, 0, page.parent, page.nextCursor));
    setActiveSearch(undefined);
  };

  const cancelActiveOperation = async () => {
    if (!activeOperation) return;
    try {
      await cancelOperation(activeOperation);
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  };

  const connectAndLoad = async (profile: ConnectionProfile, transientSecret?: string) => {
    await stopActiveWatch();
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    setBusy(true);
    setMessage(undefined);
    showDocument(undefined);
    setRows([]);
    try {
      const operationId = startOperation();
      let session: ConnectionSession;
      try {
        session = await openConnection(profile, operationId, transientSecret);
      } finally {
        finishOperation(operationId);
      }
      setSessions((current) => ({ ...current, [session.id]: session }));
      setSelectedId(session.id);
      await reloadRoot(session.id);
      setMessage(`已连接 ${session.endpoint}`);
      return true;
    } catch (reason) {
      setMessage(errorMessage(reason));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const saveAndConnect = async () => {
    const candidate = normalizedProfile(form);
    if (!candidate.name || !candidate.endpoint) {
      setMessage("连接名称和 endpoint 不能为空");
      return;
    }
    if (candidate.auth.mode !== "none" && dialogMode !== "edit" && !connectionSecret) {
      setMessage("新连接启用认证时必须填写密钥");
      return;
    }
    try {
      const credentialUpdate = candidate.auth.mode === "none"
        ? { operation: "clear" as const }
        : connectionSecret
          ? { operation: "replace" as const, secret: connectionSecret }
          : { operation: "preserve" as const };
      const nextProfiles = await upsertConnectionProfile(candidate, credentialUpdate);
      setProfiles(nextProfiles);
      setDialogOpen(false);
      await connectAndLoad(candidate, connectionSecret || undefined);
      setConnectionSecret("");
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  };

  const testConnection = async () => {
    const candidate = normalizedProfile(form);
    if (!candidate.name || !candidate.endpoint) {
      setMessage("连接名称和 endpoint 不能为空");
      return;
    }
    setBusy(true);
    setTestingConnection(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const result = await probeConnection(candidate, operationId, connectionSecret || undefined);
      setMessage(`连接测试成功：${result.endpoint}`);
    } catch (reason) {
      setMessage(isCancelled(reason) ? "连接测试已取消" : errorMessage(reason));
    } finally {
      finishOperation(operationId);
      setTestingConnection(false);
      setBusy(false);
    }
  };

  const selectProfile = async (profile: ConnectionProfile) => {
    await stopActiveWatch();
    setSelectedId(profile.id);
    setRows([]);
    setActiveSearch(undefined);
    setExportDialogOpen(false);
    setImportPreview(undefined);
    setServerHistoryOpen(false);
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    showDocument(undefined);
    setSelectedAddress(undefined);
    setMessage(sessions[profile.id] ? "连接会话已打开，点击刷新加载资源" : undefined);
  };

  const refreshRoot = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    setMessage(undefined);
    try {
      await reloadRoot(selectedSession.id);
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const searchCurrentScope = async () => {
    if (!selectedSession || !selectedProfile || busy) return;
    const query = resourceQuery.trim();
    if (!query) {
      setMessage("请输入要搜索的资源标识");
      return;
    }
    const scope = searchScope(selectedProfile.adapter, selectedAddress);
    setBusy(true);
    setMessage(undefined);
    try {
      const page = await runSearch(selectedSession.id, scope, query);
      setRows(searchPageRows(page.items, page.scope, query, page.nextCursor));
      setActiveSearch({ scope: page.scope, query, scanned: page.scanned, exhaustive: page.exhaustive });
      setFilter("");
      setMessage(
        `${page.items.length} 个匹配项 · 本次检查 ${page.scanned} 个标识${page.exhaustive ? " · 已到当前范围末尾" : " · 可继续加载"}`,
      );
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const locateResource = async () => {
    if (!selectedSession || !selectedProfile || busy) return;
    let address: ResourceAddress;
    try {
      address = locateAddress(selectedProfile.adapter, resourceQuery);
    } catch (reason) {
      setMessage(errorMessage(reason));
      return;
    }
    if (document && draftValue !== document.value.content
      && !globalThis.confirm("定位其他资源会丢弃当前未保存的编辑，是否继续？")) {
      return;
    }
    if (!document || !sameAddress(document.address, address)) await stopActiveWatch();
    setBusy(true);
    setMessage(undefined);
    setSelectedAddress(address);
    try {
      showDocument(await runRead(selectedSession.id, address));
      setMessage("已精确定位并读取资源");
    } catch (reason) {
      showDocument(undefined);
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const exitSearch = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    setMessage(undefined);
    try {
      await reloadRoot(selectedSession.id);
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const openResource = async (index: number, row: ResourceRow) => {
    if (!selectedSession || busy) return;
    if (!document || !sameAddress(document.address, row.node.address)) {
      await stopActiveWatch();
    }
    setSelectedAddress(row.node.address);
    setMessage(undefined);

    if (row.node.readable) {
      setBusy(true);
      try {
        showDocument(await runRead(selectedSession.id, row.node.address));
      } catch (reason) {
        showDocument(undefined);
        setMessage(errorMessage(reason));
        if (isCancelled(reason)) return;
      } finally {
        setBusy(false);
      }
    }

    if (row.node.hasChildren === false) return;
    if (row.expanded) {
      setRows((current) => {
        const next = [...current];
        next[index] = { ...row, expanded: false };
        let end = index + 1;
        while (end < next.length && next[end].depth > row.depth) end += 1;
        next.splice(index + 1, end - index - 1);
        return next;
      });
      return;
    }

    setBusy(true);
    try {
      const page = await runList(selectedSession.id, row.node.address);
      setRows((current) => {
        const next = [...current];
        next[index] = {
          ...row,
          expanded: page.items.length > 0,
          node: {
            ...row.node,
            hasChildren: page.items.length > 0 || Boolean(page.nextCursor),
          },
        };
        next.splice(
          index + 1,
          0,
          ...pageRows(page.items, row.depth + 1, page.parent, page.nextCursor),
        );
        return next;
      });
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const loadMore = async (index: number, row: MoreRow) => {
    if (!selectedSession || busy) return;
    setBusy(true);
    try {
      if (row.search) {
        const page = await runSearch(
          selectedSession.id,
          row.search.scope,
          row.search.query,
          row.cursor,
        );
        setRows((current) => {
          const next = [...current];
          next.splice(
            index,
            1,
            ...searchPageRows(page.items, page.scope, row.search!.query, page.nextCursor),
          );
          return next;
        });
        setActiveSearch((current) => current
          ? { ...current, scanned: current.scanned + page.scanned, exhaustive: page.exhaustive }
          : current);
        return;
      }
      const page = await runList(selectedSession.id, row.parent, row.cursor);
      setRows((current) => {
        const next = [...current];
        next.splice(
          index,
          1,
          ...pageRows(page.items, row.depth, page.parent, page.nextCursor),
        );
        return next;
      });
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const openCreateResource = () => {
    if (!selectedProfile || !selectedSession || busy) return;
    setResourceDraft(emptyResourceDraft(selectedProfile.adapter));
    setCreateDialogOpen(true);
  };

  const prepareCreate = () => {
    if (!selectedProfile) return;
    const contentType = resourceDraft.contentType.trim() || undefined;
    let address: ResourceAddress;
    if (selectedProfile.adapter === "etcd") {
      if (!resourceDraft.keyOrPath.trim()) {
        setMessage("etcd key 不能为空");
        return;
      }
      address = { type: "etcd", keyBase64: utf8Base64(resourceDraft.keyOrPath) };
    } else if (selectedProfile.adapter === "zookeeper") {
      const path = resourceDraft.keyOrPath.trim();
      if (!path.startsWith("/") || path === "/" || path.endsWith("/") || path.includes("//")) {
        setMessage("ZooKeeper 路径必须是规范的绝对非根路径，且不能以 / 结尾");
        return;
      }
      address = { type: "zookeeper", path };
    } else {
      const group = resourceDraft.group.trim();
      const dataId = resourceDraft.dataId.trim();
      if (!group || !dataId || !resourceDraft.content) {
        setMessage("Nacos 创建需要 group、dataId 和非空内容");
        return;
      }
      address = { type: "nacosConfig", group, dataId };
    }
    if (selectedProfile.adapter === "zookeeper" && resourceDraft.zookeeperMode !== "persistent") {
      setPendingZookeeperAction({
        action: "create",
        address,
        value: { content: resourceDraft.content, encoding: "utf8" },
        mode: resourceDraft.zookeeperMode,
      });
      setZookeeperConfirmation("");
      setCreateDialogOpen(false);
      return;
    }
    setPendingMutation({
      operation: "create",
      address,
      value: { content: resourceDraft.content, encoding: "utf8" },
      contentType,
    });
    setConfirmationText("");
    setCreateDialogOpen(false);
  };

  const prepareUpdate = () => {
    if (!document?.version) {
      setMessage("当前资源没有可用于条件更新的版本，请先刷新");
      return;
    }
    setPendingMutation({
      operation: "update",
      address: document.address,
      value: { content: draftValue, encoding: document.value.encoding },
      contentType: document.contentType,
      expectedVersion: document.version,
    });
    setConfirmationText("");
  };

  const prepareDelete = () => {
    if (!document?.version) {
      setMessage("当前资源没有可用于条件删除的版本，请先刷新");
      return;
    }
    setPendingMutation({
      operation: "delete",
      address: document.address,
      expectedVersion: document.version,
    });
    setConfirmationText("");
  };

  const reconcileUnknownMutation = async (connectionId: string, address: ResourceAddress) => {
    try {
      await reloadRoot(connectionId);
      const remoteDocument = await runRead(connectionId, address);
      showDocument(remoteDocument);
      setSelectedAddress(address);
      return true;
    } catch {
      showDocument(undefined);
      setSelectedAddress(undefined);
      return false;
    }
  };

  const executeMutation = async () => {
    if (!selectedSession || !pendingMutation || busy) return;
    const mutation = pendingMutation;
    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    let result: Awaited<ReturnType<typeof mutateResource>>;
    try {
      result = await mutateResource(selectedSession.id, mutation, operationId);
    } catch (reason) {
      finishOperation(operationId);
      const message = errorMessage(reason);
      if (isOutcomeUnknown(reason) || message.includes("mutation succeeded")) {
        const reconciled = await reconcileUnknownMutation(selectedSession.id, mutation.address);
        setPendingMutation(undefined);
        setMessage(reconciled
          ? `${message}；已重新读取远端状态`
          : `${message}；自动回读失败，远端结果仍未知，请先恢复连接并刷新，勿直接重试`);
      } else {
        if (reason && typeof reason === "object" && "code" in reason && reason.code === "conflict") {
          const reconciled = await reconcileUnknownMutation(selectedSession.id, mutation.address);
          setPendingMutation(undefined);
          setMessage(reconciled ? message : `${message}；自动刷新失败，请恢复连接后手动刷新`);
        } else {
          setMessage(message);
        }
      }
      setBusy(false);
      return;
    }
    finishOperation(operationId);
    setPendingMutation(undefined);
    try {
      await reloadRoot(selectedSession.id);
      if (result.operation === "delete") {
        await stopActiveWatch();
        showDocument(undefined);
        setSelectedAddress(undefined);
      } else {
        const refreshed = await runRead(selectedSession.id, result.address);
        showDocument(refreshed);
        setSelectedAddress(result.address);
        setResourceWatch((current) => current ? { ...current, remoteChanged: false } : current);
      }
      setMessage(
        result.consistency === "atomic"
          ? "变更成功，条件版本已校验，脱敏审计已记录"
          : "变更成功；Nacos 操作为校验后变更，脱敏审计已记录",
      );
    } catch (reason) {
      setMessage(`变更已成功，但刷新失败：${errorMessage(reason)}`);
    } finally {
      setBusy(false);
    }
  };

  const openExportDialog = () => {
    if (!document || busy) return;
    setExportIncludeValue(false);
    setExportDialogOpen(true);
  };

  const executeExport = async () => {
    if (!selectedSession || !document || busy) return;
    setBusy(true);
    setMessage(undefined);
    try {
      const receipt = await exportResource(
        selectedSession.id,
        document.address,
        exportIncludeValue,
      );
      if (!receipt) {
        setMessage("已取消导出");
        return;
      }
      setExportDialogOpen(false);
      setMessage(
        `已导出 ${receipt.fileName} · ${receipt.includeValue ? "包含 value，请按敏感文件保管" : "metadata-only，不包含 value"}`,
      );
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const chooseImportFile = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    setMessage(undefined);
    try {
      const preview = await chooseImport(selectedSession.id);
      if (!preview) {
        setMessage("已取消选择导入文件");
        return;
      }
      setImportPreview(preview);
      setImportConfirmationText("");
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const executeImport = async () => {
    if (!selectedSession || !importPreview || busy) return;
    const preview = importPreview;
    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const result = await applyImport(
        selectedSession.id,
        preview.planId,
        operationId,
      );
      setImportPreview(undefined);
      let refreshSuffix = "";
      try {
        await reloadRoot(selectedSession.id);
      } catch (reason) {
        refreshSuffix = `；资源树刷新失败：${errorMessage(reason)}`;
      }
      if (result.failed) {
        setMessage(
          `已写入 ${result.applied.length} 项；“${result.failed.item.name}”失败：${result.failed.error.message}；另有 ${result.remaining} 项未执行${refreshSuffix}`,
        );
      } else {
        setMessage(`导入完成：已写入 ${result.applied.length} 项，${preview.skipped} 项因不含 value 跳过；脱敏审计已记录${refreshSuffix}`);
      }
    } catch (reason) {
      setImportPreview(undefined);
      setMessage(errorMessage(reason));
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const loadHistory = async (scope: string, cursor?: string, append = false) => {
    const generation = historyGeneration.current + 1;
    historyGeneration.current = generation;
    setHistoryLoading(true);
    try {
      const page = await loadAuditHistory(scope === "all" ? undefined : scope, cursor);
      if (historyGeneration.current !== generation) return;
      setHistoryItems((current) => append ? [...current, ...page.items] : page.items);
      setHistoryCursor(page.nextCursor);
    } catch (reason) {
      if (historyGeneration.current === generation) setMessage(errorMessage(reason));
    } finally {
      if (historyGeneration.current === generation) setHistoryLoading(false);
    }
  };

  const openHistory = () => {
    const scope = selectedId ?? "all";
    setHistoryScope(scope);
    setHistoryItems([]);
    setHistoryCursor(undefined);
    setHistoryOpen(true);
    void loadHistory(scope);
  };

  const exportDiagnostics = async () => {
    setBusy(true);
    try {
      const receipt = await exportDiagnosticBundle();
      if (receipt) {
        setMessage(`诊断包 ${receipt.fileName} 已导出；仅包含 ${receipt.connectionCount} 个连接的聚合计数，不含 endpoint、名称、value 或凭据`);
      }
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const changeHistoryScope = (scope: string) => {
    setHistoryScope(scope);
    setHistoryItems([]);
    setHistoryCursor(undefined);
    void loadHistory(scope);
  };

  const closeHistory = () => {
    historyGeneration.current += 1;
    setHistoryLoading(false);
    setHistoryOpen(false);
  };

  const loadServerHistory = async (
    address: ResourceAddress,
    cursor?: string,
    append = false,
  ) => {
    if (!selectedSession) return;
    setServerHistoryLoading(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const page = await listResourceHistory(
        selectedSession.id,
        address,
        operationId,
        cursor,
      );
      setServerHistoryItems((current) => append ? [...current, ...page.items] : page.items);
      setServerHistoryCursor(page.nextCursor);
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      finishOperation(operationId);
      setServerHistoryLoading(false);
    }
  };

  const openServerHistory = () => {
    if (!document || document.address.type !== "nacosConfig" || !selectedSession) return;
    const address = document.address;
    setServerHistoryAddress(address);
    setServerHistoryItems([]);
    setServerHistoryCursor(undefined);
    setServerHistoryDetail(undefined);
    setServerHistoryOpen(true);
    void loadServerHistory(address);
  };

  const readServerHistory = async (entry: ResourceHistoryEntry) => {
    if (!selectedSession || !serverHistoryAddress || serverHistoryLoading) return;
    setServerHistoryLoading(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      setServerHistoryDetail(await readResourceHistory(
        selectedSession.id,
        serverHistoryAddress,
        entry.revisionId,
        operationId,
      ));
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      finishOperation(operationId);
      setServerHistoryLoading(false);
    }
  };

  const openNativeInfo = async () => {
    if (!selectedSession || !selectedProfile || !document || busy) return;
    if (document.address.type !== "etcd" && document.address.type !== "zookeeper") return;
    setNativeInfo(undefined);
    setNativeInfoOpen(true);
    if (document.address.type === "etcd"
      && (!document.metadata.lease || document.metadata.lease === "0")) {
      setNativeInfoLoading(false);
      return;
    }
    setNativeInfoLoading(true);
    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      setNativeInfo(await inspectNativeResource(
        selectedSession.id,
        document.address,
        operationId,
      ));
    } catch (reason) {
      setNativeInfoOpen(false);
      setMessage(isCancelled(reason) ? "原生元数据读取已取消" : errorMessage(reason));
    } finally {
      finishOperation(operationId);
      setNativeInfoLoading(false);
      setBusy(false);
    }
  };

  const executeLeaseAction = async (action: EtcdLeaseAction) => {
    if (!selectedSession || selectedProfile?.adapter !== "etcd" || busy) return;
    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const result = await executeEtcdLeaseAction(selectedSession.id, action, operationId);
      await reloadRoot(selectedSession.id);
      if (result.action === "revoke") {
        await stopActiveWatch();
        setNativeInfoOpen(false);
        setNativeInfo(undefined);
        showDocument(undefined);
        setSelectedAddress(undefined);
        setMessage(`Lease ${result.leaseId} 已撤销；关联 key 已过期，脱敏审计已记录`);
        return;
      }
      const refreshed = await runRead(selectedSession.id, action.address);
      showDocument(refreshed);
      setSelectedAddress(action.address);
      if (result.action === "detach") {
        setNativeInfo(undefined);
        setMessage(`Lease ${result.previousLeaseId} 已原子解绑；key 已变为永久，脱敏审计已记录`);
      } else {
        try {
          setNativeInfo(await inspectNativeResource(
            selectedSession.id,
            action.address,
            newConnectionId(),
          ));
        } catch {
          setNativeInfo(undefined);
        }
        setMessage(result.action === "keepAlive"
          ? `Lease ${result.leaseId} 已续租一次，剩余 TTL ${result.remainingTtlSeconds} 秒；脱敏审计已记录`
          : `Lease ${result.leaseId} 已原子绑定；脱敏审计已记录`);
      }
    } catch (reason) {
      const message = errorMessage(reason);
      try {
        const refreshed = await runRead(selectedSession.id, action.address);
        showDocument(refreshed);
        setSelectedAddress(action.address);
        if (refreshed.metadata.lease && refreshed.metadata.lease !== "0") {
          setNativeInfo(await inspectNativeResource(
            selectedSession.id,
            action.address,
            newConnectionId(),
          ));
        } else {
          setNativeInfo(undefined);
        }
      } catch (readReason) {
        if (isNotFound(readReason)) {
          setNativeInfoOpen(false);
          setNativeInfo(undefined);
          showDocument(undefined);
          setSelectedAddress(undefined);
        }
      }
      if (isOutcomeUnknown(reason)) setNativeInfoOpen(false);
      setMessage(isOutcomeUnknown(reason)
        ? `${message}；已尽力刷新 key 与 Lease 状态，请核对后再决定下一步`
        : message);
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const executeZookeeperAction = async (action: ZookeeperNativeAction) => {
    if (!selectedSession || selectedProfile?.adapter !== "zookeeper" || busy) return;
    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const result = await executeZookeeperNativeAction(selectedSession.id, action, operationId);
      await reloadRoot(selectedSession.id);
      if (result.action === "create") {
        setPendingZookeeperAction(undefined);
        setZookeeperConfirmation("");
        const created = await runRead(selectedSession.id, result.address);
        showDocument(created);
        setSelectedAddress(result.address);
        const path = result.address.type === "zookeeper" ? result.address.path : "新节点";
        setMessage(`${path} 已原子创建并继承父 ACL；脱敏审计已记录`);
      } else {
        setNativeInfo(await inspectNativeResource(
          selectedSession.id,
          result.address,
          newConnectionId(),
        ));
        setMessage(`ACL 已从 aversion ${result.previousAclVersion} 原子更新到 ${result.currentAclVersion}；脱敏审计已记录`);
      }
    } catch (reason) {
      const message = errorMessage(reason);
      try {
        await reloadRoot(selectedSession.id);
        if (action.action === "setAcl") {
          setNativeInfo(await inspectNativeResource(
            selectedSession.id,
            action.address,
            newConnectionId(),
          ));
        }
      } catch {
        // Best-effort reconciliation must not hide the original mutation error.
      }
      if (isOutcomeUnknown(reason)) {
        setPendingZookeeperAction(undefined);
        setNativeInfoOpen(false);
      }
      setMessage(isOutcomeUnknown(reason)
        ? `${message}；已刷新资源树，请核对实际路径或 ACL 后再决定下一步`
        : message);
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const openEtcdTransaction = () => {
    if (!selectedSession || selectedProfile?.adapter !== "etcd" || busy) return;
    setEtcdTransactionItems([emptyEtcdTransactionItem(), emptyEtcdTransactionItem()]);
    setEtcdTransactionConfirmation("");
    setEtcdTransactionOpen(true);
  };

  const executeTransaction = async () => {
    if (!selectedSession || selectedProfile?.adapter !== "etcd" || busy) return;
    let transaction: EtcdTransaction;
    try {
      const seen = new Set<string>();
      const mutations = etcdTransactionItems.map<ResourceMutation>((item) => {
        const address = etcdAddressFromInput(item.key);
        if (seen.has(address.keyBase64)) throw new Error("同一个 etcd key 不能在一次事务中出现两次");
        seen.add(address.keyBase64);
        if (item.operation === "create") {
          return {
            operation: "create",
            address,
            value: { content: item.value, encoding: item.encoding },
          };
        }
        const expectedVersion = item.expectedVersion.trim();
        if (!/^[1-9]\d*$/.test(expectedVersion)) {
          throw new Error(`“${item.key.trim()}”需要正整数 Mod Revision`);
        }
        if (item.operation === "delete") {
          return { operation: "delete", address, expectedVersion };
        }
        return {
          operation: "update",
          address,
          value: { content: item.value, encoding: item.encoding },
          expectedVersion,
        };
      });
      transaction = { mutations };
    } catch (reason) {
      setMessage(errorMessage(reason));
      return;
    }

    setBusy(true);
    setMessage(undefined);
    const operationId = startOperation();
    try {
      const result = await executeEtcdTransaction(selectedSession.id, transaction, operationId);
      setEtcdTransactionOpen(false);
      await reloadRoot(selectedSession.id);
      const selectedResult = document
        ? result.results.find((item) => sameAddress(item.address, document.address))
        : undefined;
      if (selectedResult?.operation === "delete") {
        await stopActiveWatch();
        showDocument(undefined);
        setSelectedAddress(undefined);
      } else if (selectedResult && document) {
        showDocument(await runRead(selectedSession.id, document.address));
      }
      setMessage(`事务已在 revision ${result.revision} 原子提交 ${result.results.length} 项；脱敏审计已记录`);
    } catch (reason) {
      const message = errorMessage(reason);
      try {
        await reloadRoot(selectedSession.id);
        if (document) {
          try {
            showDocument(await runRead(selectedSession.id, document.address));
          } catch (readReason) {
            if (isNotFound(readReason)) {
              showDocument(undefined);
              setSelectedAddress(undefined);
            }
          }
        }
      } catch {
        // The original transaction error is the actionable result.
      }
      if (isOutcomeUnknown(reason)) setEtcdTransactionOpen(false);
      setMessage(isOutcomeUnknown(reason)
        ? `${message}；已尽力刷新远端状态，请核对所有目标 key 后再决定下一步`
        : message);
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (!selectedSession) return;
    await stopActiveWatch();
    try {
      await closeConnection(selectedSession.id);
    } catch {
      // A closed backend session and an absent session have the same local result.
    }
    setSessions((current) => {
      const next = { ...current };
      delete next[selectedSession.id];
      return next;
    });
    setRows([]);
    setActiveSearch(undefined);
    setExportDialogOpen(false);
    setImportPreview(undefined);
    setServerHistoryOpen(false);
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    setEtcdTransactionOpen(false);
    showDocument(undefined);
    setPendingMutation(undefined);
    setPendingZookeeperAction(undefined);
    setNacosNativeOpen(false);
    setCreateDialogOpen(false);
    setMessage("连接已断开");
  };

  const openNewConnection = () => {
    setDialogMode("new");
    setForm(emptyForm());
    setConnectionSecret("");
    setDialogOpen(true);
  };

  const openEditConnection = () => {
    if (!selectedProfile) return;
    setDialogMode("edit");
    setForm(structuredClone(selectedProfile));
    setConnectionSecret("");
    setDialogOpen(true);
  };

  const openCopyConnection = () => {
    if (!selectedProfile) return;
    setDialogMode("copy");
    setForm({
      ...structuredClone(selectedProfile),
      id: newConnectionId(),
      name: `${selectedProfile.name} 副本`,
    });
    setConnectionSecret("");
    setDialogOpen(true);
  };

  const deleteCurrentConnection = async () => {
    if (dialogMode !== "edit") return;
    if (!globalThis.confirm(`确定删除连接“${form.name}”吗？系统凭据也会一并删除。`)) return;
    setBusy(true);
    try {
      if (selectedId === form.id) await stopActiveWatch();
      if (sessions[form.id]) {
        try {
          await closeConnection(form.id);
        } catch {
          // Missing and already-closed sessions have the same local outcome.
        }
      }
      const nextProfiles = await deleteConnectionProfile(form.id);
      setProfiles(nextProfiles);
      setSessions((current) => {
        const next = { ...current };
        delete next[form.id];
        return next;
      });
      if (selectedId === form.id) {
        setSelectedId(undefined);
        setRows([]);
        setActiveSearch(undefined);
        setExportDialogOpen(false);
        setImportPreview(undefined);
        setServerHistoryOpen(false);
        setNativeInfoOpen(false);
        setNativeInfo(undefined);
        showDocument(undefined);
        setSelectedAddress(undefined);
      }
      setDialogOpen(false);
      setConnectionSecret("");
      setMessage("连接和系统凭据已删除");
    } catch (reason) {
      setMessage(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const watchBackendIsActive = resourceWatch
    ? ["starting", "live", "reconnecting", "compacted", "sessionExpired"].includes(resourceWatch.state)
    : false;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand"><span className="logo">A</span>Atlas Registry</div>
        <span className="release-tag">SAFE-WRITE ALPHA</span>
        <div className="top-spacer" />
        <div className={`runtime ${capabilities ? "" : "pending"}`}>
          <span className="status-dot" />
          {capabilities ? `Rust Core · ${capabilities.length} adapters` : "正在启动 Rust Core…"}
        </div>
        <button className="button" disabled={busy} onClick={() => void exportDiagnostics()}>诊断包</button>
        <button className="button" onClick={openHistory}>历史</button>
        <button className="button primary" onClick={openNewConnection}>＋ 新建连接</button>
      </header>

      <div className="shell">
        <aside className="connections">
          <div className="eyebrow">连接</div>
          {profiles.length === 0 && (
            <div className="empty compact">
              <b>还没有连接</b>
              <span>添加 etcd、ZooKeeper 或 Nacos 后开始浏览。</span>
            </div>
          )}
          {profiles.map((profile) => (
            <button
              className={`connection ${profile.id === selectedId ? "active" : ""}`}
              key={profile.id}
              onClick={() => void selectProfile(profile)}
            >
              <span className={`status-dot ${sessions[profile.id] ? "" : "offline"}`} />
              <span><b>{profile.name}</b><small>{profile.endpoint} · {connectionEnvironmentLabels[profile.environment]}</small></span>
              <span className={`badge ${profile.adapter}`}>{connectionLabel(profile.adapter)}</span>
            </button>
          ))}

          {selectedProfile && !selectedSession && (
            <button className="button primary wide" disabled={busy} onClick={() => void connectAndLoad(selectedProfile)}>
              {busy ? "连接中…" : "连接并浏览"}
            </button>
          )}
          {selectedSession && (
            <button className="button wide" onClick={() => void disconnect()}>断开连接</button>
          )}
          {selectedProfile && (
            <div className="connection-actions">
              <button className="button" disabled={busy} onClick={openEditConnection}>编辑</button>
              <button className="button" disabled={busy} onClick={openCopyConnection}>复制</button>
            </div>
          )}
          <button className="button wide" onClick={openNewConnection}>＋ 添加连接</button>

          <div className="capabilities">
            <div className="eyebrow">NATIVE RUST ADAPTERS</div>
            {capabilities?.map((adapter) => (
              <span
                className={`badge ${adapter.id}`}
                title={adapter.capabilities.join(" · ")}
                key={adapter.id}
              >
                {adapter.id} · {adapter.capabilities.length}
              </span>
            ))}
          </div>
        </aside>

        <section className="tree">
          <div className="tree-header">
            <b>{selectedProfile?.name ?? "资源"}</b>
            <button className="icon-button import-resource" disabled={!selectedSession || busy} onClick={() => void chooseImportFile()} title="从 Atlas JSON 导入">⇧</button>
            {selectedProfile?.adapter === "etcd" && <button className="icon-button transaction-resource" disabled={!selectedSession || busy} onClick={openEtcdTransaction} title="etcd 原子批量事务">T</button>}
            {selectedProfile?.adapter === "nacos" && <button className="icon-button transaction-resource" disabled={!selectedSession || busy} onClick={() => setNacosNativeOpen(true)} title="Nacos 命名空间、服务与实例管理">N</button>}
            <button className="icon-button create-resource" disabled={!selectedSession || busy} onClick={openCreateResource} title="新建资源">＋</button>
            <button className="icon-button" disabled={!selectedSession || busy} onClick={() => void refreshRoot()} title="刷新">↻</button>
            <input value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="筛选当前已加载资源…" />
            <div className="resource-query">
              <input
                value={resourceQuery}
                disabled={!selectedSession || busy}
                onChange={(event) => setResourceQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void searchCurrentScope();
                }}
                placeholder={selectedProfile?.adapter === "nacos"
                  ? "搜索 dataId；定位请填 GROUP / dataId"
                  : selectedProfile?.adapter === "zookeeper"
                    ? "搜索节点名；定位请填 /绝对路径"
                    : "搜索 key；定位可填 key 或 base64:…"}
              />
              <button className="button" disabled={!selectedSession || busy} onClick={() => void searchCurrentScope()}>搜索</button>
              <button className="button" disabled={!selectedSession || busy} onClick={() => void locateResource()}>定位</button>
            </div>
            {activeSearch && (
              <div className="search-state">
                <span>“{activeSearch.query}” · 已检查 {activeSearch.scanned} 个标识{activeSearch.exhaustive ? " · 已完成" : ""}</span>
                <button disabled={busy} onClick={() => void exitSearch()}>返回资源树</button>
              </div>
            )}
          </div>

          {!selectedSession && (
            <div className="empty"><span className="empty-icon">◇</span><b>选择并打开连接</b><span>资源会按需加载，不会扫描整个集群。</span></div>
          )}
          {selectedSession && rows.length === 0 && !busy && (
            <div className="empty"><span className="empty-icon">∅</span><b>{activeSearch ? "没有匹配的资源" : "当前范围没有资源"}</b><span>{activeSearch ? "可调整标识关键词，搜索不会读取资源值。" : "可以刷新，或检查所选 namespace 和权限。"}</span></div>
          )}
          {visibleRows.map((row) => {
            const actualIndex = rows.indexOf(row);
            if (row.kind === "more") {
              return (
                <button className="node load-more" style={{ paddingLeft: 14 + row.depth * 20 }} key={`more-${row.cursor}`} onClick={() => void loadMore(actualIndex, row)}>
                  … 加载更多
                </button>
              );
            }
            const selected = selectedAddress && JSON.stringify(selectedAddress) === JSON.stringify(row.node.address);
            return (
              <button
                className={`node ${selected ? "active" : ""}`}
                style={{ paddingLeft: 14 + row.depth * 20 }}
                key={`${row.depth}-${row.node.name}-${JSON.stringify(row.node.address)}`}
                onClick={() => void openResource(actualIndex, row)}
              >
                <span className="disclosure">{row.node.hasChildren === false ? "" : row.expanded ? "⌄" : "›"}</span>
                <span className={row.node.readable ? "key" : "folder"}>{row.node.readable ? "◇" : "◆"}</span>
                <span className="node-name">{row.node.name}</span>
              </button>
            );
          })}
          {busy && <div className="loading-line">正在与注册中心通信… {activeOperation && <button onClick={() => void cancelActiveOperation()}>取消</button>}</div>}
        </section>

        <main className="detail">
          {!document ? (
            <div className="detail-empty">
              <span className="empty-icon large">{busy ? "◌" : "◇"}</span>
              <h1>{busy ? "正在读取" : "选择一个资源"}</h1>
              <p>资源值仅在选中时读取；二进制数据会以 Base64 无损展示。</p>
            </div>
          ) : (
            <>
              <div className="breadcrumb">{selectedProfile?.name} / <b>{addressLabel(document.address)}</b></div>
              <div className="detail-title">
                <div><span className="eyebrow">RESOURCE</span><h1>{document.name}</h1></div>
                <div className="actions">
                  {document.address.type === "nacosConfig" && <button className="button" disabled={busy} onClick={openServerHistory}>服务端历史</button>}
                  {document.address.type === "nacosConfig" && <button className="button" disabled={busy} onClick={() => setNacosNativeOpen(true)}>服务管理</button>}
                  {document.address.type === "etcd" && <button className="button" disabled={busy} onClick={() => void openNativeInfo()}>Lease</button>}
                  {document.address.type === "zookeeper" && <button className="button" disabled={busy} onClick={() => void openNativeInfo()}>ACL</button>}
                  <button className="button" disabled={busy} onClick={openExportDialog}>导出</button>
                  <button className="button danger" disabled={busy || !document.version} onClick={prepareDelete}>删除</button>
                  <button className="button primary" disabled={busy || !document.version || draftValue === document.value.content} onClick={prepareUpdate}>保存变更</button>
                </div>
              </div>
              <div className="stats">
                <div><span>版本</span><strong>{document.version || "—"}</strong></div>
                <div><span>编码</span><strong>{document.value.encoding.toUpperCase()}</strong></div>
                <div><span>大小</span><strong>{document.value.sizeBytes.toLocaleString()} B</strong></div>
              </div>
              <div className={`watch-panel ${resourceWatch?.state ?? "idle"} ${resourceWatch?.remoteChanged ? "changed" : ""}`}>
                <div className="watch-summary">
                  <span className="watch-pulse" />
                  <div>
                    <b>{resourceWatch ? watchStatusLabels[resourceWatch.state] : "实时监听未开启"}</b>
                    <span>
                      {resourceWatch?.message
                        ? `${resourceWatch.message}${resourceWatch.retryInMs ? ` · ${resourceWatch.retryInMs} ms 后重试` : ""}`
                        : (resourceWatch?.lastChange
                          ? `${watchChangeLabels[resourceWatch.lastChange.change]} · 版本 ${resourceWatch.lastChange.version ?? "未知"}`
                          : "监听事件只包含地址、类型和版本，不传输资源值")}
                    </span>
                  </div>
                  {resourceWatch && <span className="watch-count">{resourceWatch.changeCount} 次变化</span>}
                </div>
                <div className="watch-actions">
                  {resourceWatch?.remoteChanged && (
                    <button className="button primary" disabled={busy} onClick={() => void refreshWatchedResource()}>
                      读取最新版本
                    </button>
                  )}
                  <button
                    className="button"
                    disabled={busy}
                    onClick={() => void (watchBackendIsActive ? stopResourceWatch() : startResourceWatch())}
                  >
                    {watchBackendIsActive ? "停止监听" : resourceWatch ? "重新监听" : "开始监听"}
                  </button>
                </div>
              </div>
              {document.value.encoding === "base64" && (
                <div className="binary-warning">该值不是有效 UTF-8，已使用 Base64 展示，内容没有被替换或损坏。</div>
              )}
              <div className="editor-header"><span>{document.contentType?.toUpperCase() || "TEXT"}</span><span>{draftValue === document.value.content ? document.value.encoding.toUpperCase() : `${document.value.encoding.toUpperCase()} · 已修改`}</span></div>
              <textarea value={draftValue} disabled={busy} onChange={(event) => setDraftValue(event.target.value)} spellCheck={false} />
              <div className="metadata">
                {Object.entries(document.metadata).map(([name, value]) => (
                  <div className="metadata-row" key={name}><span>{name}</span><b>{value || "—"}</b></div>
                ))}
              </div>
            </>
          )}
        </main>
      </div>

      {message && <button className="toast" onClick={() => setMessage(undefined)}>{message}</button>}

      {dialogOpen && (
        <ConnectionDialog
          mode={dialogMode}
          form={form}
          secret={connectionSecret}
          busy={busy}
          testing={testingConnection}
          onChange={setForm}
          onSecretChange={setConnectionSecret}
          onCancel={() => setDialogOpen(false)}
          onTest={() => void testConnection()}
          onSave={() => void saveAndConnect()}
          onDelete={() => void deleteCurrentConnection()}
          onCancelOperation={() => void cancelActiveOperation()}
        />
      )}

      {createDialogOpen && selectedProfile && (
        <NewResourceDialog
          adapter={selectedProfile.adapter}
          draft={resourceDraft}
          onChange={setResourceDraft}
          onCancel={() => setCreateDialogOpen(false)}
          onContinue={prepareCreate}
        />
      )}

      {pendingMutation && selectedProfile && (
        <MutationConfirmationDialog
          mutation={pendingMutation}
          profile={selectedProfile}
          confirmationText={confirmationText}
          busy={busy}
          onConfirmationTextChange={setConfirmationText}
          onCancel={() => setPendingMutation(undefined)}
          onConfirm={() => void executeMutation()}
          onCancelOperation={() => void cancelActiveOperation()}
        />
      )}

      {pendingZookeeperAction && selectedProfile?.adapter === "zookeeper" && (
        <ZookeeperCreateConfirmationDialog
          profile={selectedProfile}
          action={pendingZookeeperAction}
          confirmation={zookeeperConfirmation}
          busy={busy}
          onConfirmationChange={setZookeeperConfirmation}
          onConfirm={() => void executeZookeeperAction(pendingZookeeperAction)}
          onCancelOperation={() => void cancelActiveOperation()}
          onClose={() => setPendingZookeeperAction(undefined)}
        />
      )}

      {etcdTransactionOpen && selectedProfile?.adapter === "etcd" && (
        <EtcdTransactionDialog
          profile={selectedProfile}
          items={etcdTransactionItems}
          confirmationText={etcdTransactionConfirmation}
          busy={busy}
          onItemsChange={setEtcdTransactionItems}
          onConfirmationTextChange={setEtcdTransactionConfirmation}
          onCancel={() => setEtcdTransactionOpen(false)}
          onExecute={() => void executeTransaction()}
          onCancelOperation={() => void cancelActiveOperation()}
        />
      )}

      {exportDialogOpen && document && (
        <ExportDialog
          document={document}
          includeValue={exportIncludeValue}
          busy={busy}
          onIncludeValueChange={setExportIncludeValue}
          onCancel={() => setExportDialogOpen(false)}
          onExport={() => void executeExport()}
        />
      )}

      {importPreview && selectedProfile && (
        <ImportPreviewDialog
          preview={importPreview}
          profile={selectedProfile}
          confirmationText={importConfirmationText}
          busy={busy}
          onConfirmationTextChange={setImportConfirmationText}
          onCancel={() => setImportPreview(undefined)}
          onApply={() => void executeImport()}
          onCancelOperation={() => void cancelActiveOperation()}
        />
      )}

      {historyOpen && (
        <HistoryDialog
          profiles={profiles}
          scope={historyScope}
          items={historyItems}
          nextCursor={historyCursor}
          loading={historyLoading}
          onScopeChange={changeHistoryScope}
          onLoadMore={() => void loadHistory(historyScope, historyCursor, true)}
          onClose={closeHistory}
        />
      )}

      {serverHistoryOpen && serverHistoryAddress && (
        <NacosHistoryDialog
          resourceName={document?.name ?? "Nacos 配置"}
          items={serverHistoryItems}
          nextCursor={serverHistoryCursor}
          detail={serverHistoryDetail}
          loading={serverHistoryLoading}
          onRead={(entry) => void readServerHistory(entry)}
          onLoadMore={() => void loadServerHistory(serverHistoryAddress, serverHistoryCursor, true)}
          onBack={() => setServerHistoryDetail(undefined)}
          onCancelOperation={() => void cancelActiveOperation()}
          onClose={() => setServerHistoryOpen(false)}
        />
      )}

      {nacosNativeOpen && selectedProfile?.adapter === "nacos" && selectedSession && (
        <NacosNativeDialog
          profile={selectedProfile}
          connectionId={selectedSession.id}
          onMessage={setMessage}
          onClose={() => setNacosNativeOpen(false)}
        />
      )}

      {nativeInfoOpen
        && selectedProfile?.adapter === "etcd"
        && document?.address.type === "etcd"
        && (
        <EtcdLeaseDialog
          profile={selectedProfile}
          document={document}
          info={nativeInfo?.kind === "etcdLease" ? nativeInfo : undefined}
          loading={nativeInfoLoading}
          busy={busy}
          onExecute={(action) => void executeLeaseAction(action)}
          onCancelOperation={() => void cancelActiveOperation()}
          onClose={() => {
            setNativeInfoOpen(false);
            setNativeInfo(undefined);
          }}
        />
      )}

      {nativeInfoOpen && selectedProfile?.adapter === "zookeeper" && (
        document?.address.type === "zookeeper" && <ZookeeperAclDialog
          profile={selectedProfile}
          document={document}
          info={nativeInfo?.kind === "zookeeperAcl" ? nativeInfo : undefined}
          loading={nativeInfoLoading}
          busy={busy}
          onExecute={(action) => void executeZookeeperAction(action)}
          onCancelOperation={() => void cancelActiveOperation()}
          onClose={() => {
            setNativeInfoOpen(false);
            setNativeInfo(undefined);
          }}
        />
      )}
    </div>
  );
}
