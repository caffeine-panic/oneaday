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

## 已实现能力

- 连接：新建、编辑、复制、删除、探测，系统凭据库，三协议认证与 TLS，长生命周期原生会话。
- 通用资源：可取消的分页/懒加载、值延迟读取、有界标识搜索、精确定位、二进制无损展示、1 MiB 内联边界、条件创建/更新/删除、监听、导入导出和脱敏审计历史。
- etcd：mod revision 条件写、watch、Lease 创建并绑定/绑定已有/单次续租/解绑/撤销，以及 2–32 项、总载荷不超过 1 MiB 的原子事务；Lease ID 始终以字符串跨 IPC 传输。
- ZooKeeper：version 条件写、one-shot watch 自动续订、ACL aversion 条件编辑与 ADMIN 防误锁，以及持久/持久顺序/临时/临时顺序节点；临时节点由桌面连接 session 持有。
- Nacos：2.x/3.x 配置、MD5 条件更新、SDK listener 加权威 MD5 对账、服务端配置历史，以及 namespace/service/instance 管理。持久实例走版本化管理 API；临时实例由当前桌面连接持有 Naming SDK session、维持心跳，并支持注册、更新和注销。
- 安全边界：导出默认不含 value，导入 value 只进入 10 分钟一次性 Rust 计划；监听事件、历史列表、审计、错误和管理操作目标均不携带正文、密码或 token；诊断包只导出运行环境、adapter 能力和连接聚合计数，不含连接名、endpoint、namespace 或凭据。
- 应用更新：标题栏可手动检查 GitHub Release；更新包由 Rust 下载、使用内置公钥验签、安装并重启，私钥只存在于发布环境。
- 验证与发布：六版本真实服务矩阵，10 万 ZooKeeper 子节点分页证据，以及 macOS/Windows/Linux 安装包构建和安装验证门禁。

adapter 只统一连接与资源操作外形；etcd lease/transaction、ZooKeeper ACL/ephemeral、Nacos namespace/service/instance 保留各自语义。ZooKeeper SASL 仍是预留扩展点，不在当前支持范围。

完整产品范围与验收口径见 [docs/PRODUCT_REQUIREMENTS.md](docs/PRODUCT_REQUIREMENTS.md)。
本地真实服务验证方法见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
已完成的验证记录见 [docs/VERIFICATION.md](docs/VERIFICATION.md)。
