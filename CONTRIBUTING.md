# 贡献指南

本文是人类开发者的入口；AI 编码工具的权威说明在 [AGENTS.md](AGENTS.md)，两者共享同一套规则。

## 环境准备

- Node.js 22（与 CI 一致）
- Rust stable（rustup），组件 `clippy`、`rustfmt`
- `protoc`（`etcd-client` 构建依赖；不在 PATH 时设 `PROTOC=/path/to/protoc`）
- 平台对应的 [Tauri 2 系统依赖](https://tauri.app/start/prerequisites/)

```bash
npm install
npm run tauri dev
```

## 开发循环

1. 从 `master` 拉分支，小步提交。
2. 写代码前先看 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 确认改动落在哪一层；前端业务逻辑写进纯函数模块，Rust 变更遵守条件变更与脱敏不变量。
3. 修 bug 先写失败测试；改跨 IPC 类型后运行 `npm run generate:contracts` 并提交生成物。
4. 提交前跑完整本地门禁：

```bash
npm run format:check && npm run lint && npm run test:ui && npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

真实服务验证（可选，需要测试集群或本地 Docker）见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 代码风格

风格由工具定义，不接受手工例外：

- **TypeScript / CSS / Markdown**：Prettier 默认风格（`npm run format`），ESLint 类型感知规则，`tsc --strict`。
- **Rust**：rustfmt 默认风格；clippy `-D warnings`；`Cargo.toml [lints]` 禁 `unsafe`、`dbg!`、`todo!`、`print_*`。
- `src/generated/` 是 ts-rs 生成物，禁止手改、不参与格式化。
- 注释与文档写中文，标识符与提交信息写英文。

## 提交与 PR

- 提交信息用 Conventional Commits：`feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`ci:`、`chore:`。
- 新增依赖在 PR 描述说明动机与替代方案；版本号改动保持 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处一致。
- PR 合并条件：quality workflow 三平台全绿；涉及协议行为时 compatibility workflow 全绿。
- 按 [AGENTS.md 的文档同步义务](AGENTS.md#文档同步义务) 更新对应文档；重大技术决策新增 ADR。

## 发布

见 [docs/RELEASING.md](docs/RELEASING.md)。核心规则：tag 必须与三处版本号一致；release workflow 只产 Draft；签名 / 公证 / 安装验证完成前不得公开。
