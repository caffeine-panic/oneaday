# AGENTS.md — Atlas Registry 协作指南

本文件是 AI 编码工具（Claude Code、GitHub Copilot、Codex、Cursor 等）的权威工作说明，人也可以读。CLAUDE.md 只是对本文件的引用；两者出现分歧时以本文件为准，真正的门禁定义以 CI（`.github/workflows/quality.yml`）为准。

## 项目是什么

Tauri 2 桌面客户端：React 19 WebView + 纯 Rust 后端，统一访问 etcd / ZooKeeper / Nacos。没有 Node 服务端、没有 sidecar 进程。架构细节见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)，技术选型背景见 [docs/ADR-0001-TAURI-PURE-RUST.md](docs/ADR-0001-TAURI-PURE-RUST.md)。

## 目录地图

| 路径                            | 内容                                                                                                                                                                                     |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/`                          | React 前端。UI 状态逻辑放在纯函数模块（`resourceTree.ts`、`resourceWorkspaceState.ts` 等），组件只做渲染和事件接线                                                                       |
| `src/registry.ts`               | 前端唯一的 IPC 入口：所有 `invoke` 调用的类型化封装                                                                                                                                      |
| `src/generated/`                | ts-rs 从 Rust 生成的 IPC 契约类型，**禁止手改**                                                                                                                                          |
| `src-tauri/src/`                | Rust 核心：命令面（`lib.rs`）、领域模型与会话（`registry.rs`）、协议适配（`registry/`）、审计（`audit.rs`）、凭据（`credentials.rs`）、导入导出（`transfer.rs`）、更新器（`updates.rs`） |
| `src-tauri/tests/`              | Rust 集成测试，含 ignored 的真实服务契约 `live_registry.rs`                                                                                                                              |
| `scripts/*.test.mjs`            | 前端行为与契约测试（`node --test`，不起浏览器）                                                                                                                                          |
| `scripts/compatibility-test.sh` | 容器化真实服务兼容矩阵                                                                                                                                                                   |
| `docs/`                         | 产品需求、开发验证、发布流程、架构、ADR                                                                                                                                                  |
| `prototype/`、`work/`、`logs/`  | 历史遗留，不维护、不引用、不格式化                                                                                                                                                       |

## 环境与命令

依赖：Node 22、Rust stable、`protoc`（`etcd-client` 构建需要；不在 PATH 时设 `PROTOC=/path/to/protoc`）。

```bash
npm install          # 首次
npm run tauri dev    # 日常开发
```

提交前的完整本地门禁（必须全绿，与 CI 一致）：

```bash
npm run format:check   # Prettier（写代码时可先 npm run format）
npm run lint           # ESLint（类型感知规则）
npm run test:ui        # 前端行为/契约测试
npm run build          # tsc + vite
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

## IPC 契约（最重要的机制）

- Rust DTO 是唯一事实源，跨 IPC 的类型标注 `#[cfg_attr(test, derive(ts_rs::TS))]` 与 `#[cfg_attr(test, ts(export))]`。
- `src/generated/*.ts` 由 `npm run generate:contracts` 重新生成；CI 会用 `scripts/verify-generated-contracts.mjs` 检查生成物是否与 Rust 漂移。
- 修改任何跨 IPC 类型后：重新生成、连同生成物一起提交。

## 新增 / 修改 Tauri command 检查单

1. 在 `src-tauri/src/` 实现 `#[tauri::command]`，错误类型用 `RegistryError`。
2. 注册到 `src-tauri/src/lib.rs` 的 `generate_handler![]`。
3. 加入 `src-tauri/build.rs` 的 `COMMANDS` 数组。
4. 在 `src-tauri/capabilities/default.json` 增加 `allow-<kebab-case>` 权限。
5. 在 `src/registry.ts` 增加类型化封装，UI 不得直接 `invoke`。
6. 在 `scripts/` 相应契约测试中覆盖命令面变化。

四处缺一处都会在编译或运行时失败，这是有意设计的防漂移结构。

## 不可破坏的产品约束

改动涉及以下任何一条时，先读 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 对应章节再动手：

- **脱敏**：审计记录、诊断包、watch 事件、错误信息与日志一律不得包含资源 value、密码、token；诊断包有 sentinel 单元测试守护。
- **有界 IO**：内联 value ≤ 1 MiB；列表 / 搜索 / 历史全部分页 + 游标，禁止无界扫描；审计读取每页扫描 ≤ 512 KiB。
- **条件变更**：etcd 用 revision、ZooKeeper 用 version/aversion、Nacos 用 MD5 或指纹。远端结果不确定时报 `mutationOutcomeUnknown`，绝不自动重试；审计落盘失败报 `auditIncomplete`，不得伪装成远端未知。
- **协议语义不抹平**：lease / transaction / ACL / ephemeral / namespace / service 走各自的原生入口，通用界面只统一连接与资源操作外形。
- **凭据**：只存系统凭据库（`keyring`），临时凭据不落盘；任何新代码不得把密钥写进配置文件或日志。
- **错误处理**：统一走 `RegistryError` 分类；生产路径避免 `unwrap`/`expect`，不许 panic 传播到命令面。

## 代码风格

工具即规范，不要靠口头约定：

- TypeScript：Prettier（默认配置，版本精确锁定）+ ESLint 类型感知规则 + `tsc --strict`（含 `noUnusedLocals`）。
- Rust：rustfmt 默认风格 + clippy `-D warnings`；`Cargo.toml [lints]` 已 deny `unsafe_code`、`dbg_macro`、`todo`、`unimplemented`、`print_stdout`、`print_stderr`。
- 契约测试里针对源码的正则断言必须对空白不敏感（Prettier 可能随时重排代码）。
- 注释和文档用中文；标识符和提交信息用英文。

## 测试策略

- 前端逻辑改动 → `scripts/*.test.mjs` 补行为断言（纯函数模块可直接转译导入测试）。
- Rust 领域逻辑 → 同文件 `#[cfg(test)]` 单元测试；命令面 → `lib.rs` 里基于 `tauri::test` 的 mock 测试。
- 协议行为 → `src-tauri/tests/live_registry.rs`（ignored，需要真实服务，环境变量见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)）。
- 修 bug 先写失败测试再修；新增 UI 状态逻辑写成可测试的纯函数，不塞进组件。

## 文档同步义务

| 改了什么                      | 必须同步                                        |
| ----------------------------- | ----------------------------------------------- |
| 用户可见能力 / 限制           | [README.md](README.md)                          |
| 架构、模块边界、IPC 机制      | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)    |
| 开发 / 验证命令、测试环境变量 | [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)      |
| 发布、签名、更新器            | [docs/RELEASING.md](docs/RELEASING.md)          |
| 重大技术决策                  | 新增 `docs/ADR-NNNN-*.md`，不改写已接受的旧 ADR |
| 已完成的真实服务验证证据      | [docs/VERIFICATION.md](docs/VERIFICATION.md)    |

## 依赖与版本

- 新增依赖需要在 PR 描述里说明动机与替代方案；Rust 依赖优先 rustls / tokio 生态，禁止引入需要额外系统动态库的 crate。
- 版本号必须三处一致：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。
- 提交信息用 Conventional Commits（`feat:` / `fix:` / `ci:` / `docs:` / `refactor:` / `test:`）。
- 发布流程只看 [docs/RELEASING.md](docs/RELEASING.md)；GitHub Actions 一律固定到完整 commit SHA。
