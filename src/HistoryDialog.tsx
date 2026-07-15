import type {
  AuditHistoryItem,
  ConnectionProfile,
  ResourceAddress,
  ResourceSnapshot,
} from "./registry";

type HistoryDialogProps = {
  profiles: ConnectionProfile[];
  scope: string;
  items: AuditHistoryItem[];
  nextCursor?: string;
  loading: boolean;
  onScopeChange: (scope: string) => void;
  onLoadMore: () => void;
  onClose: () => void;
};

const kindLabels: Record<AuditHistoryItem["kind"], string> = {
  started: "已开始",
  applied: "已应用",
  failed: "失败",
  outcomeUnknown: "结果未知",
};

const operationLabels = {
  create: "创建",
  update: "更新",
  delete: "删除",
} as const;

export function HistoryDialog({
  profiles,
  scope,
  items,
  nextCursor,
  loading,
  onScopeChange,
  onLoadMore,
  onClose,
}: HistoryDialogProps) {
  const profileNames = new Map(profiles.map((profile) => [profile.id, profile.name]));
  return (
    <div className="dialog-backdrop" onMouseDown={onClose}>
      <section className="dialog history-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">LOCAL AUDIT HISTORY</span><h2>变更历史</h2></div>
          <button className="icon-button" onClick={onClose}>×</button>
        </div>
        <div className="history-toolbar">
          <label>连接范围
            <select value={scope} disabled={loading} onChange={(event) => onScopeChange(event.target.value)}>
              <option value="all">全部连接</option>
              {profiles.map((profile) => <option value={profile.id} key={profile.id}>{profile.name}</option>)}
            </select>
          </label>
          <span>倒序分页 · 每次最多读取 512 KiB · 永不返回 value</span>
        </div>
        <div className="history-list">
          {!loading && items.length === 0 && (
            <div className="empty compact"><b>暂无本地变更记录</b><span>成功、失败和结果未知的写入会出现在这里。</span></div>
          )}
          {items.map((item, index) => (
            <article className={`history-item ${item.kind}`} key={`${item.operationId}-${item.kind}-${item.timestampMs}-${index}`}>
              <div className="history-item-heading">
                <span className="history-kind">{kindLabels[item.kind]}</span>
                <b>{item.operation ? operationLabels[item.operation] : "写入流程"}</b>
                <time>{new Date(item.timestampMs).toLocaleString("zh-CN")}</time>
              </div>
              <div className="history-target">
                <span>{profileNames.get(item.connectionId) ?? item.connectionId}</span>
                <b>{item.address ? addressText(item.address) : `操作 ${item.operationId}`}</b>
              </div>
              <div className="history-details">
                {item.expectedVersion && <span>期望版本 {item.expectedVersion}</span>}
                {item.consistency && <span>{item.consistency === "atomic" ? "原子条件" : "检查后变更"}</span>}
                {item.errorCode && <span>错误 {item.errorCode}</span>}
                {snapshotText(item.previous, "变更前")}
                {snapshotText(item.current, "变更后")}
              </div>
            </article>
          ))}
          {loading && <div className="loading-line">正在读取本地脱敏审计…</div>}
        </div>
        <div className="dialog-actions split-actions">
          <span className="form-note history-note">日志只展示版本、大小、编码和 SHA-256 摘要。</span>
          <div className="action-group">
            {nextCursor && <button className="button" disabled={loading} onClick={onLoadMore}>加载更早记录</button>}
            <button className="button primary" onClick={onClose}>完成</button>
          </div>
        </div>
      </section>
    </div>
  );
}

function snapshotText(snapshot: ResourceSnapshot | undefined, label: string) {
  if (!snapshot) return null;
  return (
    <span>{label} {snapshot.sizeBytes.toLocaleString()} B · {snapshot.encoding.toUpperCase()} · {snapshot.sha256.slice(0, 12)}…</span>
  );
}

function addressText(address: ResourceAddress) {
  switch (address.type) {
    case "root": return "/";
    case "etcd": return decodeEtcdKey(address.keyBase64);
    case "etcdPrefix": return `prefix base64:${address.prefixBase64}`;
    case "zookeeper": return address.path;
    case "nacosConfig": return `${address.group} / ${address.dataId}`;
  }
}

function decodeEtcdKey(encoded: string) {
  try {
    const bytes = Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0));
    return new TextDecoder(undefined, { fatal: true }).decode(bytes);
  } catch {
    return `base64:${encoded}`;
  }
}
