import { useEffect, useMemo, useState } from "react";
import {
  MutationConfirmationDialog,
  NewResourceDialog,
  type NewResourceDraft,
} from "./MutationDialogs";
import {
  ROOT_ADDRESS,
  cancelOperation,
  closeConnection,
  errorMessage,
  isCancelled,
  isOutcomeUnknown,
  listResources,
  loadConnectionProfiles,
  newConnectionId,
  mutateResource,
  openConnection,
  probeConnection,
  readResource,
  registryCapabilities,
  saveConnectionProfiles,
  type AdapterDescriptor,
  type AdapterId,
  type ConnectionProfile,
  type ConnectionSession,
  type ResourceAddress,
  type ResourceDocument,
  type ResourceNode,
  type ResourceMutation,
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
};

type TreeRow = ResourceRow | MoreRow;

const emptyForm = (): ConnectionProfile => ({
  id: newConnectionId(),
  name: "",
  adapter: "etcd",
  endpoint: "127.0.0.1:2379",
  namespace: "",
  nacosApiVersion: "v2",
});

const endpointPlaceholders: Record<AdapterId, string> = {
  etcd: "127.0.0.1:2379 或 etcd-1:2379,etcd-2:2379",
  zookeeper: "127.0.0.1:2181 或 zk-1:2181,zk-2:2181/app",
  nacos: "127.0.0.1:8848",
};

const emptyResourceDraft = (adapter: AdapterId): NewResourceDraft => ({
  keyOrPath: adapter === "zookeeper" ? "/" : "",
  group: "DEFAULT_GROUP",
  dataId: "",
  content: "",
  contentType: "text",
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

function connectionLabel(adapter: AdapterId) {
  return adapter === "zookeeper" ? "ZK" : adapter;
}

function normalizedProfile(profile: ConnectionProfile): ConnectionProfile {
  return {
    ...profile,
    name: profile.name.trim(),
    endpoint: profile.endpoint.trim(),
    namespace: profile.namespace.trim(),
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

function utf8Base64(value: string) {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
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
  const [busy, setBusy] = useState(false);
  const [activeOperation, setActiveOperation] = useState<string>();
  const [message, setMessage] = useState<string>();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [testingConnection, setTestingConnection] = useState(false);
  const [form, setForm] = useState<ConnectionProfile>(emptyForm);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [resourceDraft, setResourceDraft] = useState<NewResourceDraft>(() => emptyResourceDraft("etcd"));
  const [pendingMutation, setPendingMutation] = useState<ResourceMutation>();
  const [confirmationText, setConfirmationText] = useState("");

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

  const showDocument = (nextDocument: ResourceDocument | undefined) => {
    setDocument(nextDocument);
    setDraftValue(nextDocument?.value.content ?? "");
  };

  const reloadRoot = async (connectionId: string) => {
    const page = await runList(connectionId, ROOT_ADDRESS);
    setRows(pageRows(page.items, 0, page.parent, page.nextCursor));
  };

  const cancelActiveOperation = async () => {
    if (!activeOperation) return;
    try {
      await cancelOperation(activeOperation);
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  };

  const connectAndLoad = async (profile: ConnectionProfile) => {
    setBusy(true);
    setMessage(undefined);
    showDocument(undefined);
    setRows([]);
    try {
      const operationId = startOperation();
      let session: ConnectionSession;
      try {
        session = await openConnection(profile, operationId);
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
    const nextProfiles = [...profiles.filter((item) => item.id !== candidate.id), candidate];
    try {
      await saveConnectionProfiles(nextProfiles);
      setProfiles(nextProfiles);
      setDialogOpen(false);
      await connectAndLoad(candidate);
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
      const result = await probeConnection(candidate, operationId);
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
    setSelectedId(profile.id);
    setRows([]);
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

  const openResource = async (index: number, row: ResourceRow) => {
    if (!selectedSession || busy) return;
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
      if (!path.startsWith("/") || path === "/") {
        setMessage("ZooKeeper 路径必须以 / 开头且不能是根节点");
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
        showDocument(undefined);
        setSelectedAddress(undefined);
      } else {
        const refreshed = await runRead(selectedSession.id, result.address);
        showDocument(refreshed);
        setSelectedAddress(result.address);
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

  const disconnect = async () => {
    if (!selectedSession) return;
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
    showDocument(undefined);
    setPendingMutation(undefined);
    setCreateDialogOpen(false);
    setMessage("连接已断开");
  };

  const openNewConnection = () => {
    setForm(emptyForm());
    setDialogOpen(true);
  };

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
              <span><b>{profile.name}</b><small>{profile.endpoint}</small></span>
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
            <button className="icon-button create-resource" disabled={!selectedSession || busy} onClick={openCreateResource} title="新建资源">＋</button>
            <button className="icon-button" disabled={!selectedSession || busy} onClick={() => void refreshRoot()} title="刷新">↻</button>
            <input value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="筛选当前已加载资源…" />
          </div>

          {!selectedSession && (
            <div className="empty"><span className="empty-icon">◇</span><b>选择并打开连接</b><span>资源会按需加载，不会扫描整个集群。</span></div>
          )}
          {selectedSession && rows.length === 0 && !busy && (
            <div className="empty"><span className="empty-icon">∅</span><b>当前范围没有资源</b><span>可以刷新，或检查所选 namespace 和权限。</span></div>
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
                  <button className="button danger" disabled={busy || !document.version} onClick={prepareDelete}>删除</button>
                  <button className="button primary" disabled={busy || !document.version || draftValue === document.value.content} onClick={prepareUpdate}>保存变更</button>
                </div>
              </div>
              <div className="stats">
                <div><span>版本</span><strong>{document.version || "—"}</strong></div>
                <div><span>编码</span><strong>{document.value.encoding.toUpperCase()}</strong></div>
                <div><span>大小</span><strong>{document.value.sizeBytes.toLocaleString()} B</strong></div>
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
        <div className="dialog-backdrop" onMouseDown={() => { if (!testingConnection) setDialogOpen(false); }}>
          <section className="dialog" onMouseDown={(event) => event.stopPropagation()}>
            <div className="dialog-heading"><div><span className="eyebrow">CONNECTION</span><h2>新建连接</h2></div><button className="icon-button" disabled={testingConnection} onClick={() => setDialogOpen(false)}>×</button></div>
            <label>类型
              <select value={form.adapter} onChange={(event) => {
                const adapter = event.target.value as AdapterId;
                setForm((current) => ({ ...current, adapter, endpoint: endpointPlaceholders[adapter].split(" ")[0] }));
              }}>
                <option value="etcd">etcd</option>
                <option value="zookeeper">ZooKeeper</option>
                <option value="nacos">Nacos</option>
              </select>
            </label>
            <label>名称<input autoFocus value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="例如：生产配置中心" /></label>
            <label>Endpoint<input value={form.endpoint} onChange={(event) => setForm({ ...form, endpoint: event.target.value })} placeholder={endpointPlaceholders[form.adapter]} /></label>
            {form.adapter === "nacos" && (
              <div className="form-grid">
                <label>Namespace<input value={form.namespace} onChange={(event) => setForm({ ...form, namespace: event.target.value })} placeholder="public" /></label>
                <label>Admin API<select value={form.nacosApiVersion} onChange={(event) => setForm({ ...form, nacosApiVersion: event.target.value as "v2" | "v3" })}><option value="v2">Nacos 2.x</option><option value="v3">Nacos 3.x</option></select></label>
              </div>
            )}
            <p className="form-note">当前切片支持无认证连接。凭据与 TLS 将进入系统安全存储，不会写入浏览器 localStorage。</p>
            <div className="dialog-actions">
              <button className="button" onClick={() => testingConnection ? void cancelActiveOperation() : setDialogOpen(false)}>{testingConnection ? "取消测试" : "取消"}</button>
              <button className="button" disabled={busy} onClick={() => void testConnection()}>测试连接</button>
              <button className="button primary" disabled={busy} onClick={() => void saveAndConnect()}>保存并连接</button>
            </div>
          </section>
        </div>
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
    </div>
  );
}
