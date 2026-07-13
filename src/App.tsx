import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

type Capabilities = {
  protocolVersion: string;
  adapters: string[];
};

const connections = [
  { name: "生产 etcd", kind: "etcd", endpoint: "10.8.0.12:2379" },
  { name: "订单 ZK", kind: "zookeeper", endpoint: "3 节点 · 已连接" },
  { name: "Nacos 开发", kind: "nacos", endpoint: "dev.nacos.local" },
];

const nodes = [
  ["folder", "/services", 0],
  ["folder", "payment", 1],
  ["key", "config.json", 2],
  ["key", "instances", 2],
  ["folder", "checkout", 1],
  ["folder", "/locks", 0],
  ["folder", "/feature-flags", 0],
] as const;

const sampleValue = `{
  "service": "payment-api",
  "version": "2.4.1",
  "timeout": 3000,
  "features": {
    "riskCheck": true,
    "shadowTraffic": false
  }
}`;

export function App() {
  const [capabilities, setCapabilities] = useState<Capabilities>();
  const [error, setError] = useState<string>();
  const [value, setValue] = useState(sampleValue);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    invoke<Capabilities>("sidecar_capabilities")
      .then(setCapabilities)
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

  const save = () => {
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1600);
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand"><span className="logo">A</span>Atlas Registry</div>
        <span className="spike">TECH SPIKE</span>
        <div className="top-spacer" />
        <div className={`runtime ${error ? "failed" : ""}`}>
          <span className="status-dot" />
          {error ? "sidecar 启动失败" : capabilities ? `Core ${capabilities.protocolVersion}` : "正在启动 core…"}
        </div>
        <button className="button primary">＋ 新建连接</button>
      </header>

      <div className="shell">
        <aside className="connections">
          <div className="eyebrow">连接</div>
          {connections.map((connection, index) => (
            <button className={`connection ${index === 0 ? "active" : ""}`} key={connection.name}>
              <span className="status-dot" />
              <span><b>{connection.name}</b><small>{connection.endpoint}</small></span>
              <span className={`badge ${connection.kind}`}>{connection.kind === "zookeeper" ? "ZK" : connection.kind}</span>
            </button>
          ))}
          <button className="button wide">＋ 添加连接</button>
          <div className="capabilities">
            <div className="eyebrow">CORE ADAPTERS</div>
            {capabilities?.adapters.map((adapter) => <span className={`badge ${adapter}`} key={adapter}>{adapter}</span>)}
            {error && <p>{error}</p>}
          </div>
        </aside>

        <section className="tree">
          <div className="tree-header">
            <b>生产 etcd</b><button className="icon-button">↻</button>
            <input placeholder="筛选 key…" />
          </div>
          {nodes.map(([kind, name, level]) => (
            <button className={`node ${name === "config.json" ? "active" : ""}`} style={{ paddingLeft: 14 + level * 20 }} key={name}>
              <span className={kind}>{kind === "folder" ? "◆" : "◇"}</span>{name}
            </button>
          ))}
        </section>

        <main className="detail">
          <div className="breadcrumb">生产 etcd / services / payment / <b>config.json</b></div>
          <div className="detail-title">
            <div><span className="eyebrow">KEY</span><h1>config.json</h1></div>
            <div className="actions"><button className="button danger">删除</button><button className="button primary" onClick={save}>保存修改</button></div>
          </div>
          <div className="stats">
            <div><span>版本</span><strong>42</strong></div>
            <div><span>租约</span><strong>永久</strong></div>
            <div><span>大小</span><strong>{new Blob([value]).size} B</strong></div>
          </div>
          <div className="editor-header"><span>JSON</span><span>UTF-8</span></div>
          <textarea value={value} onChange={(event) => setValue(event.target.value)} spellCheck={false} />
          <div className="metadata"><span>创建版本</span><b>18,294</b><span>修改版本</span><b>20,118 · 2 分钟前</b></div>
        </main>
      </div>
      {saved && <div className="toast">技术 Spike：保存操作暂未连接真实集群</div>}
    </div>
  );
}
