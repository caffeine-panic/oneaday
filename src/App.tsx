import { useEffect, useMemo, useRef, useState } from "react";
import { planProfileSelection } from "./profileSelection";
import { useRegistryOperations } from "./useRegistryOperations";
import { useResourceWorkspace } from "./useResourceWorkspace";
import {
  collapseResourceRow,
  expandResourceRow,
  pageRows,
  replaceContinuationRow,
  searchPageRows,
  type MoreRow,
  type ResourceRow,
} from "./resourceTree";
import { UpdateDialog, type UpdateProgress } from "./UpdateDialog";
import { SettingsDialog } from "./SettingsDialog";
import { Toast } from "./Toast";
import { nextToast, type ToastMessage, type ToastTone } from "./toastState";
import {
  loadUpdateProxySettings,
  saveUpdateProxySettings,
  type UpdateProxySettings,
} from "./updateSettings";
import {
  loadPanelLayout,
  savePanelLayout,
  togglePanel,
  type PanelId,
} from "./panelLayout";
import {
  MutationConfirmationDialog,
  NewResourceDialog,
  type NewResourceDraft,
} from "./MutationDialogs";
import {
  ConnectionDialog,
  type ConnectionDialogMode,
} from "./ConnectionDialog";
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
  checkForAppUpdate,
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
  installAppUpdate,
  isCancelled,
  isNotFound,
  isOutcomeUnknown,
  listResources,
  listResourceHistory,
  loadAuditHistory,
  loadConnectionProfiles,
  newConnectionId,
  mutateResource,
  mutationFailureRecovery,
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
  type AppUpdateEvent,
  type AppUpdateInfo,
  type AuditHistoryItem,
  type ConnectionProfile,
  type ConnectionSession,
  type EtcdLeaseAction,
  type EtcdTransaction,
  type ImportPreview,
  type NativeResourceInfo,
  type ResourceAddress,
  type ResourceHistoryDocument,
  type ResourceHistoryEntry,
  type ResourceMutation,
  type WatchEvent,
  type WatchHandle,
  type WatchStatusState,
  type ZookeeperNativeAction,
} from "./registry";

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

function etcdAddressFromInput(
  rawInput: string,
): Extract<ResourceAddress, { type: "etcd" }> {
  const input = rawInput.trim();
  if (!input) throw new Error("etcd transaction key 不能为空");
  if (!input.startsWith("base64:"))
    return { type: "etcd", keyBase64: utf8Base64(input) };
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
    if (!input.startsWith("base64:"))
      return { type: "etcd", keyBase64: utf8Base64(input) };
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
    if (
      !input.startsWith("/") ||
      input.includes("//") ||
      (input.length > 1 && input.endsWith("/"))
    ) {
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

function searchScope(
  adapter: AdapterId,
  selected?: ResourceAddress,
): ResourceAddress {
  if (adapter === "etcd" && selected?.type === "etcdPrefix") return selected;
  if (adapter === "zookeeper" && selected?.type === "zookeeper")
    return selected;
  return ROOT_ADDRESS;
}

export function App() {
  const [capabilities, setCapabilities] = useState<AdapterDescriptor[]>();
  const [profiles, setProfiles] = useState<ConnectionProfile[]>([]);
  const [sessions, setSessions] = useState<Record<string, ConnectionSession>>(
    {},
  );
  const [selectedId, setSelectedId] = useState<string>();
  const {
    state: {
      rows,
      document,
      draftValue,
      selectedAddress,
      filter,
      resourceQuery,
      activeSearch,
    },
    clearView,
    showDocument,
    setRows,
    setDraftValue,
    setSelectedAddress,
    setFilter,
    setResourceQuery,
    setActiveSearch,
  } = useResourceWorkspace();
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<ToastMessage>();
  const showToast = (text: string, tone: ToastTone) =>
    setToast((current) => nextToast(current, text, tone));
  const clearToast = () => setToast(undefined);
  const showSuccess = (text: string) => showToast(text, "success");
  const showInfo = (text: string) => showToast(text, "info");
  const showWarning = (text: string) => showToast(text, "warning");
  const showErrorText = (text: string) => showToast(text, "error");
  const showError = (reason: unknown) => showErrorText(errorMessage(reason));
  const dismissToast = (id: number) =>
    setToast((current) => (current?.id === id ? undefined : current));
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdateInfo>();
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress>();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [updateProxySettings, setUpdateProxySettings] =
    useState<UpdateProxySettings>(loadUpdateProxySettings);
  const [panelLayout, setPanelLayout] = useState(loadPanelLayout);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [dialogMode, setDialogMode] = useState<ConnectionDialogMode>("new");
  const [testingConnection, setTestingConnection] = useState(false);
  const [form, setForm] = useState<ConnectionProfile>(emptyForm);
  const [connectionSecret, setConnectionSecret] = useState("");
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [resourceDraft, setResourceDraft] = useState<NewResourceDraft>(() =>
    emptyResourceDraft("etcd"),
  );
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
  const [serverHistoryAddress, setServerHistoryAddress] =
    useState<ResourceAddress>();
  const [serverHistoryItems, setServerHistoryItems] = useState<
    ResourceHistoryEntry[]
  >([]);
  const [serverHistoryCursor, setServerHistoryCursor] = useState<string>();
  const [serverHistoryDetail, setServerHistoryDetail] =
    useState<ResourceHistoryDocument>();
  const [serverHistoryLoading, setServerHistoryLoading] = useState(false);
  const [nativeInfoOpen, setNativeInfoOpen] = useState(false);
  const [nativeInfo, setNativeInfo] = useState<NativeResourceInfo>();
  const [nativeInfoLoading, setNativeInfoLoading] = useState(false);
  const [etcdTransactionOpen, setEtcdTransactionOpen] = useState(false);
  const [etcdTransactionItems, setEtcdTransactionItems] = useState<
    EtcdTransactionDraftItem[]
  >([]);
  const [etcdTransactionConfirmation, setEtcdTransactionConfirmation] =
    useState("");
  const [pendingZookeeperAction, setPendingZookeeperAction] =
    useState<Extract<ZookeeperNativeAction, { action: "create" }>>();
  const [zookeeperConfirmation, setZookeeperConfirmation] = useState("");
  const [nacosNativeOpen, setNacosNativeOpen] = useState(false);
  const [resourceWatch, setResourceWatch] = useState<ResourceWatchView>();
  const watchHandle = useRef<WatchHandle | undefined>(undefined);
  const watchGeneration = useRef(0);
  const historyGeneration = useRef(0);
  const operations = useRegistryOperations<"main" | "serverHistory">(
    newConnectionId,
    cancelOperation,
  );
  const activeOperation = operations.active.main;

  const selectedProfile = profiles.find((profile) => profile.id === selectedId);
  const selectedSession = selectedId ? sessions[selectedId] : undefined;
  const connectionsExpanded = panelLayout.connections === "expanded";
  const resourcesExpanded = panelLayout.resources === "expanded";

  const toggleNavigationPanel = (panel: PanelId) => {
    setPanelLayout((current) => savePanelLayout(togglePanel(current, panel)));
  };

  useEffect(() => {
    registryCapabilities()
      .then(setCapabilities)
      .catch((reason: unknown) =>
        setToast((current) =>
          nextToast(current, errorMessage(reason), "error"),
        ),
      );
    loadConnectionProfiles()
      .then(setProfiles)
      .catch((reason: unknown) =>
        setToast((current) =>
          nextToast(current, errorMessage(reason), "error"),
        ),
      );
  }, []);

  useEffect(
    () => () => {
      watchGeneration.current += 1;
      const active = watchHandle.current;
      if (active) void stopWatch(active.subscriptionId);
    },
    [],
  );

  const visibleRows = useMemo(() => {
    const query = filter.trim().toLocaleLowerCase();
    if (!query) return rows;
    return rows.filter(
      (row) =>
        row.kind === "resource" &&
        row.node.name.toLocaleLowerCase().includes(query),
    );
  }, [filter, rows]);

  const startOperation = () => operations.start("main");

  const finishOperation = (operationId: string) => {
    operations.finish("main", operationId);
  };

  const checkForUpdates = async () => {
    if (checkingUpdate || installingUpdate) return;
    setCheckingUpdate(true);
    clearToast();
    try {
      const update = await checkForAppUpdate(updateProxySettings);
      if (!update) {
        showInfo("当前已是最新版本");
        return;
      }
      setUpdateProgress(undefined);
      setAvailableUpdate(update);
    } catch (reason) {
      showError(reason);
    } finally {
      setCheckingUpdate(false);
    }
  };

  const installAvailableUpdate = async () => {
    if (!availableUpdate || installingUpdate) return;
    setInstallingUpdate(true);
    setUpdateProgress({ phase: "downloading", downloaded: 0 });
    clearToast();
    const onEvent = (event: AppUpdateEvent) => {
      if (event.event === "started") {
        setUpdateProgress({
          phase: "downloading",
          downloaded: 0,
          contentLength: event.data.contentLength,
        });
      } else if (event.event === "progress") {
        setUpdateProgress({
          phase: "downloading",
          downloaded: event.data.downloaded,
          contentLength: event.data.contentLength,
        });
      } else {
        setUpdateProgress((current) => ({
          phase: "installing",
          downloaded: current?.downloaded ?? 0,
          contentLength: current?.contentLength,
        }));
      }
    };
    try {
      await installAppUpdate(onEvent);
    } catch (reason) {
      setInstallingUpdate(false);
      showError(reason);
    }
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
      return await searchResources(
        connectionId,
        scope,
        query,
        operationId,
        cursor,
      );
    } finally {
      finishOperation(operationId);
    }
  };

  const releaseActiveWatch = async (clearView = true) => {
    const active = watchHandle.current;
    watchHandle.current = undefined;
    if (clearView) setResourceWatch(undefined);
    if (!active) return;
    try {
      await stopWatch(active.subscriptionId);
    } catch (reason) {
      if (!clearView) showError(reason);
    }
  };

  const stopActiveWatch = async (clearView = true) => {
    watchGeneration.current += 1;
    await releaseActiveWatch(clearView);
  };

  const handleWatchEvent = (event: WatchEvent) => {
    setResourceWatch((current) => {
      if (!current || current.subscriptionId !== event.subscriptionId)
        return current;
      if (event.kind === "status") {
        if (event.state === "stopped" || event.state === "failed") {
          if (watchHandle.current?.subscriptionId === event.subscriptionId) {
            watchHandle.current = undefined;
          }
        }
        return {
          ...current,
          state: event.state,
          message: event.message ?? undefined,
          retryInMs: event.retryInMs ?? undefined,
          remoteChanged:
            event.state === "compacted" ? true : current.remoteChanged,
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
      setResourceWatch((current) =>
        current?.subscriptionId === subscriptionId
          ? { ...current, state: "failed", message: errorMessage(reason) }
          : current,
      );
      showError(reason);
    }
  };

  const stopResourceWatch = async () => {
    await stopActiveWatch(false);
    setResourceWatch((current) =>
      current
        ? {
            ...current,
            state: "stopped",
            message: undefined,
            retryInMs: undefined,
          }
        : current,
    );
  };

  const refreshWatchedResource = async () => {
    if (!selectedSession || !document || busy) return;
    if (
      draftValue !== document.value.content &&
      !globalThis.confirm(
        "远端资源已变化。刷新会丢弃当前未保存的编辑，是否继续？",
      )
    ) {
      return;
    }
    setBusy(true);
    clearToast();
    try {
      showDocument(await runRead(selectedSession.id, document.address));
      setResourceWatch((current) =>
        current ? { ...current, remoteChanged: false } : current,
      );
      showSuccess("已读取远端最新版本");
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
        showWarning("远端资源已删除，已移除本地旧内容");
      } else {
        showErrorText(`刷新监听资源失败：${errorMessage(reason)}`);
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
    try {
      await operations.cancel("main");
    } catch (reason) {
      showError(reason);
    }
  };

  const connectAndLoad = async (
    profile: ConnectionProfile,
    transientSecret?: string,
  ) => {
    await stopActiveWatch();
    await operations.cancel("serverHistory").catch(() => false);
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    setBusy(true);
    clearToast();
    clearView();
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
      showSuccess(`已连接 ${session.endpoint}`);
      return true;
    } catch (reason) {
      showError(reason);
      return false;
    } finally {
      setBusy(false);
    }
  };

  const saveAndConnect = async () => {
    const candidate = normalizedProfile(form);
    if (!candidate.name || !candidate.endpoint) {
      showErrorText("连接名称和 endpoint 不能为空");
      return;
    }
    if (
      candidate.auth.mode !== "none" &&
      dialogMode !== "edit" &&
      !connectionSecret
    ) {
      showErrorText("新连接启用认证时必须填写密钥");
      return;
    }
    try {
      const credentialUpdate =
        candidate.auth.mode === "none"
          ? { operation: "clear" as const }
          : connectionSecret
            ? { operation: "replace" as const, secret: connectionSecret }
            : { operation: "preserve" as const };
      const nextProfiles = await upsertConnectionProfile(
        candidate,
        credentialUpdate,
      );
      setProfiles(nextProfiles);
      setDialogOpen(false);
      await connectAndLoad(candidate, connectionSecret || undefined);
      setConnectionSecret("");
    } catch (reason) {
      showError(reason);
    }
  };

  const testConnection = async () => {
    const candidate = normalizedProfile(form);
    if (!candidate.name || !candidate.endpoint) {
      showErrorText("连接名称和 endpoint 不能为空");
      return;
    }
    setBusy(true);
    setTestingConnection(true);
    clearToast();
    const operationId = startOperation();
    try {
      const result = await probeConnection(
        candidate,
        operationId,
        connectionSecret || undefined,
      );
      showSuccess(`连接测试成功：${result.endpoint}`);
    } catch (reason) {
      if (isCancelled(reason)) showInfo("连接测试已取消");
      else showError(reason);
    } finally {
      finishOperation(operationId);
      setTestingConnection(false);
      setBusy(false);
    }
  };

  const selectProfile = async (profile: ConnectionProfile) => {
    const session = sessions[profile.id];
    const selectionPlan = planProfileSelection(
      selectedId,
      profile.id,
      Boolean(session),
    );
    if (selectionPlan === "preserve") return;

    await stopActiveWatch();
    await operations.cancel("serverHistory").catch(() => false);
    setSelectedId(profile.id);
    clearView();
    setExportDialogOpen(false);
    setImportPreview(undefined);
    setServerHistoryOpen(false);
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    clearToast();

    if (selectionPlan === "reload" && session) {
      setBusy(true);
      try {
        await reloadRoot(session.id);
      } catch (reason) {
        showError(reason);
      } finally {
        setBusy(false);
      }
    }
  };

  const refreshRoot = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    clearToast();
    try {
      await reloadRoot(selectedSession.id);
    } catch (reason) {
      showError(reason);
    } finally {
      setBusy(false);
    }
  };

  const searchCurrentScope = async () => {
    if (!selectedSession || !selectedProfile || busy) return;
    const query = resourceQuery.trim();
    if (!query) {
      showErrorText("请输入要搜索的资源标识");
      return;
    }
    const scope = searchScope(selectedProfile.adapter, selectedAddress);
    setBusy(true);
    clearToast();
    try {
      const page = await runSearch(selectedSession.id, scope, query);
      setRows(searchPageRows(page.items, page.scope, query, page.nextCursor));
      setActiveSearch({
        scope: page.scope,
        query,
        scanned: page.scanned,
        exhaustive: page.exhaustive,
      });
      setFilter("");
      showInfo(
        `${page.items.length} 个匹配项 · 本次检查 ${page.scanned} 个标识${page.exhaustive ? " · 已到当前范围末尾" : " · 可继续加载"}`,
      );
    } catch (reason) {
      showError(reason);
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
      showError(reason);
      return;
    }
    if (
      document &&
      draftValue !== document.value.content &&
      !globalThis.confirm("定位其他资源会丢弃当前未保存的编辑，是否继续？")
    ) {
      return;
    }
    if (!document || !sameAddress(document.address, address))
      await stopActiveWatch();
    setBusy(true);
    clearToast();
    setSelectedAddress(address);
    try {
      showDocument(await runRead(selectedSession.id, address));
      showSuccess("已精确定位并读取资源");
    } catch (reason) {
      showDocument(undefined);
      showError(reason);
    } finally {
      setBusy(false);
    }
  };

  const exitSearch = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    clearToast();
    try {
      await reloadRoot(selectedSession.id);
    } catch (reason) {
      showError(reason);
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
    clearToast();

    if (row.node.readable) {
      setBusy(true);
      try {
        showDocument(await runRead(selectedSession.id, row.node.address));
      } catch (reason) {
        showDocument(undefined);
        showError(reason);
        if (isCancelled(reason)) return;
      } finally {
        setBusy(false);
      }
    }

    if (row.node.hasChildren === false) return;
    if (row.expanded) {
      setRows((current) =>
        collapseResourceRow(current, index, row.node.address),
      );
      return;
    }

    setBusy(true);
    try {
      const page = await runList(selectedSession.id, row.node.address);
      setRows((current) => expandResourceRow(current, index, page));
    } catch (reason) {
      showError(reason);
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
        setRows((current) =>
          replaceContinuationRow(
            current,
            index,
            searchPageRows(
              page.items,
              page.scope,
              row.search!.query,
              page.nextCursor,
            ),
            row,
          ),
        );
        setActiveSearch((current) =>
          current
            ? {
                ...current,
                scanned: current.scanned + page.scanned,
                exhaustive: page.exhaustive,
              }
            : current,
        );
        return;
      }
      const page = await runList(selectedSession.id, row.parent, row.cursor);
      setRows((current) =>
        replaceContinuationRow(
          current,
          index,
          pageRows(page.items, row.depth, page.parent, page.nextCursor),
          row,
        ),
      );
    } catch (reason) {
      showError(reason);
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
        showErrorText("etcd key 不能为空");
        return;
      }
      address = {
        type: "etcd",
        keyBase64: utf8Base64(resourceDraft.keyOrPath),
      };
    } else if (selectedProfile.adapter === "zookeeper") {
      const path = resourceDraft.keyOrPath.trim();
      if (
        !path.startsWith("/") ||
        path === "/" ||
        path.endsWith("/") ||
        path.includes("//")
      ) {
        showErrorText(
          "ZooKeeper 路径必须是规范的绝对非根路径，且不能以 / 结尾",
        );
        return;
      }
      address = { type: "zookeeper", path };
    } else {
      const group = resourceDraft.group.trim();
      const dataId = resourceDraft.dataId.trim();
      if (!group || !dataId || !resourceDraft.content) {
        showErrorText("Nacos 创建需要 group、dataId 和非空内容");
        return;
      }
      address = { type: "nacosConfig", group, dataId };
    }
    if (
      selectedProfile.adapter === "zookeeper" &&
      resourceDraft.zookeeperMode !== "persistent"
    ) {
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
      showErrorText("当前资源没有可用于条件更新的版本，请先刷新");
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
      showErrorText("当前资源没有可用于条件删除的版本，请先刷新");
      return;
    }
    setPendingMutation({
      operation: "delete",
      address: document.address,
      expectedVersion: document.version,
    });
    setConfirmationText("");
  };

  const reconcileUnknownMutation = async (
    connectionId: string,
    address: ResourceAddress,
  ) => {
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
    clearToast();
    const operationId = startOperation();
    let result: Awaited<ReturnType<typeof mutateResource>>;
    try {
      result = await mutateResource(selectedSession.id, mutation, operationId);
    } catch (reason) {
      finishOperation(operationId);
      const message = errorMessage(reason);
      const recovery = mutationFailureRecovery(reason);
      if (recovery === "unknownOutcome") {
        const reconciled = await reconcileUnknownMutation(
          selectedSession.id,
          mutation.address,
        );
        setPendingMutation(undefined);
        showErrorText(
          reconciled
            ? `${message}；已重新读取远端状态`
            : `${message}；自动回读失败，远端结果仍未知，请先恢复连接并刷新，勿直接重试`,
        );
      } else {
        if (recovery === "conflict") {
          const reconciled = await reconcileUnknownMutation(
            selectedSession.id,
            mutation.address,
          );
          setPendingMutation(undefined);
          showErrorText(
            reconciled
              ? message
              : `${message}；自动刷新失败，请恢复连接后手动刷新`,
          );
        } else {
          showErrorText(message);
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
        setResourceWatch((current) =>
          current ? { ...current, remoteChanged: false } : current,
        );
      }
      showSuccess(
        result.consistency === "atomic"
          ? "变更成功，条件版本已校验，脱敏审计已记录"
          : "变更成功；Nacos 操作为校验后变更，脱敏审计已记录",
      );
    } catch (reason) {
      showWarning(`变更已成功，但刷新失败：${errorMessage(reason)}`);
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
    clearToast();
    try {
      const receipt = await exportResource(
        selectedSession.id,
        document.address,
        exportIncludeValue,
      );
      if (!receipt) {
        showInfo("已取消导出");
        return;
      }
      setExportDialogOpen(false);
      const exportMessage = `已导出 ${receipt.fileName} · ${receipt.includeValue ? "包含 value，请按敏感文件保管" : "metadata-only，不包含 value"}`;
      if (receipt.includeValue) showWarning(exportMessage);
      else showSuccess(exportMessage);
    } catch (reason) {
      showError(reason);
    } finally {
      setBusy(false);
    }
  };

  const chooseImportFile = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    clearToast();
    try {
      const preview = await chooseImport(selectedSession.id);
      if (!preview) {
        showInfo("已取消选择导入文件");
        return;
      }
      setImportPreview(preview);
      setImportConfirmationText("");
    } catch (reason) {
      showError(reason);
    } finally {
      setBusy(false);
    }
  };

  const executeImport = async () => {
    if (!selectedSession || !importPreview || busy) return;
    const preview = importPreview;
    setBusy(true);
    clearToast();
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
        showWarning(
          `已写入 ${result.applied.length} 项；“${result.failed.item.name}”失败：${result.failed.error.message}；另有 ${result.remaining} 项未执行${refreshSuffix}`,
        );
      } else {
        const importMessage = `导入完成：已写入 ${result.applied.length} 项，${preview.skipped} 项因不含 value 跳过；脱敏审计已记录${refreshSuffix}`;
        if (refreshSuffix) showWarning(importMessage);
        else showSuccess(importMessage);
      }
    } catch (reason) {
      setImportPreview(undefined);
      showError(reason);
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const loadHistory = async (
    scope: string,
    cursor?: string,
    append = false,
  ) => {
    const generation = historyGeneration.current + 1;
    historyGeneration.current = generation;
    setHistoryLoading(true);
    try {
      const page = await loadAuditHistory(
        scope === "all" ? undefined : scope,
        cursor,
      );
      if (historyGeneration.current !== generation) return;
      setHistoryItems((current) =>
        append ? [...current, ...page.items] : page.items,
      );
      setHistoryCursor(page.nextCursor);
    } catch (reason) {
      if (historyGeneration.current === generation) showError(reason);
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
        showSuccess(
          `诊断包 ${receipt.fileName} 已导出；仅包含 ${receipt.connectionCount} 个连接的聚合计数，不含 endpoint、名称、value 或凭据`,
        );
      }
    } catch (reason) {
      showError(reason);
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
    clearToast();
    const operationId = operations.start("serverHistory");
    try {
      const page = await listResourceHistory(
        selectedSession.id,
        address,
        operationId,
        cursor,
      );
      if (!operations.isCurrent("serverHistory", operationId)) return;
      setServerHistoryItems((current) =>
        append ? [...current, ...page.items] : page.items,
      );
      setServerHistoryCursor(page.nextCursor);
    } catch (reason) {
      if (operations.isCurrent("serverHistory", operationId)) {
        showError(reason);
      }
    } finally {
      const current = operations.isCurrent("serverHistory", operationId);
      operations.finish("serverHistory", operationId);
      if (current) setServerHistoryLoading(false);
    }
  };

  const openServerHistory = () => {
    if (
      !document ||
      document.address.type !== "nacosConfig" ||
      !selectedSession
    )
      return;
    const address = document.address;
    setServerHistoryAddress(address);
    setServerHistoryItems([]);
    setServerHistoryCursor(undefined);
    setServerHistoryDetail(undefined);
    setServerHistoryOpen(true);
    void loadServerHistory(address);
  };

  const readServerHistory = async (entry: ResourceHistoryEntry) => {
    if (!selectedSession || !serverHistoryAddress || serverHistoryLoading)
      return;
    setServerHistoryLoading(true);
    clearToast();
    const operationId = operations.start("serverHistory");
    try {
      const detail = await readResourceHistory(
        selectedSession.id,
        serverHistoryAddress,
        entry.revisionId,
        operationId,
      );
      if (operations.isCurrent("serverHistory", operationId))
        setServerHistoryDetail(detail);
    } catch (reason) {
      if (operations.isCurrent("serverHistory", operationId)) {
        showError(reason);
      }
    } finally {
      const current = operations.isCurrent("serverHistory", operationId);
      operations.finish("serverHistory", operationId);
      if (current) setServerHistoryLoading(false);
    }
  };

  const openNativeInfo = async () => {
    if (!selectedSession || !selectedProfile || !document || busy) return;
    if (
      document.address.type !== "etcd" &&
      document.address.type !== "zookeeper"
    )
      return;
    setNativeInfo(undefined);
    setNativeInfoOpen(true);
    if (
      document.address.type === "etcd" &&
      (!document.metadata.lease || document.metadata.lease === "0")
    ) {
      setNativeInfoLoading(false);
      return;
    }
    setNativeInfoLoading(true);
    setBusy(true);
    clearToast();
    const operationId = startOperation();
    try {
      setNativeInfo(
        await inspectNativeResource(
          selectedSession.id,
          document.address,
          operationId,
        ),
      );
    } catch (reason) {
      setNativeInfoOpen(false);
      if (isCancelled(reason)) showInfo("原生元数据读取已取消");
      else showError(reason);
    } finally {
      finishOperation(operationId);
      setNativeInfoLoading(false);
      setBusy(false);
    }
  };

  const executeLeaseAction = async (action: EtcdLeaseAction) => {
    if (!selectedSession || selectedProfile?.adapter !== "etcd" || busy) return;
    setBusy(true);
    clearToast();
    const operationId = startOperation();
    try {
      const result = await executeEtcdLeaseAction(
        selectedSession.id,
        action,
        operationId,
      );
      await reloadRoot(selectedSession.id);
      if (result.action === "revoke") {
        await stopActiveWatch();
        setNativeInfoOpen(false);
        setNativeInfo(undefined);
        showDocument(undefined);
        setSelectedAddress(undefined);
        showSuccess(
          `Lease ${result.leaseId} 已撤销；关联 key 已过期，脱敏审计已记录`,
        );
        return;
      }
      const refreshed = await runRead(selectedSession.id, action.address);
      showDocument(refreshed);
      setSelectedAddress(action.address);
      if (result.action === "detach") {
        setNativeInfo(undefined);
        showSuccess(
          `Lease ${result.previousLeaseId} 已原子解绑；key 已变为永久，脱敏审计已记录`,
        );
      } else {
        try {
          setNativeInfo(
            await inspectNativeResource(
              selectedSession.id,
              action.address,
              newConnectionId(),
            ),
          );
        } catch {
          setNativeInfo(undefined);
        }
        showSuccess(
          result.action === "keepAlive"
            ? `Lease ${result.leaseId} 已续租一次，剩余 TTL ${result.remainingTtlSeconds} 秒；脱敏审计已记录`
            : `Lease ${result.leaseId} 已原子绑定；脱敏审计已记录`,
        );
      }
    } catch (reason) {
      const message = errorMessage(reason);
      try {
        const refreshed = await runRead(selectedSession.id, action.address);
        showDocument(refreshed);
        setSelectedAddress(action.address);
        if (refreshed.metadata.lease && refreshed.metadata.lease !== "0") {
          setNativeInfo(
            await inspectNativeResource(
              selectedSession.id,
              action.address,
              newConnectionId(),
            ),
          );
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
      showErrorText(
        isOutcomeUnknown(reason)
          ? `${message}；已尽力刷新 key 与 Lease 状态，请核对后再决定下一步`
          : message,
      );
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const executeZookeeperAction = async (action: ZookeeperNativeAction) => {
    if (!selectedSession || selectedProfile?.adapter !== "zookeeper" || busy)
      return;
    setBusy(true);
    clearToast();
    const operationId = startOperation();
    try {
      const result = await executeZookeeperNativeAction(
        selectedSession.id,
        action,
        operationId,
      );
      await reloadRoot(selectedSession.id);
      if (result.action === "create") {
        setPendingZookeeperAction(undefined);
        setZookeeperConfirmation("");
        const created = await runRead(selectedSession.id, result.address);
        showDocument(created);
        setSelectedAddress(result.address);
        const path =
          result.address.type === "zookeeper" ? result.address.path : "新节点";
        showSuccess(`${path} 已原子创建并继承父 ACL；脱敏审计已记录`);
      } else {
        setNativeInfo(
          await inspectNativeResource(
            selectedSession.id,
            result.address,
            newConnectionId(),
          ),
        );
        showSuccess(
          `ACL 已从 aversion ${result.previousAclVersion} 原子更新到 ${result.currentAclVersion}；脱敏审计已记录`,
        );
      }
    } catch (reason) {
      const message = errorMessage(reason);
      try {
        await reloadRoot(selectedSession.id);
        if (action.action === "setAcl") {
          setNativeInfo(
            await inspectNativeResource(
              selectedSession.id,
              action.address,
              newConnectionId(),
            ),
          );
        }
      } catch {
        // Best-effort reconciliation must not hide the original mutation error.
      }
      if (isOutcomeUnknown(reason)) {
        setPendingZookeeperAction(undefined);
        setNativeInfoOpen(false);
      }
      showErrorText(
        isOutcomeUnknown(reason)
          ? `${message}；已刷新资源树，请核对实际路径或 ACL 后再决定下一步`
          : message,
      );
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const openEtcdTransaction = () => {
    if (!selectedSession || selectedProfile?.adapter !== "etcd" || busy) return;
    setEtcdTransactionItems([
      emptyEtcdTransactionItem(),
      emptyEtcdTransactionItem(),
    ]);
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
        if (seen.has(address.keyBase64))
          throw new Error("同一个 etcd key 不能在一次事务中出现两次");
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
      showError(reason);
      return;
    }

    setBusy(true);
    clearToast();
    const operationId = startOperation();
    try {
      const result = await executeEtcdTransaction(
        selectedSession.id,
        transaction,
        operationId,
      );
      setEtcdTransactionOpen(false);
      await reloadRoot(selectedSession.id);
      const selectedResult = document
        ? result.results.find((item) =>
            sameAddress(item.address, document.address),
          )
        : undefined;
      if (selectedResult?.operation === "delete") {
        await stopActiveWatch();
        showDocument(undefined);
        setSelectedAddress(undefined);
      } else if (selectedResult && document) {
        showDocument(await runRead(selectedSession.id, document.address));
      }
      showSuccess(
        `事务已在 revision ${result.revision} 原子提交 ${result.results.length} 项；脱敏审计已记录`,
      );
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
      showErrorText(
        isOutcomeUnknown(reason)
          ? `${message}；已尽力刷新远端状态，请核对所有目标 key 后再决定下一步`
          : message,
      );
    } finally {
      finishOperation(operationId);
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (!selectedSession) return;
    await stopActiveWatch();
    await operations.cancel("serverHistory").catch(() => false);
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
    clearView();
    setExportDialogOpen(false);
    setImportPreview(undefined);
    setServerHistoryOpen(false);
    setNativeInfoOpen(false);
    setNativeInfo(undefined);
    setEtcdTransactionOpen(false);
    setPendingMutation(undefined);
    setPendingZookeeperAction(undefined);
    setNacosNativeOpen(false);
    setCreateDialogOpen(false);
    showSuccess("连接已断开");
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
    if (
      !globalThis.confirm(
        `确定删除连接“${form.name}”吗？系统凭据也会一并删除。`,
      )
    )
      return;
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
        clearView();
        setExportDialogOpen(false);
        setImportPreview(undefined);
        setServerHistoryOpen(false);
        setNativeInfoOpen(false);
        setNativeInfo(undefined);
      }
      setDialogOpen(false);
      setConnectionSecret("");
      showSuccess("连接和系统凭据已删除");
    } catch (reason) {
      showError(reason);
    } finally {
      setBusy(false);
    }
  };

  const watchBackendIsActive = resourceWatch
    ? [
        "starting",
        "live",
        "reconnecting",
        "compacted",
        "sessionExpired",
      ].includes(resourceWatch.state)
    : false;

  return (
    <div className="app">
      <header className="topbar" data-tauri-drag-region="deep">
        <div className="brand">
          <span className="logo">A</span>Atlas Registry
        </div>
        <span className="release-tag">SAFE-WRITE ALPHA</span>
        <div className="top-spacer" data-tauri-drag-region />
        <div className={`runtime ${capabilities ? "" : "pending"}`}>
          <span className="status-dot" />
          {capabilities
            ? `Rust Core · ${capabilities.length} adapters`
            : "正在启动 Rust Core…"}
        </div>
        <button
          className="button update-button"
          disabled={checkingUpdate || installingUpdate}
          onClick={() => void checkForUpdates()}
        >
          {checkingUpdate ? "检查中…" : "⇩ 更新"}
        </button>
        <button
          className="button"
          disabled={checkingUpdate || installingUpdate}
          onClick={() => setSettingsOpen(true)}
        >
          ⚙ 设置
        </button>
        <button
          className="button"
          disabled={busy}
          onClick={() => void exportDiagnostics()}
        >
          诊断包
        </button>
        <button className="button" onClick={openHistory}>
          历史
        </button>
        <button className="button primary" onClick={openNewConnection}>
          ＋ 新建连接
        </button>
      </header>

      <div
        className="shell"
        data-connections={panelLayout.connections}
        data-resources={panelLayout.resources}
      >
        <aside className="connections" aria-label="连接">
          <button
            className="panel-toggle"
            aria-controls="connections-panel-content"
            aria-expanded={connectionsExpanded}
            aria-label={connectionsExpanded ? "收起连接栏" : "展开连接栏"}
            title={connectionsExpanded ? "收起连接栏" : "展开连接栏"}
            onClick={() => toggleNavigationPanel("connections")}
          >
            <span aria-hidden="true">{connectionsExpanded ? "‹" : "›"}</span>
          </button>
          {!connectionsExpanded && (
            <span className="panel-rail-label" aria-hidden="true">
              连接
            </span>
          )}
          <div
            id="connections-panel-content"
            className="panel-content"
            hidden={!connectionsExpanded}
          >
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
                disabled={busy}
                onClick={() => void selectProfile(profile)}
              >
                <span
                  className={`status-dot ${sessions[profile.id] ? "" : "offline"}`}
                />
                <span>
                  <b>{profile.name}</b>
                  <small>
                    {profile.endpoint} ·{" "}
                    {connectionEnvironmentLabels[profile.environment]}
                  </small>
                </span>
                <span className={`badge ${profile.adapter}`}>
                  {connectionLabel(profile.adapter)}
                </span>
              </button>
            ))}

            {selectedProfile && !selectedSession && (
              <button
                className="button primary wide"
                disabled={busy}
                onClick={() => void connectAndLoad(selectedProfile)}
              >
                {busy ? "连接中…" : "连接并浏览"}
              </button>
            )}
            {selectedSession && (
              <button className="button wide" onClick={() => void disconnect()}>
                断开连接
              </button>
            )}
            {selectedProfile && (
              <div className="connection-actions">
                <button
                  className="button"
                  disabled={busy}
                  onClick={openEditConnection}
                >
                  编辑
                </button>
                <button
                  className="button"
                  disabled={busy}
                  onClick={openCopyConnection}
                >
                  复制
                </button>
              </div>
            )}
            <button className="button wide" onClick={openNewConnection}>
              ＋ 添加连接
            </button>

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
          </div>
        </aside>

        <section className="tree" aria-label="资源">
          <button
            className="panel-toggle"
            aria-controls="resources-panel-content"
            aria-expanded={resourcesExpanded}
            aria-label={resourcesExpanded ? "收起资源栏" : "展开资源栏"}
            title={resourcesExpanded ? "收起资源栏" : "展开资源栏"}
            onClick={() => toggleNavigationPanel("resources")}
          >
            <span aria-hidden="true">{resourcesExpanded ? "‹" : "›"}</span>
          </button>
          {!resourcesExpanded && (
            <span className="panel-rail-label" aria-hidden="true">
              资源
            </span>
          )}
          <div
            id="resources-panel-content"
            className="panel-content"
            hidden={!resourcesExpanded}
          >
            <div className="tree-header">
              <b>{selectedProfile?.name ?? "资源"}</b>
              <button
                className="icon-button import-resource"
                disabled={!selectedSession || busy}
                onClick={() => void chooseImportFile()}
                title="从 Atlas JSON 导入"
              >
                ⇧
              </button>
              {selectedProfile?.adapter === "etcd" && (
                <button
                  className="icon-button transaction-resource"
                  disabled={!selectedSession || busy}
                  onClick={openEtcdTransaction}
                  title="etcd 原子批量事务"
                >
                  T
                </button>
              )}
              {selectedProfile?.adapter === "nacos" && (
                <button
                  className="icon-button transaction-resource"
                  disabled={!selectedSession || busy}
                  onClick={() => setNacosNativeOpen(true)}
                  title="Nacos 命名空间、服务与实例管理"
                >
                  N
                </button>
              )}
              <button
                className="icon-button create-resource"
                disabled={!selectedSession || busy}
                onClick={openCreateResource}
                title="新建资源"
              >
                ＋
              </button>
              <button
                className="icon-button"
                disabled={!selectedSession || busy}
                onClick={() => void refreshRoot()}
                title="刷新"
              >
                ↻
              </button>
              <input
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
                placeholder="筛选当前已加载资源…"
              />
              <div className="resource-query">
                <input
                  value={resourceQuery}
                  disabled={!selectedSession || busy}
                  onChange={(event) => setResourceQuery(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void searchCurrentScope();
                  }}
                  placeholder={
                    selectedProfile?.adapter === "nacos"
                      ? "搜索 dataId；定位请填 GROUP / dataId"
                      : selectedProfile?.adapter === "zookeeper"
                        ? "搜索节点名；定位请填 /绝对路径"
                        : "搜索 key；定位可填 key 或 base64:…"
                  }
                />
                <button
                  className="button"
                  disabled={!selectedSession || busy}
                  onClick={() => void searchCurrentScope()}
                >
                  搜索
                </button>
                <button
                  className="button"
                  disabled={!selectedSession || busy}
                  onClick={() => void locateResource()}
                >
                  定位
                </button>
              </div>
              {activeSearch && (
                <div className="search-state">
                  <span>
                    “{activeSearch.query}” · 已检查 {activeSearch.scanned}{" "}
                    个标识
                    {activeSearch.exhaustive ? " · 已完成" : ""}
                  </span>
                  <button disabled={busy} onClick={() => void exitSearch()}>
                    返回资源树
                  </button>
                </div>
              )}
            </div>

            {!selectedSession && (
              <div className="empty">
                <span className="empty-icon">◇</span>
                <b>选择并打开连接</b>
                <span>资源会按需加载，不会扫描整个集群。</span>
              </div>
            )}
            {selectedSession && rows.length === 0 && !busy && (
              <div className="empty">
                <span className="empty-icon">∅</span>
                <b>{activeSearch ? "没有匹配的资源" : "当前范围没有资源"}</b>
                <span>
                  {activeSearch
                    ? "可调整标识关键词，搜索不会读取资源值。"
                    : "可以刷新，或检查所选 namespace 和权限。"}
                </span>
              </div>
            )}
            {visibleRows.map((row) => {
              const actualIndex = rows.indexOf(row);
              if (row.kind === "more") {
                return (
                  <button
                    className="node load-more"
                    style={{ paddingLeft: 14 + row.depth * 20 }}
                    key={`more-${row.cursor}`}
                    onClick={() => void loadMore(actualIndex, row)}
                  >
                    … 加载更多
                  </button>
                );
              }
              const selected =
                selectedAddress &&
                JSON.stringify(selectedAddress) ===
                  JSON.stringify(row.node.address);
              return (
                <button
                  className={`node ${selected ? "active" : ""}`}
                  style={{ paddingLeft: 14 + row.depth * 20 }}
                  key={`${row.depth}-${row.node.name}-${JSON.stringify(row.node.address)}`}
                  onClick={() => void openResource(actualIndex, row)}
                >
                  <span className="disclosure">
                    {row.node.hasChildren === false
                      ? ""
                      : row.expanded
                        ? "⌄"
                        : "›"}
                  </span>
                  <span className={row.node.readable ? "key" : "folder"}>
                    {row.node.readable ? "◇" : "◆"}
                  </span>
                  <span className="node-name">{row.node.name}</span>
                </button>
              );
            })}
            {busy && (
              <div className="loading-line">
                正在与注册中心通信…{" "}
                {activeOperation && (
                  <button onClick={() => void cancelActiveOperation()}>
                    取消
                  </button>
                )}
              </div>
            )}
          </div>
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
              <div className="breadcrumb">
                {selectedProfile?.name} /{" "}
                <b>{addressLabel(document.address)}</b>
              </div>
              <div className="detail-title">
                <div>
                  <span className="eyebrow">RESOURCE</span>
                  <h1>{document.name}</h1>
                </div>
                <div className="actions">
                  {document.address.type === "nacosConfig" && (
                    <button
                      className="button"
                      disabled={busy}
                      onClick={openServerHistory}
                    >
                      服务端历史
                    </button>
                  )}
                  {document.address.type === "nacosConfig" && (
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => setNacosNativeOpen(true)}
                    >
                      服务管理
                    </button>
                  )}
                  {document.address.type === "etcd" && (
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => void openNativeInfo()}
                    >
                      Lease
                    </button>
                  )}
                  {document.address.type === "zookeeper" && (
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => void openNativeInfo()}
                    >
                      ACL
                    </button>
                  )}
                  <button
                    className="button"
                    disabled={busy}
                    onClick={openExportDialog}
                  >
                    导出
                  </button>
                  <button
                    className="button danger"
                    disabled={busy || !document.version}
                    onClick={prepareDelete}
                  >
                    删除
                  </button>
                  <button
                    className="button primary"
                    disabled={
                      busy ||
                      !document.version ||
                      draftValue === document.value.content
                    }
                    onClick={prepareUpdate}
                  >
                    保存变更
                  </button>
                </div>
              </div>
              <div className="stats">
                <div>
                  <span>版本</span>
                  <strong>{document.version || "—"}</strong>
                </div>
                <div>
                  <span>编码</span>
                  <strong>{document.value.encoding.toUpperCase()}</strong>
                </div>
                <div>
                  <span>大小</span>
                  <strong>{document.value.sizeBytes.toLocaleString()} B</strong>
                </div>
              </div>
              <div
                className={`watch-panel ${resourceWatch?.state ?? "idle"} ${resourceWatch?.remoteChanged ? "changed" : ""}`}
              >
                <div className="watch-summary">
                  <span className="watch-pulse" />
                  <div>
                    <b>
                      {resourceWatch
                        ? watchStatusLabels[resourceWatch.state]
                        : "实时监听未开启"}
                    </b>
                    <span>
                      {resourceWatch?.message
                        ? `${resourceWatch.message}${resourceWatch.retryInMs ? ` · ${resourceWatch.retryInMs} ms 后重试` : ""}`
                        : resourceWatch?.lastChange
                          ? `${watchChangeLabels[resourceWatch.lastChange.change]} · 版本 ${resourceWatch.lastChange.version ?? "未知"}`
                          : "监听事件只包含地址、类型和版本，不传输资源值"}
                    </span>
                  </div>
                  {resourceWatch && (
                    <span className="watch-count">
                      {resourceWatch.changeCount} 次变化
                    </span>
                  )}
                </div>
                <div className="watch-actions">
                  {resourceWatch?.remoteChanged && (
                    <button
                      className="button primary"
                      disabled={busy}
                      onClick={() => void refreshWatchedResource()}
                    >
                      读取最新版本
                    </button>
                  )}
                  <button
                    className="button"
                    disabled={busy}
                    onClick={() =>
                      void (watchBackendIsActive
                        ? stopResourceWatch()
                        : startResourceWatch())
                    }
                  >
                    {watchBackendIsActive
                      ? "停止监听"
                      : resourceWatch
                        ? "重新监听"
                        : "开始监听"}
                  </button>
                </div>
              </div>
              {document.value.encoding === "base64" && (
                <div className="binary-warning">
                  该值不是有效 UTF-8，已使用 Base64 展示，内容没有被替换或损坏。
                </div>
              )}
              <div className="editor-header">
                <span>{document.contentType?.toUpperCase() || "TEXT"}</span>
                <span>
                  {draftValue === document.value.content
                    ? document.value.encoding.toUpperCase()
                    : `${document.value.encoding.toUpperCase()} · 已修改`}
                </span>
              </div>
              <textarea
                value={draftValue}
                disabled={busy}
                onChange={(event) => setDraftValue(event.target.value)}
                spellCheck={false}
              />
              <div className="metadata">
                {Object.entries(document.metadata).map(([name, value]) => (
                  <div className="metadata-row" key={name}>
                    <span>{name}</span>
                    <b>{value || "—"}</b>
                  </div>
                ))}
              </div>
            </>
          )}
        </main>
      </div>

      {toast && <Toast key={toast.id} toast={toast} onDismiss={dismissToast} />}

      {settingsOpen && (
        <SettingsDialog
          settings={updateProxySettings}
          onSave={(settings) => {
            const saved = saveUpdateProxySettings(settings);
            setUpdateProxySettings(saved);
            setSettingsOpen(false);
            showSuccess("更新网络设置已保存");
          }}
          onCancel={() => setSettingsOpen(false)}
        />
      )}

      {availableUpdate && (
        <UpdateDialog
          update={availableUpdate}
          installing={installingUpdate}
          progress={updateProgress}
          onInstall={() => void installAvailableUpdate()}
          onClose={() => setAvailableUpdate(undefined)}
        />
      )}

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
          onLoadMore={() =>
            void loadServerHistory(
              serverHistoryAddress,
              serverHistoryCursor,
              true,
            )
          }
          onBack={() => setServerHistoryDetail(undefined)}
          onCancelOperation={() => void cancelActiveOperation()}
          onClose={() => setServerHistoryOpen(false)}
        />
      )}

      {nacosNativeOpen &&
        selectedProfile?.adapter === "nacos" &&
        selectedSession && (
          <NacosNativeDialog
            profile={selectedProfile}
            connectionId={selectedSession.id}
            onMessage={showSuccess}
            onClose={() => setNacosNativeOpen(false)}
          />
        )}

      {nativeInfoOpen &&
        selectedProfile?.adapter === "etcd" &&
        document?.address.type === "etcd" && (
          <EtcdLeaseDialog
            key={
              nativeInfo?.kind === "etcdLease" ? nativeInfo.leaseId : "unbound"
            }
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

      {nativeInfoOpen &&
        selectedProfile?.adapter === "zookeeper" &&
        document?.address.type === "zookeeper" && (
          <ZookeeperAclDialog
            key={
              nativeInfo?.kind === "zookeeperAcl"
                ? nativeInfo.aclVersion
                : "loading"
            }
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
