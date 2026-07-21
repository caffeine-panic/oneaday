import { useEffect, useState } from "react";
import { useRegistryOperations } from "./useRegistryOperations";
import {
  cancelOperation,
  connectionEnvironmentLabels,
  errorMessage,
  executeNacosNativeAction,
  isOutcomeUnknown,
  listNacosInstances,
  listNacosNamespaces,
  listNacosServices,
  readNacosService,
} from "./registry";
import type {
  ConnectionProfile,
  NacosInstance,
  NacosNamespace,
  NacosNativeAction,
  NacosService,
} from "./registry";

type Tab = "namespaces" | "services" | "instances";

type Props = {
  profile: ConnectionProfile;
  connectionId: string;
  onMessage: (message: string) => void;
  onClose: () => void;
};

const emptyNamespace = { id: "", name: "", description: "" };
const emptyService = {
  group: "DEFAULT_GROUP",
  name: "",
  protectThreshold: "0",
  ephemeral: false,
  metadata: "{}",
};
const emptyInstance = {
  cluster: "DEFAULT",
  ip: "127.0.0.1",
  port: "8080",
  weight: "1",
  enabled: true,
  ephemeral: false,
  metadata: "{}",
};

function actionImpact(action: NacosNativeAction) {
  switch (action.action) {
    case "createNamespace":
    case "updateNamespace":
      return `namespace ${action.namespaceId} · 名称 ${action.name}`;
    case "deleteNamespace":
      return `namespace ${action.namespaceId} · 删除`;
    case "createService":
    case "updateService":
      return `${action.group}@@${action.serviceName} · protect ${action.protectThreshold} · ${action.ephemeral ? "ephemeral" : "persistent"} · metadata ${Object.keys(action.metadata).length} 项`;
    case "deleteService":
      return `${action.group}@@${action.serviceName} · 删除服务`;
    case "registerInstance":
    case "updateInstance":
      return `${action.group}@@${action.serviceName}/${action.cluster}/${action.ip}:${action.port} · weight ${action.weight} · ${action.enabled ? "enabled" : "disabled"} · ${action.ephemeral ? "ephemeral" : "persistent"} · metadata ${Object.keys(action.metadata).length} 项`;
    case "deregisterInstance":
      return `${action.group}@@${action.serviceName}/${action.cluster}/${action.ip}:${action.port} · 注销 ${action.ephemeral ? "ephemeral" : "persistent"} instance`;
  }
}

export function NacosNativeDialog({
  profile,
  connectionId,
  onMessage,
  onClose,
}: Props) {
  const [tab, setTab] = useState<Tab>("services");
  const [busy, setBusy] = useState(true);
  const [localError, setLocalError] = useState<string>();
  const [namespaces, setNamespaces] = useState<NacosNamespace[]>([]);
  const [services, setServices] = useState<NacosService[]>([]);
  const [serviceCursor, setServiceCursor] = useState<string>();
  const [instances, setInstances] = useState<NacosInstance[]>([]);
  const [selectedNamespace, setSelectedNamespace] = useState<NacosNamespace>();
  const [selectedService, setSelectedService] = useState<NacosService>();
  const [selectedInstance, setSelectedInstance] = useState<NacosInstance>();
  const [namespaceForm, setNamespaceForm] = useState(emptyNamespace);
  const [serviceForm, setServiceForm] = useState(emptyService);
  const [instanceForm, setInstanceForm] = useState(emptyInstance);
  const [groupFilter, setGroupFilter] = useState("DEFAULT_GROUP");
  const [pending, setPending] = useState<NacosNativeAction>();
  const [confirmation, setConfirmation] = useState("");
  const operations = useRegistryOperations<"nacos">(
    crypto.randomUUID.bind(crypto),
    cancelOperation,
  );
  const runOperation = operations.run;
  const cancelTrackedOperation = operations.cancel;

  useEffect(() => {
    let disposed = false;
    void (async () => {
      const loadedNamespaces = await runOperation("nacos", (operationId) =>
        listNacosNamespaces(connectionId, operationId),
      );
      if (disposed) return;
      const page = await runOperation("nacos", (operationId) =>
        listNacosServices(connectionId, "DEFAULT_GROUP", operationId),
      );
      if (disposed) return;
      setNamespaces(loadedNamespaces);
      setServices(page.items);
      setServiceCursor(page.nextCursor);
    })()
      .catch((reason: unknown) => {
        if (!disposed) setLocalError(errorMessage(reason));
      })
      .finally(() => {
        if (!disposed) setBusy(false);
      });
    return () => {
      disposed = true;
      void cancelTrackedOperation("nacos");
    };
  }, [cancelTrackedOperation, connectionId, runOperation]);

  const run = async <T,>(operation: (operationId: string) => Promise<T>) => {
    if (busy) throw new Error("已有 Nacos 请求正在执行");
    setBusy(true);
    setLocalError(undefined);
    try {
      return await operations.run("nacos", operation);
    } finally {
      setBusy(false);
    }
  };

  const refreshNamespaces = async () => {
    try {
      setNamespaces(
        await run((operationId) =>
          listNacosNamespaces(connectionId, operationId),
        ),
      );
    } catch (reason) {
      setLocalError(errorMessage(reason));
    }
  };

  const refreshServices = async (append: boolean) => {
    const group = groupFilter.trim() || "DEFAULT_GROUP";
    try {
      const page = await run((operationId) =>
        listNacosServices(
          connectionId,
          group,
          operationId,
          append ? serviceCursor : undefined,
        ),
      );
      setServices((current) =>
        append ? [...current, ...page.items] : page.items,
      );
      setServiceCursor(page.nextCursor);
      if (!append) {
        setSelectedService(undefined);
        setSelectedInstance(undefined);
        setInstances([]);
      }
    } catch (reason) {
      setLocalError(errorMessage(reason));
    }
  };

  const selectService = async (summary: NacosService) => {
    try {
      const detail = await run((operationId) =>
        readNacosService(
          connectionId,
          summary.group,
          summary.name,
          operationId,
        ),
      );
      setSelectedService(detail);
      setServiceForm({
        group: detail.group,
        name: detail.name,
        protectThreshold: String(detail.protectThreshold),
        ephemeral: detail.ephemeral,
        metadata: JSON.stringify(detail.metadata, null, 2),
      });
      const loaded = await run((operationId) =>
        listNacosInstances(
          connectionId,
          detail.group,
          detail.name,
          operationId,
        ),
      );
      setInstances(loaded);
      setSelectedInstance(undefined);
    } catch (reason) {
      setLocalError(errorMessage(reason));
    }
  };

  const selectNamespace = (item: NacosNamespace) => {
    setSelectedNamespace(item);
    setNamespaceForm({
      id: item.id,
      name: item.name,
      description: item.description,
    });
  };

  const selectInstance = (item: NacosInstance) => {
    setSelectedInstance(item);
    setInstanceForm({
      cluster: item.cluster,
      ip: item.ip,
      port: String(item.port),
      weight: String(item.weight),
      enabled: item.enabled,
      ephemeral: item.ephemeral,
      metadata: JSON.stringify(item.metadata, null, 2),
    });
  };

  const review = (action: NacosNativeAction) => {
    setLocalError(undefined);
    setPending(action);
    setConfirmation("");
  };

  const reviewNamespaceSave = () => {
    const base = {
      namespaceId: namespaceForm.id.trim(),
      name: namespaceForm.name.trim(),
      description: namespaceForm.description.trim(),
    };
    review(
      selectedNamespace
        ? {
            action: "updateNamespace",
            ...base,
            expectedFingerprint: selectedNamespace.fingerprint,
          }
        : { action: "createNamespace", ...base },
    );
  };

  const reviewServiceSave = () => {
    try {
      const base = {
        group: serviceForm.group.trim(),
        serviceName: serviceForm.name.trim(),
        protectThreshold: Number(serviceForm.protectThreshold),
        ephemeral: serviceForm.ephemeral,
        metadata: parseMetadata(serviceForm.metadata),
      };
      review(
        selectedService
          ? {
              action: "updateService",
              ...base,
              expectedFingerprint: selectedService.fingerprint,
            }
          : { action: "createService", ...base },
      );
    } catch (reason) {
      setLocalError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  const reviewInstanceSave = () => {
    if (!selectedService) return;
    try {
      const base = {
        group: selectedService.group,
        serviceName: selectedService.name,
        cluster: instanceForm.cluster.trim(),
        ip: instanceForm.ip.trim(),
        port: Number(instanceForm.port),
        weight: Number(instanceForm.weight),
        enabled: instanceForm.enabled,
        ephemeral: instanceForm.ephemeral,
        metadata: parseMetadata(instanceForm.metadata),
      };
      review(
        selectedInstance
          ? {
              action: "updateInstance",
              ...base,
              expectedFingerprint: selectedInstance.fingerprint,
            }
          : { action: "registerInstance", ...base },
      );
    } catch (reason) {
      setLocalError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  const execute = async () => {
    if (!pending || confirmation !== profile.name) return;
    const action = pending;
    try {
      const result = await run((operationId) =>
        executeNacosNativeAction(connectionId, action, operationId),
      );
      setPending(undefined);
      setConfirmation("");
      onMessage(
        `${result.target} 已执行；这是检查后变更，远端结果已回读确认，脱敏审计已记录`,
      );
      if (action.action.endsWith("Namespace")) {
        setSelectedNamespace(undefined);
        setNamespaceForm(emptyNamespace);
        await refreshNamespaces();
      } else if (
        ["registerInstance", "updateInstance", "deregisterInstance"].includes(
          action.action,
        )
      ) {
        const service = selectedService;
        setSelectedInstance(undefined);
        setInstanceForm(emptyInstance);
        if (service) {
          await selectService(service);
          setTab("instances");
        } else {
          setTab("services");
          await refreshServices(false);
        }
      } else {
        setTab("services");
        await refreshServices(false);
        setServiceForm({
          ...emptyService,
          group: groupFilter.trim() || "DEFAULT_GROUP",
        });
        setInstanceForm(emptyInstance);
      }
    } catch (reason) {
      const message = errorMessage(reason);
      setLocalError(
        isOutcomeUnknown(reason)
          ? `${message}；请刷新列表核对远端状态，勿直接重试`
          : message,
      );
      if (isOutcomeUnknown(reason)) setPending(undefined);
    }
  };

  const cancel = async () => {
    await operations.cancel("nacos");
  };

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={() => {
        if (!busy) onClose();
      }}
    >
      <section
        className="dialog nacos-native-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">NACOS NATIVE MANAGEMENT</span>
            <h2>命名空间 · 服务 · 实例</h2>
          </div>
          <button className="icon-button" disabled={busy} onClick={onClose}>
            ×
          </button>
        </div>
        <div className="native-tabs">
          <button
            className={tab === "namespaces" ? "active" : ""}
            onClick={() => setTab("namespaces")}
          >
            命名空间
          </button>
          <button
            className={tab === "services" ? "active" : ""}
            onClick={() => setTab("services")}
          >
            服务
          </button>
          <button
            className={tab === "instances" ? "active" : ""}
            disabled={!selectedService}
            onClick={() => setTab("instances")}
          >
            实例 {selectedService ? `· ${selectedService.name}` : ""}
          </button>
        </div>

        {localError && (
          <div className="mutation-warning danger-warning">{localError}</div>
        )}
        {busy && (
          <div className="loading-line">
            正在访问 Nacos{" "}
            {profile.nacosApiVersion === "v3"
              ? "3.x Admin API"
              : "2.x Open API"}
            … <button onClick={() => void cancel()}>取消</button>
          </div>
        )}

        {tab === "namespaces" && (
          <div className="native-management-grid">
            <div className="native-object-list">
              <div className="list-heading">
                <b>命名空间</b>
                <button
                  className="button"
                  disabled={busy}
                  onClick={() => void refreshNamespaces()}
                >
                  刷新
                </button>
              </div>
              {namespaces.map((item) => (
                <button
                  className={selectedNamespace?.id === item.id ? "active" : ""}
                  key={item.id}
                  onClick={() => selectNamespace(item)}
                >
                  <b>{item.name}</b>
                  <span>
                    {item.id} · {item.configCount} configs
                  </span>
                </button>
              ))}
              <button
                className="new-object"
                onClick={() => {
                  setSelectedNamespace(undefined);
                  setNamespaceForm(emptyNamespace);
                }}
              >
                ＋ 新建命名空间
              </button>
            </div>
            <div className="native-object-editor">
              <h3>{selectedNamespace ? "编辑命名空间" : "新建命名空间"}</h3>
              <label>
                ID
                <input
                  value={namespaceForm.id}
                  disabled={Boolean(selectedNamespace) || busy}
                  onChange={(event) =>
                    setNamespaceForm({
                      ...namespaceForm,
                      id: event.target.value,
                    })
                  }
                />
              </label>
              <label>
                显示名称
                <input
                  value={namespaceForm.name}
                  disabled={busy}
                  onChange={(event) =>
                    setNamespaceForm({
                      ...namespaceForm,
                      name: event.target.value,
                    })
                  }
                />
              </label>
              <label>
                描述
                <textarea
                  value={namespaceForm.description}
                  disabled={busy}
                  onChange={(event) =>
                    setNamespaceForm({
                      ...namespaceForm,
                      description: event.target.value,
                    })
                  }
                />
              </label>
              <div className="editor-actions">
                {selectedNamespace && selectedNamespace.id !== "public" && (
                  <button
                    className="button danger"
                    disabled={busy}
                    onClick={() =>
                      review({
                        action: "deleteNamespace",
                        namespaceId: selectedNamespace.id,
                        expectedFingerprint: selectedNamespace.fingerprint,
                      })
                    }
                  >
                    删除
                  </button>
                )}
                <button
                  className="button primary"
                  disabled={
                    busy ||
                    !namespaceForm.id.trim() ||
                    !namespaceForm.name.trim()
                  }
                  onClick={reviewNamespaceSave}
                >
                  {selectedNamespace ? "保存修改" : "创建"}
                </button>
              </div>
            </div>
          </div>
        )}

        {tab === "services" && (
          <div className="native-management-grid">
            <div className="native-object-list">
              <div className="list-heading">
                <input
                  value={groupFilter}
                  onChange={(event) => setGroupFilter(event.target.value)}
                  placeholder="DEFAULT_GROUP"
                />
                <button
                  className="button"
                  disabled={busy}
                  onClick={() => void refreshServices(false)}
                >
                  查询
                </button>
              </div>
              {services.map((item) => (
                <button
                  className={
                    selectedService?.group === item.group &&
                    selectedService?.name === item.name
                      ? "active"
                      : ""
                  }
                  key={`${item.group}@@${item.name}`}
                  onClick={() => void selectService(item)}
                >
                  <b>{item.name}</b>
                  <span>{item.group}</span>
                </button>
              ))}
              {serviceCursor && (
                <button
                  className="new-object"
                  disabled={busy}
                  onClick={() => void refreshServices(true)}
                >
                  加载下一页
                </button>
              )}
              <button
                className="new-object"
                onClick={() => {
                  setSelectedService(undefined);
                  setServiceForm({
                    ...emptyService,
                    group: groupFilter.trim() || "DEFAULT_GROUP",
                  });
                }}
              >
                ＋ 新建服务
              </button>
            </div>
            <ServiceEditor
              form={serviceForm}
              disabled={busy}
              selected={selectedService}
              onChange={setServiceForm}
              onSave={reviewServiceSave}
              onDelete={() =>
                selectedService &&
                review({
                  action: "deleteService",
                  group: selectedService.group,
                  serviceName: selectedService.name,
                  expectedFingerprint: selectedService.fingerprint,
                })
              }
            />
          </div>
        )}

        {tab === "instances" && selectedService && (
          <div className="native-management-grid">
            <div className="native-object-list">
              <div className="list-heading">
                <b>
                  {selectedService.group}@@{selectedService.name}
                </b>
              </div>
              {instances.map((item) => (
                <button
                  className={
                    selectedInstance?.fingerprint === item.fingerprint
                      ? "active"
                      : ""
                  }
                  key={`${item.cluster}-${item.ip}-${item.port}`}
                  onClick={() => selectInstance(item)}
                >
                  <b>
                    {item.ip}:{item.port}
                  </b>
                  <span>
                    {item.cluster} · {item.healthy ? "healthy" : "unhealthy"} ·{" "}
                    {item.ephemeral ? "ephemeral" : "persistent"}
                  </span>
                </button>
              ))}
              <button
                className="new-object"
                onClick={() => {
                  setSelectedInstance(undefined);
                  setInstanceForm(emptyInstance);
                }}
              >
                ＋ 注册实例
              </button>
            </div>
            <InstanceEditor
              form={instanceForm}
              disabled={busy}
              selected={selectedInstance}
              onChange={setInstanceForm}
              onSave={reviewInstanceSave}
              onDelete={() =>
                selectedInstance &&
                review({
                  action: "deregisterInstance",
                  group: selectedInstance.group,
                  serviceName: selectedInstance.serviceName,
                  cluster: selectedInstance.cluster,
                  ip: selectedInstance.ip,
                  port: selectedInstance.port,
                  ephemeral: selectedInstance.ephemeral,
                  expectedFingerprint: selectedInstance.fingerprint,
                })
              }
            />
          </div>
        )}

        {pending && (
          <div className="native-confirm-panel">
            <div className="mutation-warning">
              Nacos 原生管理 API 没有 CAS。客户端会先比较读取时的 SHA-256
              指纹，写入后再回读确认，但校验与写入之间仍有竞争窗口。
            </div>
            <div className="impact-grid">
              <span>环境</span>
              <b>{connectionEnvironmentLabels[profile.environment]}</b>
              <span>Endpoint</span>
              <b>{profile.endpoint}</b>
              <span>Namespace</span>
              <b>{profile.namespace || "public"}</b>
              <span>操作</span>
              <b>{pending.action}</b>
              <span>影响范围</span>
              <b>{actionImpact(pending)}</b>
            </div>
            <label className="production-confirmation">
              确认执行 <b>{pending.action}</b>，请输入当前连接名{" "}
              <b>{profile.name}</b>。
              <input
                value={confirmation}
                disabled={busy}
                onChange={(event) => setConfirmation(event.target.value)}
                placeholder={profile.name}
              />
            </label>
            <div className="dialog-actions">
              <button
                className="button"
                disabled={busy}
                onClick={() => setPending(undefined)}
              >
                返回
              </button>
              <button
                className="button danger"
                disabled={busy || confirmation !== profile.name}
                onClick={() => void execute()}
              >
                确认执行并审计
              </button>
            </div>
          </div>
        )}
        <p className="form-note">
          当前连接命名空间：{profile.namespace || "public"}
          。服务和实例操作限定在该命名空间；命名空间页需要管理员权限。
        </p>
      </section>
    </div>
  );
}

type ServiceForm = typeof emptyService;
function ServiceEditor({
  form,
  disabled,
  selected,
  onChange,
  onSave,
  onDelete,
}: {
  form: ServiceForm;
  disabled: boolean;
  selected?: NacosService;
  onChange: (form: ServiceForm) => void;
  onSave: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="native-object-editor">
      <h3>{selected ? "编辑服务" : "新建服务"}</h3>
      <div className="form-grid equal">
        <label>
          Group
          <input
            value={form.group}
            disabled={Boolean(selected) || disabled}
            onChange={(event) =>
              onChange({ ...form, group: event.target.value })
            }
          />
        </label>
        <label>
          服务名
          <input
            value={form.name}
            disabled={Boolean(selected) || disabled}
            onChange={(event) =>
              onChange({ ...form, name: event.target.value })
            }
          />
        </label>
      </div>
      <label>
        保护阈值（0–1）
        <input
          inputMode="decimal"
          value={form.protectThreshold}
          disabled={disabled}
          onChange={(event) =>
            onChange({ ...form, protectThreshold: event.target.value })
          }
        />
      </label>
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={form.ephemeral}
          disabled={disabled || Boolean(selected)}
          onChange={(event) =>
            onChange({ ...form, ephemeral: event.target.checked })
          }
        />
        临时服务
      </label>
      <label>
        Metadata JSON
        <textarea
          value={form.metadata}
          disabled={disabled}
          onChange={(event) =>
            onChange({ ...form, metadata: event.target.value })
          }
          spellCheck={false}
        />
      </label>
      <div className="editor-actions">
        {selected && (
          <button
            className="button danger"
            disabled={disabled}
            onClick={onDelete}
          >
            删除服务
          </button>
        )}
        <button
          className="button primary"
          disabled={disabled || !form.group.trim() || !form.name.trim()}
          onClick={onSave}
        >
          {selected ? "保存修改" : "创建服务"}
        </button>
      </div>
    </div>
  );
}

type InstanceForm = typeof emptyInstance;
function InstanceEditor({
  form,
  disabled,
  selected,
  onChange,
  onSave,
  onDelete,
}: {
  form: InstanceForm;
  disabled: boolean;
  selected?: NacosInstance;
  onChange: (form: InstanceForm) => void;
  onSave: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="native-object-editor">
      <h3>{selected ? "编辑实例" : "注册实例"}</h3>
      <div className="form-grid equal">
        <label>
          IP
          <input
            value={form.ip}
            disabled={Boolean(selected) || disabled}
            onChange={(event) => onChange({ ...form, ip: event.target.value })}
          />
        </label>
        <label>
          端口
          <input
            inputMode="numeric"
            value={form.port}
            disabled={Boolean(selected) || disabled}
            onChange={(event) =>
              onChange({ ...form, port: event.target.value })
            }
          />
        </label>
      </div>
      <div className="form-grid equal">
        <label>
          Cluster
          <input
            value={form.cluster}
            disabled={Boolean(selected) || disabled}
            onChange={(event) =>
              onChange({ ...form, cluster: event.target.value })
            }
          />
        </label>
        <label>
          Weight
          <input
            inputMode="decimal"
            value={form.weight}
            disabled={disabled}
            onChange={(event) =>
              onChange({ ...form, weight: event.target.value })
            }
          />
        </label>
      </div>
      <div className="instance-flags">
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={form.enabled}
            disabled={disabled}
            onChange={(event) =>
              onChange({ ...form, enabled: event.target.checked })
            }
          />
          启用
        </label>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={form.ephemeral}
            disabled={disabled || Boolean(selected)}
            onChange={(event) =>
              onChange({ ...form, ephemeral: event.target.checked })
            }
          />
          临时实例
        </label>
      </div>
      <p className="form-note">
        {form.ephemeral
          ? "Naming SDK 会在当前桌面连接期间维持心跳；断开连接后，实例将按 Nacos TTL 自动过期。"
          : "持久实例无需客户端心跳，必须显式注销。"}
      </p>
      <label>
        Metadata JSON
        <textarea
          value={form.metadata}
          disabled={disabled}
          onChange={(event) =>
            onChange({ ...form, metadata: event.target.value })
          }
          spellCheck={false}
        />
      </label>
      <div className="editor-actions">
        {selected && (
          <button
            className="button danger"
            disabled={disabled}
            onClick={onDelete}
          >
            注销实例
          </button>
        )}
        <button
          className="button primary"
          disabled={disabled || !form.ip.trim() || !form.port.trim()}
          onClick={onSave}
        >
          {selected ? "保存修改" : "注册实例"}
        </button>
      </div>
    </div>
  );
}

function parseMetadata(value: string): Record<string, string> {
  const parsed: unknown = JSON.parse(value || "{}");
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
    throw new Error("Metadata 必须是 JSON object");
  const entries = Object.entries(parsed);
  if (entries.some(([, item]) => typeof item !== "string"))
    throw new Error("Metadata 的所有值必须是字符串");
  return Object.fromEntries(entries);
}
