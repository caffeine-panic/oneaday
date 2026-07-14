# 开发与验证

## 快速检查

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle
```

`etcd-client` 在构建时需要 `protoc`。如果它不在 `PATH`，可显式设置 `PROTOC=/path/to/protoc`。

## 真实服务只读契约

默认测试套件不会假装存在本地注册中心。准备测试集群后，通过 ignored tests 验证连接、会话和根资源浏览：

```bash
ATLAS_TEST_ETCD_ENDPOINT=127.0.0.1:2379 \
ATLAS_TEST_ZOOKEEPER_ENDPOINT=127.0.0.1:2181 \
ATLAS_TEST_NACOS_ENDPOINT=127.0.0.1:8848 \
cargo test --manifest-path src-tauri/Cargo.toml --test live_registry -- --ignored
```

Nacos 3.x 额外设置：

```bash
ATLAS_TEST_NACOS_VERSION=v3
ATLAS_TEST_NACOS_NAMESPACE=public
```

这些测试只执行连接与读取型列表操作，不会写入测试集群。认证、TLS 与写操作会在对应开发切片中加入独立环境矩阵。

若要同时验证读取与元数据，提供已有的只读 fixture：

```bash
ATLAS_TEST_ETCD_KEY=/atlas/fixture \
ATLAS_TEST_ZOOKEEPER_PATH=/atlas/fixture \
ATLAS_TEST_NACOS_GROUP=DEFAULT_GROUP \
ATLAS_TEST_NACOS_DATA_ID=atlas-fixture.yaml
```
