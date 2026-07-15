import type { AdapterId, NativeResourceInfo } from "./registry";

type NativeInfoDialogProps = {
  adapter: AdapterId;
  info?: NativeResourceInfo;
  loading: boolean;
  onCancelOperation: () => void;
  onClose: () => void;
};

const permissionLabels: Record<string, string> = {
  read: "读取",
  write: "写入",
  create: "创建子节点",
  delete: "删除子节点",
  admin: "管理 ACL",
};

export function NativeInfoDialog({
  adapter,
  info,
  loading,
  onCancelOperation,
  onClose,
}: NativeInfoDialogProps) {
  const title = adapter === "zookeeper" ? "ZooKeeper ACL" : "etcd Lease";
  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!loading) onClose(); }}>
      <section className="dialog native-info-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">NATIVE RESOURCE INFO</span><h2>{title}</h2></div>
          <button className="icon-button" disabled={loading} onClick={onClose}>×</button>
        </div>

        {loading && !info && (
          <div className="empty compact"><span className="empty-icon">◌</span><b>正在读取原生元数据</b><span>请求可取消，不会修改远端状态。</span></div>
        )}

        {info?.kind === "etcdLease" && (
          <div className="native-stat-grid">
            <div><span>Lease ID</span><strong>{info.leaseId}</strong></div>
            <div><span>剩余 TTL</span><strong>{formatDuration(info.remainingTtlSeconds)}</strong></div>
            <div><span>授予 TTL</span><strong>{formatDuration(info.grantedTtlSeconds)}</strong></div>
          </div>
        )}

        {info?.kind === "zookeeperAcl" && (
          <>
            <div className="native-summary"><span>ACL 版本</span><b>{info.aclVersion}</b><small>{info.entries.length} 条规则</small></div>
            <div className="acl-list">
              {info.entries.map((entry, index) => (
                <article className="acl-entry" key={`${entry.scheme}-${entry.id}-${index}`}>
                  <div><span className="history-kind">{entry.scheme}</span><b>{entry.id || "（空身份）"}</b></div>
                  <div className="acl-permissions">
                    {entry.permissions.map((permission) => (
                      <span key={permission}>{permissionLabels[permission] ?? permission}</span>
                    ))}
                    {entry.permissions.length === 0 && <span>无权限</span>}
                  </div>
                </article>
              ))}
              {info.entries.length === 0 && <div className="empty compact"><b>没有 ACL 条目</b></div>}
            </div>
          </>
        )}

        <p className="form-note">此入口只读取协议原生元数据，不会续租、撤销 Lease 或修改 ACL。</p>
        <div className="dialog-actions">
          {loading && <button className="button" onClick={onCancelOperation}>取消请求</button>}
          <button className="button primary" disabled={loading} onClick={onClose}>完成</button>
        </div>
      </section>
    </div>
  );
}

function formatDuration(seconds: number) {
  if (seconds < 0) return "已失效";
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return remainder ? `${minutes} 分 ${remainder} 秒` : `${minutes} 分`;
}
