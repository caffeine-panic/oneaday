# Atlas Registry

跨平台桌面客户端，用一个界面访问 etcd、ZooKeeper 和 Nacos。

当前处于技术 Spike 阶段，已建立以下垂直链路：

```text
React/TypeScript UI → Tauri 2 command → Rust RegistryCatalog → native protocol adapters
```

## 本地开发

需要 Node.js、Rust stable、`protoc`，以及对应平台的 Tauri 系统依赖。

macOS 可安装：

```bash
brew install rust protobuf
```

```bash
npm install
npm run tauri dev
```

单独运行检查：

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

最终架构决策见 [docs/ADR-0001-TAURI-PURE-RUST.md](docs/ADR-0001-TAURI-PURE-RUST.md)，早期技术调研见 [docs/TECH_STACK_RESEARCH.md](docs/TECH_STACK_RESEARCH.md)。原始界面方案位于 `prototype/registry-client/`，正式界面以方案 A（三栏资源管理器）为基础。

## Spike 边界

- 已实现：方案 A 的 React 页面、Tauri 工程、纯 Rust adapter catalog、三种协议客户端依赖和真实连接探测。
- 尚未实现：凭据保存、节点读写、分页浏览、事件监听和生产环境保护。
- adapter 只统一连接与资源操作外形；etcd lease/transaction、ZooKeeper ACL/ephemeral、Nacos namespace/service 保留各自语义。
