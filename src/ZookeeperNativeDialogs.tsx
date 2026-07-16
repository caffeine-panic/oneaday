import { useEffect, useMemo, useState } from "react";
import { connectionEnvironmentLabels } from "./registry";
import type {
  ConnectionProfile,
  NativeResourceInfo,
  ResourceDocument,
  ZookeeperAclEntry,
  ZookeeperAclPermission,
  ZookeeperNativeAction,
} from "./registry";

type AclDialogProps = {
  profile: ConnectionProfile;
  document: ResourceDocument;
  info?: Extract<NativeResourceInfo, { kind: "zookeeperAcl" }>;
  loading: boolean;
  busy: boolean;
  onExecute: (action: Extract<ZookeeperNativeAction, { action: "setAcl" }>) => void;
  onCancelOperation: () => void;
  onClose: () => void;
};

const permissions: Array<{ id: ZookeeperAclPermission; label: string }> = [
  { id: "read", label: "读取" },
  { id: "write", label: "写入" },
  { id: "create", label: "创建子节点" },
  { id: "delete", label: "删除子节点" },
  { id: "admin", label: "管理 ACL" },
];

export function ZookeeperAclDialog({
  profile,
  document,
  info,
  loading,
  busy,
  onExecute,
  onCancelOperation,
  onClose,
}: AclDialogProps) {
  const [entries, setEntries] = useState<ZookeeperAclEntry[]>([]);
  const [confirmation, setConfirmation] = useState("");

  useEffect(() => {
    setEntries(info?.entries.map((entry) => ({ ...entry, permissions: [...entry.permissions] })) ?? []);
    setConfirmation("");
  }, [info?.aclVersion]);

  const validation = useMemo(() => validateAcl(entries), [entries]);
  const changed = Boolean(info) && JSON.stringify(entries) !== JSON.stringify(info?.entries);
  const canSubmit = Boolean(info)
    && changed
    && !validation
    && !loading
    && !busy
    && confirmation === profile.name;

  const changeEntry = (index: number, patch: Partial<ZookeeperAclEntry>) => {
    setEntries((current) => current.map((entry, itemIndex) => (
      itemIndex === index ? { ...entry, ...patch } : entry
    )));
  };

  const togglePermission = (index: number, permission: ZookeeperAclPermission) => {
    const entry = entries[index];
    if (!entry) return;
    const next = entry.permissions.includes(permission)
      ? entry.permissions.filter((item) => item !== permission)
      : [...entry.permissions, permission];
    changeEntry(index, { permissions: next });
  };

  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!busy) onClose(); }}>
      <section className="dialog native-info-dialog zookeeper-acl-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">ZOOKEEPER ACL</span><h2>ACL 条件变更</h2></div>
          <button className="icon-button" disabled={busy} onClick={onClose}>×</button>
        </div>

        {loading && !info && <div className="loading-line">正在读取当前 ACL 与 aversion…</div>}
        {info && <div className="native-summary"><span>当前 ACL 版本</span><b>{info.aclVersion}</b><small>{info.entries.length} 条规则</small></div>}

        <div className="impact-grid">
          <span>环境</span><b>{connectionEnvironmentLabels[profile.environment]}</b>
          <span>Endpoint</span><b>{profile.endpoint}</b>
          <span>ZNode</span><b>{document.address.type === "zookeeper" ? document.address.path : "—"}</b>
          <span>一致性</span><b>服务端 aversion 原子条件写</b>
        </div>

        <div className="acl-editor-list">
          {entries.map((entry, index) => (
            <article className="acl-editor-entry" key={index}>
              <div className="form-grid equal">
                <label>Scheme<input value={entry.scheme} disabled={busy} onChange={(event) => changeEntry(index, { scheme: event.target.value })} placeholder="world / auth / digest" /></label>
                <label>Identity<input value={entry.id} disabled={busy} onChange={(event) => changeEntry(index, { id: event.target.value })} placeholder="anyone / user:hash" /></label>
              </div>
              <div className="acl-permission-editor">
                {permissions.map((permission) => (
                  <label key={permission.id}>
                    <input type="checkbox" checked={entry.permissions.includes(permission.id)} disabled={busy} onChange={() => togglePermission(index, permission.id)} />
                    {permission.label}
                  </label>
                ))}
                <button className="button danger compact-button" disabled={busy || entries.length <= 1} onClick={() => setEntries((current) => current.filter((_, itemIndex) => itemIndex !== index))}>移除</button>
              </div>
            </article>
          ))}
        </div>
        <button className="button" disabled={busy || entries.length >= 256} onClick={() => setEntries((current) => [...current, { scheme: "digest", id: "", permissions: ["read"] }])}>＋ 添加 ACL 身份</button>

        {validation && <div className="mutation-warning danger-warning">{validation}</div>}
        <div className="mutation-warning">ACL 错配可能立即使当前连接失去访问权。客户端强制保留至少一个 ADMIN 身份，但无法判断该身份是否属于当前操作者。</div>
        <label className="production-confirmation">确认以 aversion {info?.aclVersion ?? "—"} 提交，请输入当前连接名 <b>{profile.name}</b>。
          <input value={confirmation} disabled={busy} onChange={(event) => setConfirmation(event.target.value)} placeholder={profile.name} />
        </label>
        <p className="form-note">审计只记录目标、ACL 条目数和版本，不保存 znode 原文。服务端返回 BadVersion 时不会覆盖他人修改。</p>
        <div className="dialog-actions">
          <button className="button" onClick={busy ? onCancelOperation : onClose}>{busy ? "取消请求" : "关闭"}</button>
          <button className="button danger" disabled={!canSubmit} onClick={() => {
            if (!info) return;
            onExecute({
              action: "setAcl",
              address: document.address,
              expectedAclVersion: info.aclVersion,
              entries: entries.map((entry) => ({
                scheme: entry.scheme.trim(),
                id: entry.id.trim(),
                permissions: entry.permissions,
              })),
            });
          }}>{busy ? "正在提交…" : "确认更新 ACL"}</button>
        </div>
      </section>
    </div>
  );
}

type CreateConfirmationProps = {
  profile: ConnectionProfile;
  action: Extract<ZookeeperNativeAction, { action: "create" }>;
  confirmation: string;
  busy: boolean;
  onConfirmationChange: (value: string) => void;
  onConfirm: () => void;
  onCancelOperation: () => void;
  onClose: () => void;
};

const createModeLabels = {
  persistentSequential: "持久顺序节点",
  ephemeral: "临时节点",
  ephemeralSequential: "临时顺序节点",
};

export function ZookeeperCreateConfirmationDialog({
  profile,
  action,
  confirmation,
  busy,
  onConfirmationChange,
  onConfirm,
  onCancelOperation,
  onClose,
}: CreateConfirmationProps) {
  const path = action.address.type === "zookeeper" ? action.address.path : "—";
  const bytes = action.value.encoding === "utf8"
    ? new TextEncoder().encode(action.value.content).byteLength
    : action.value.content.length;
  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!busy) onClose(); }}>
      <section className="dialog mutation-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">ZOOKEEPER NATIVE CREATE</span><h2>确认创建{createModeLabels[action.mode]}</h2></div>
          <button className="icon-button" disabled={busy} onClick={onClose}>×</button>
        </div>
        <div className="impact-grid">
          <span>环境</span><b>{connectionEnvironmentLabels[profile.environment]}</b>
          <span>Endpoint</span><b>{profile.endpoint}</b>
          <span>请求路径</span><b>{path}</b>
          <span>节点模式</span><b>{createModeLabels[action.mode]}</b>
          <span>ACL</span><b>继承父节点</b>
          <span>新值大小</span><b>{bytes.toLocaleString()} B</b>
        </div>
        {action.mode !== "persistentSequential" && <div className="mutation-warning">临时节点属于当前桌面连接的 ZooKeeper session。断开连接或 session 过期后，服务端会自动删除它。</div>}
        {action.mode.endsWith("Sequential") && <p className="form-note">请求路径是前缀；服务端会追加十位或十九位序号，提交后界面将定位到实际路径。</p>}
        <label className="production-confirmation">所有写入均需二次确认。请输入当前连接名 <b>{profile.name}</b>。
          <input value={confirmation} disabled={busy} onChange={(event) => onConfirmationChange(event.target.value)} placeholder={profile.name} />
        </label>
        <p className="form-note">写入会记录脱敏审计，只保存版本、编码、大小与 SHA-256，不保存资源原文。</p>
        <div className="dialog-actions">
          <button className="button" onClick={busy ? onCancelOperation : onClose}>{busy ? "取消请求" : "返回"}</button>
          <button className="button primary" disabled={busy || confirmation !== profile.name} onClick={onConfirm}>{busy ? "正在提交…" : "确认原子创建"}</button>
        </div>
      </section>
    </div>
  );
}

function validateAcl(entries: ZookeeperAclEntry[]) {
  if (entries.length === 0) return "至少需要一条 ACL 规则。";
  const identities = new Set<string>();
  let hasAdmin = false;
  for (const entry of entries) {
    const scheme = entry.scheme.trim();
    const id = entry.id.trim();
    if (!scheme) return "每条 ACL 都需要 Scheme。";
    const identity = `${scheme}\u0000${id}`;
    if (identities.has(identity)) return `身份 ${scheme}:${id || "（空）"} 重复。`;
    identities.add(identity);
    if (entry.permissions.length === 0) return `身份 ${scheme}:${id || "（空）"} 没有任何权限。`;
    hasAdmin ||= entry.permissions.includes("admin");
  }
  return hasAdmin ? undefined : "至少保留一个“管理 ACL（ADMIN）”身份，避免 ACL 永久失控。";
}
