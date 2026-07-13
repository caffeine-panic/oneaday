# Atlas Registry

跨平台桌面客户端，用一个界面访问 etcd、ZooKeeper 和 Nacos。

当前处于技术 Spike 阶段，已建立以下垂直链路：

```text
React/TypeScript UI → Tauri 2 Rust command → Go sidecar JSON-RPC
```

## 本地开发

需要 Node.js、Go 1.26+、Rust stable，以及对应平台的 Tauri 系统依赖。

```bash
npm install
npm run sidecar:build
npm run tauri dev
```

单独运行检查：

```bash
npm run build
GOCACHE="$PWD/.cache/go-build" go test ./...
```

技术选型依据见 [docs/TECH_STACK_RESEARCH.md](docs/TECH_STACK_RESEARCH.md)。原始界面方案位于 `prototype/registry-client/`，正式界面以方案 A（三栏资源管理器）为基础。

## Spike 边界

- 已实现：方案 A 的 React 页面、Tauri 工程、Go sidecar 构建、进程级 JSON-RPC 能力握手。
- 尚未实现：真实 etcd/ZooKeeper/Nacos 连接、凭据保存、写操作和事件监听。
- sidecar 当前使用换行分隔 JSON-RPC；进入大事件流实现前需升级为长度前缀 framing。
