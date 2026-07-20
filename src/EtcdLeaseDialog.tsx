import { useState } from "react";
import { connectionEnvironmentLabels } from "./registry";
import type {
  ConnectionProfile,
  EtcdLeaseAction,
  NativeResourceInfo,
  ResourceDocument,
} from "./registry";

type LeaseMode = "grantAndAttach" | "attach" | "detach" | "keepAlive" | "revoke";

type Props = {
  profile: ConnectionProfile;
  document: ResourceDocument;
  info?: Extract<NativeResourceInfo, { kind: "etcdLease" }>;
  loading: boolean;
  busy: boolean;
  onExecute: (action: EtcdLeaseAction) => void;
  onCancelOperation: () => void;
  onClose: () => void;
};

const modeLabels: Record<LeaseMode, string> = {
  grantAndAttach: "创建并绑定",
  attach: "绑定已有 Lease",
  detach: "解绑 Lease",
  keepAlive: "续租一次",
  revoke: "撤销 Lease",
};

export function EtcdLeaseDialog({
  profile,
  document,
  info,
  loading,
  busy,
  onExecute,
  onCancelOperation,
  onClose,
}: Props) {
  const [mode, setMode] = useState<LeaseMode>(info ? "keepAlive" : "grantAndAttach");
  const [ttl, setTtl] = useState("300");
  const [leaseId, setLeaseId] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const hasLease = Boolean(info);
  const currentLeaseId = info?.leaseId ?? leaseId.trim();

  const validTtl = (value: string) => {
    if (!/^[1-9]\d*$/.test(value.trim())) return false;
    const seconds = Number(value);
    return Number.isSafeInteger(seconds) && seconds > 0;
  };
  const canSubmit = !busy
    && !loading
    && Boolean(document.version)
    && confirmation === profile.name
    && (mode !== "grantAndAttach" || validTtl(ttl))
    && (mode !== "attach" || validLeaseId(leaseId))
    && (!(["keepAlive", "revoke"] as LeaseMode[]).includes(mode) || Boolean(info));

  const submit = () => {
    if (!document.version || !canSubmit) return;
    const common = { address: document.address };
    switch (mode) {
      case "grantAndAttach":
        onExecute({
          action: mode,
          ...common,
          expectedVersion: document.version,
          ttlSeconds: Number(ttl),
        });
        return;
      case "attach":
        onExecute({
          action: mode,
          ...common,
          expectedVersion: document.version,
          leaseId: leaseId.trim(),
        });
        return;
      case "detach":
        onExecute({ action: mode, ...common, expectedVersion: document.version });
        return;
      case "keepAlive":
        onExecute({ action: mode, ...common, leaseId: currentLeaseId });
        return;
      case "revoke":
        onExecute({
          action: mode,
          ...common,
          expectedVersion: document.version,
          leaseId: currentLeaseId,
        });
    }
  };

  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!busy) onClose(); }}>
      <section className="dialog native-info-dialog lease-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">ETCD LEASE</span><h2>Lease 生命周期</h2></div>
          <button className="icon-button" disabled={busy} onClick={onClose}>×</button>
        </div>

        {loading ? <div className="loading-line">正在读取 Lease 状态…</div> : info ? (
          <div className="native-stat-grid">
            <div><span>Lease ID</span><strong>{info.leaseId}</strong></div>
            <div><span>剩余 TTL</span><strong>{info.remainingTtlSeconds} s</strong></div>
            <div><span>授予 TTL</span><strong>{info.grantedTtlSeconds} s</strong></div>
          </div>
        ) : (
          <div className="native-summary"><span className="badge etcd">ETCD</span><b>当前 key 为永久 key，未绑定 Lease</b></div>
        )}

        <div className="impact-grid">
          <span>环境</span><b>{connectionEnvironmentLabels[profile.environment]}</b>
          <span>Endpoint</span><b>{profile.endpoint}</b>
          <span>Key 版本</span><b>{document.version ?? "无可用版本"}</b>
          <span>当前状态</span><b>{info ? `Lease ${info.leaseId}` : "永久"}</b>
        </div>

        <div className="lease-actions">
          {!hasLease && <>
            <button className={`button ${mode === "grantAndAttach" ? "primary" : ""}`} onClick={() => setMode("grantAndAttach")}>创建并绑定</button>
            <button className={`button ${mode === "attach" ? "primary" : ""}`} onClick={() => setMode("attach")}>绑定已有</button>
          </>}
          {hasLease && <>
            <button className={`button ${mode === "keepAlive" ? "primary" : ""}`} onClick={() => setMode("keepAlive")}>续租一次</button>
            <button className={`button ${mode === "detach" ? "primary" : ""}`} onClick={() => setMode("detach")}>解绑</button>
            <button className={`button ${mode === "revoke" ? "danger" : ""}`} onClick={() => setMode("revoke")}>撤销</button>
          </>}
        </div>

        {mode === "grantAndAttach" && <label>TTL（秒）
          <input inputMode="numeric" value={ttl} onChange={(event) => setTtl(event.target.value)} />
        </label>}
        {mode === "attach" && <label>已有 Lease ID
          <input inputMode="numeric" value={leaseId} onChange={(event) => setLeaseId(event.target.value)} placeholder="十进制 64 位 Lease ID" />
        </label>}

        {mode === "revoke" && <div className="mutation-warning danger-warning">撤销 Lease 会让该 Lease 关联的所有 key 立即过期。etcd 不提供“compare key 后原子撤销 Lease”的组合接口，因此这是校验后变更；请确认影响可能不限于当前 key。</div>}
        {mode === "detach" && <p className="form-note">解绑通过当前 Mod Revision 的原子事务完成，value 保持不变；key 将变为永久 key。</p>}
        {mode === "keepAlive" && <p className="form-note">发送一次 keep-alive，把该 Lease 的剩余 TTL 恢复到服务端授予值，不会在后台持续保活。</p>}

        <label className="production-confirmation">确认执行“{modeLabels[mode]}”，请输入当前连接名 <b>{profile.name}</b>。
          <input value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder={profile.name} />
        </label>
        <p className="form-note">所有 Lease 写入都会记录脱敏审计；Lease ID 以字符串传输，避免 JavaScript 数值精度损失。</p>
        <div className="dialog-actions">
          <button className="button" onClick={busy ? onCancelOperation : onClose}>{busy ? "取消请求" : "关闭"}</button>
          <button className={`button ${mode === "revoke" ? "danger" : "primary"}`} disabled={!canSubmit} onClick={submit}>{busy ? "正在提交…" : `确认${modeLabels[mode]}`}</button>
        </div>
      </section>
    </div>
  );
}

function validLeaseId(value: string) {
  const trimmed = value.trim();
  if (!/^[1-9]\d*$/.test(trimmed)) return false;
  try {
    return BigInt(trimmed) <= 9_223_372_036_854_775_807n;
  } catch {
    return false;
  }
}
