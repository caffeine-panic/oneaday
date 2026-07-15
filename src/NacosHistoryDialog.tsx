import type {
  ResourceHistoryDocument,
  ResourceHistoryEntry,
} from "./registry";

type NacosHistoryDialogProps = {
  resourceName: string;
  items: ResourceHistoryEntry[];
  nextCursor?: string;
  detail?: ResourceHistoryDocument;
  loading: boolean;
  onRead: (entry: ResourceHistoryEntry) => void;
  onLoadMore: () => void;
  onBack: () => void;
  onCancelOperation: () => void;
  onClose: () => void;
};

export function NacosHistoryDialog({
  resourceName,
  items,
  nextCursor,
  detail,
  loading,
  onRead,
  onLoadMore,
  onBack,
  onCancelOperation,
  onClose,
}: NacosHistoryDialogProps) {
  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!loading) onClose(); }}>
      <section className="dialog server-history-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">NACOS SERVER HISTORY</span><h2>{resourceName}</h2></div>
          <button className="icon-button" disabled={loading} onClick={onClose}>×</button>
        </div>
        {detail ? (
          <div className="history-detail">
            <div className="history-detail-heading">
              <button className="button" disabled={loading} onClick={onBack}>← 返回历史列表</button>
              <span>Revision {detail.entry.revisionId}</span>
            </div>
            <HistoryMetadata entry={detail.entry} />
            {detail.value.encoding === "base64" && <div className="binary-warning">该历史值不是有效 UTF-8，已用 Base64 无损展示。</div>}
            <textarea value={detail.value.content} readOnly spellCheck={false} />
            <p className="form-note">读取历史详情会显式加载该版本的 value；当前切片只读，不提供一键恢复，避免把历史查看伪装成无条件覆盖。</p>
          </div>
        ) : (
          <div className="server-history-list">
            {items.map((entry) => (
              <button className="server-history-item" disabled={loading} key={entry.revisionId} onClick={() => onRead(entry)}>
                <div><span className="history-kind">{operationLabel(entry.operation)}</span><b>Revision {entry.revisionId}</b><time>{historyTime(entry)}</time></div>
                <HistoryMetadata entry={entry} />
              </button>
            ))}
            {!loading && items.length === 0 && <div className="empty compact"><b>服务端没有历史记录</b><span>历史保留窗口由 Nacos 服务端配置决定。</span></div>}
            {loading && <div className="loading-line">正在读取 Nacos 服务端历史…</div>}
          </div>
        )}
        <div className="dialog-actions">
          {loading && <button className="button" onClick={onCancelOperation}>取消请求</button>}
          {!detail && nextCursor && <button className="button" disabled={loading} onClick={onLoadMore}>加载更早版本</button>}
          <button className="button primary" disabled={loading} onClick={onClose}>完成</button>
        </div>
      </section>
    </div>
  );
}

function HistoryMetadata({ entry }: { entry: ResourceHistoryEntry }) {
  return (
    <div className="server-history-meta">
      {entry.md5 && <span>MD5 {entry.md5}</span>}
      {entry.sourceUser && <span>用户 {entry.sourceUser}</span>}
      {entry.sourceIp && <span>来源 {entry.sourceIp}</span>}
      {entry.publishType && <span>{entry.publishType}</span>}
      {entry.contentType && <span>{entry.contentType}</span>}
    </div>
  );
}

function operationLabel(operation?: string) {
  const normalized = operation?.trim().toUpperCase();
  if (normalized?.startsWith("I")) return "创建";
  if (normalized?.startsWith("U")) return "更新";
  if (normalized?.startsWith("D")) return "删除";
  return operation || "变更";
}

function historyTime(entry: ResourceHistoryEntry) {
  const value = entry.modifiedAt ?? entry.createdAt;
  if (!value) return "时间未知";
  const numeric = Number(value);
  const date = Number.isFinite(numeric) ? new Date(numeric) : new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString("zh-CN");
}
