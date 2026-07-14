# Atlas Registry

跨平台桌面客户端，用一个界面访问 etcd、ZooKeeper 和 Nacos。

技术 Spike 已结束，项目进入正式开发阶段。当前已建立以下产品链路：

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

## 当前开发边界

- 已实现：方案 A 的 React 页面、Tauri 工程、Rust 侧非敏感连接配置持久化、长生命周期会话、三协议真实连接、可取消的分页/懒加载浏览、资源读取、结构化错误、二进制安全展示、1 MiB 大值保护，以及带版本校验、连接名二次确认、影响预览和脱敏审计的创建/更新/删除链路。
- 当前 ZooKeeper 通用 create 明确限定为继承父 ACL 的持久节点；ephemeral/sequential 创建留在后续 ZooKeeper 原生能力入口，不伪装为通用资源创建。
- 正在推进：连接编辑与凭据安全存储、认证/TLS、直接定位、事件监听和生产发布加固。
- adapter 只统一连接与资源操作外形；etcd lease/transaction、ZooKeeper ACL/ephemeral、Nacos namespace/service 保留各自语义。

完整产品范围与验收口径见 [docs/PRODUCT_REQUIREMENTS.md](docs/PRODUCT_REQUIREMENTS.md)。
本地真实服务验证方法见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
