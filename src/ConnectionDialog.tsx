import { connectionEnvironmentLabels } from "./registry";
import {
  authLabels,
  authModes,
  credentialIdentityLabel,
  credentialSecretLabel,
} from "./connectionAuth";
import type {
  AdapterId,
  AuthenticationMode,
  ConnectionEnvironment,
  ConnectionProfile,
} from "./registry";

export type ConnectionDialogMode = "new" | "edit" | "copy";

type ConnectionDialogProps = {
  mode: ConnectionDialogMode;
  form: ConnectionProfile;
  secret: string;
  busy: boolean;
  testing: boolean;
  onChange: (profile: ConnectionProfile) => void;
  onSecretChange: (secret: string) => void;
  onCancel: () => void;
  onTest: () => void;
  onSave: () => void;
  onDelete: () => void;
  onCancelOperation: () => void;
};

const endpointDefaults: Record<AdapterId, string> = {
  etcd: "127.0.0.1:2379",
  zookeeper: "127.0.0.1:2181",
  nacos: "127.0.0.1:8848",
};

const endpointPlaceholders: Record<AdapterId, string> = {
  etcd: "127.0.0.1:2379 或 etcd-1:2379,etcd-2:2379",
  zookeeper: "127.0.0.1:2181 或 zk-1:2181,zk-2:2181/app",
  nacos: "127.0.0.1:8848",
};

export function ConnectionDialog({
  mode,
  form,
  secret,
  busy,
  testing,
  onChange,
  onSecretChange,
  onCancel,
  onTest,
  onSave,
  onDelete,
  onCancelOperation,
}: ConnectionDialogProps) {
  const authenticated = form.auth.mode !== "none";
  const supportsTls = form.adapter !== "nacos";
  const title =
    mode === "edit" ? "编辑连接" : mode === "copy" ? "复制连接" : "新建连接";

  const changeAdapter = (adapter: AdapterId) => {
    onSecretChange("");
    onChange({
      ...form,
      adapter,
      endpoint: endpointDefaults[adapter],
      namespace: "",
      auth: { mode: "none", username: "", customKey: "" },
      tls: {
        enabled: false,
        caCertificatePath: "",
        clientCertificatePath: "",
        clientKeyPath: "",
        serverName: "",
      },
    });
  };

  const changeAuthMode = (authMode: AuthenticationMode) => {
    onSecretChange("");
    onChange({
      ...form,
      auth: { mode: authMode, username: "", customKey: "" },
    });
  };

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={() => {
        if (!testing) onCancel();
      }}
    >
      <section
        className="dialog connection-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">CONNECTION</span>
            <h2>{title}</h2>
          </div>
          <button className="icon-button" disabled={testing} onClick={onCancel}>
            ×
          </button>
        </div>

        <div className="form-grid equal">
          <label>
            类型
            <select
              value={form.adapter}
              onChange={(event) =>
                changeAdapter(event.target.value as AdapterId)
              }
            >
              <option value="etcd">etcd</option>
              <option value="zookeeper">ZooKeeper</option>
              <option value="nacos">Nacos</option>
            </select>
          </label>
          <label>
            环境
            <select
              value={form.environment}
              onChange={(event) =>
                onChange({
                  ...form,
                  environment: event.target.value as ConnectionEnvironment,
                })
              }
            >
              {Object.entries(connectionEnvironmentLabels).map(
                ([value, label]) => (
                  <option value={value} key={value}>
                    {label}
                  </option>
                ),
              )}
            </select>
          </label>
        </div>
        <label>
          名称
          <input
            autoFocus
            value={form.name}
            onChange={(event) =>
              onChange({ ...form, name: event.target.value })
            }
            placeholder="例如：生产配置中心"
          />
        </label>
        <label>
          Endpoint
          <input
            value={form.endpoint}
            onChange={(event) =>
              onChange({ ...form, endpoint: event.target.value })
            }
            placeholder={endpointPlaceholders[form.adapter]}
          />
        </label>

        {form.adapter === "nacos" && (
          <div className="form-grid">
            <label>
              Namespace
              <input
                value={form.namespace}
                onChange={(event) =>
                  onChange({ ...form, namespace: event.target.value })
                }
                placeholder="public"
              />
            </label>
            <label>
              Admin API
              <select
                value={form.nacosApiVersion}
                onChange={(event) =>
                  onChange({
                    ...form,
                    nacosApiVersion: event.target.value as "v2" | "v3",
                  })
                }
              >
                <option value="v2">Nacos 2.x</option>
                <option value="v3">Nacos 3.x</option>
              </select>
            </label>
          </div>
        )}

        <div className="form-section">
          <div className="form-section-title">认证</div>
          <label>
            认证方式
            <select
              value={form.auth.mode}
              onChange={(event) =>
                changeAuthMode(event.target.value as AuthenticationMode)
              }
            >
              {authModes(form.adapter).map((mode) => (
                <option value={mode} key={mode}>
                  {authLabels[mode]}
                </option>
              ))}
            </select>
          </label>
          {authenticated && form.auth.mode !== "custom" && (
            <div className="form-grid equal">
              <label>
                {credentialIdentityLabel(form.auth.mode)}
                <input
                  value={form.auth.username}
                  onChange={(event) =>
                    onChange({
                      ...form,
                      auth: { ...form.auth, username: event.target.value },
                    })
                  }
                  autoComplete="off"
                />
              </label>
              <label>
                {credentialSecretLabel(form.auth.mode)}
                <input
                  type="password"
                  value={secret}
                  onChange={(event) => onSecretChange(event.target.value)}
                  autoComplete="new-password"
                  placeholder={
                    mode === "edit"
                      ? form.auth.mode === "mseAccessKey"
                        ? "留空表示保留原 AccessKey Secret"
                        : "留空表示保留原密码"
                      : "保存在系统凭据库"
                  }
                />
              </label>
            </div>
          )}
          {form.auth.mode === "custom" && (
            <div className="form-grid equal">
              <label>
                上下文键
                <input
                  value={form.auth.customKey}
                  onChange={(event) =>
                    onChange({
                      ...form,
                      auth: { ...form.auth, customKey: event.target.value },
                    })
                  }
                  placeholder="例如 accessToken"
                />
              </label>
              <label>
                上下文密钥
                <input
                  type="password"
                  value={secret}
                  onChange={(event) => onSecretChange(event.target.value)}
                  autoComplete="new-password"
                  placeholder={
                    mode === "edit" ? "留空表示保留原密钥" : "保存在系统凭据库"
                  }
                />
              </label>
            </div>
          )}
          <p className="form-note">
            密钥只通过一次性 Tauri IPC 进入
            Rust，并存入操作系统凭据库；连接配置文件与 WebView 状态不保存密钥。
          </p>
        </div>

        {supportsTls && (
          <div className="form-section">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={form.tls.enabled}
                onChange={(event) =>
                  onChange({
                    ...form,
                    tls: { ...form.tls, enabled: event.target.checked },
                  })
                }
              />
              启用 TLS
            </label>
            {form.tls.enabled && (
              <>
                <label>
                  CA 证书路径
                  <input
                    value={form.tls.caCertificatePath}
                    onChange={(event) =>
                      onChange({
                        ...form,
                        tls: {
                          ...form.tls,
                          caCertificatePath: event.target.value,
                        },
                      })
                    }
                    placeholder={
                      form.adapter === "zookeeper"
                        ? "/path/to/ca.pem（必填）"
                        : "/path/to/ca.pem（留空使用系统根证书）"
                    }
                  />
                </label>
                <div className="form-grid equal">
                  <label>
                    客户端证书路径
                    <input
                      value={form.tls.clientCertificatePath}
                      onChange={(event) =>
                        onChange({
                          ...form,
                          tls: {
                            ...form.tls,
                            clientCertificatePath: event.target.value,
                          },
                        })
                      }
                      placeholder="可选，需与私钥同时配置"
                    />
                  </label>
                  <label>
                    客户端私钥路径
                    <input
                      value={form.tls.clientKeyPath}
                      onChange={(event) =>
                        onChange({
                          ...form,
                          tls: {
                            ...form.tls,
                            clientKeyPath: event.target.value,
                          },
                        })
                      }
                      placeholder="可选；私钥内容只由 Rust 读取"
                    />
                  </label>
                </div>
                {form.adapter === "etcd" && (
                  <label>
                    Server Name
                    <input
                      value={form.tls.serverName}
                      onChange={(event) =>
                        onChange({
                          ...form,
                          tls: { ...form.tls, serverName: event.target.value },
                        })
                      }
                      placeholder="证书域名覆盖，可选"
                    />
                  </label>
                )}
              </>
            )}
          </div>
        )}

        {form.environment === "production" && (
          <div className="mutation-warning">
            该连接已标记为生产环境。资源写入仍会要求输入连接名并进行版本条件校验。
          </div>
        )}
        <div className="dialog-actions split-actions">
          <div>
            {mode === "edit" && (
              <button
                className="button danger"
                disabled={busy}
                onClick={onDelete}
              >
                删除连接
              </button>
            )}
          </div>
          <div className="action-group">
            <button
              className="button"
              onClick={testing ? onCancelOperation : onCancel}
            >
              {testing ? "取消测试" : "取消"}
            </button>
            <button className="button" disabled={busy} onClick={onTest}>
              测试连接
            </button>
            <button className="button primary" disabled={busy} onClick={onSave}>
              保存并连接
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
