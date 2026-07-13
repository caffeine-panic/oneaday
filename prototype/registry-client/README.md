# Atlas Registry 客户端原型

> PROTOTYPE — 用于回答“统一访问 etcd、ZooKeeper、Nacos 的客户端应该采用什么信息架构？”，不连接真实服务，也不持久化数据。

运行：

```bash
python3 -m http.server 4173 --directory prototype/registry-client
```

打开 `http://localhost:4173/?variant=A`。通过底部按钮或键盘左右方向键切换：

- A — 资源管理器：连接 / 树 / 详情三栏，适合高频运维和逐层浏览。
- B — 集群控制台：健康度、活动与跨集群搜索优先，适合平台团队总览。
- C — 聚焦检查器：路径直达和内容编辑优先，适合开发者快速查改配置。

所有写操作都是内存模拟。评审后请在下方记录结论，再删除落选方案并将胜出交互重写为正式实现。

## 评审结论

待填写：选择 __A_，保留 ___，调整 ___。
