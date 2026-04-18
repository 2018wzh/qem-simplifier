# qem-simplifier

`qem-simplifier` 是一个面向实时与离线流程的网格简化库，基于 QEM（Quadric Error Metrics）实现，提供：

- Rust 库（`rlib`）
- C ABI 动态库（`cdylib`）
- 静态库（`staticlib`）
- 命令行工具（`qem-cli`）

适用于引擎插件、资产管线、批处理和离线预处理。

---

## 新 API 标准（当前：ABI v7）

从 ABI v7 开始，**场景能力统一采用场景图 API**：

- 数据模型统一为 `QemSceneGraphView`
- 场景接口统一为 `qem_scene_graph_*`
- 旧场景树接口（如 `QemSceneView`、`qem_scene_*`）不再作为标准接入面

启动时建议先做版本门禁：

1. 加载动态库
2. 调用 `qem_get_abi_version()`
3. 仅当返回 `7` 时启用场景图能力

---

## 核心能力

- 单网格简化：按目标三角形数/顶点数/误差阈值控制
- 场景图简化：支持 `节点层级 + mesh 绑定` 的通用输入
- 分步流程：特征提取 → 预算决策 → 执行（可插入业务策略）
- 一步流程：场景图一键完成决策与执行
- 统计与 CSV 导出：支持预算命中率、偏差、误差等观测
- 回调能力：日志回调 + 进度回调

---

## 仓库结构

- `src/lib.rs`：核心算法与 C ABI 导出
- `src/scene.rs`：场景图决策与执行
- `src/main.rs`：CLI 入口
- `src/cli/`：`model` / `scene` 子命令实现
- `include/qem_simplifier.h`：C 头文件（构建时自动生成）
- `docs/接入指南.md`：跨引擎接入与写回契约
- `docs/数据结构标准.md`：结构体字段级标准
- `build.rs`：头文件生成与插件目录同步逻辑

---

## 构建

调试构建：

```bash
cargo build
```

发布构建：

```bash
cargo build --release
```

主要产物（按平台）：

- Windows：`target/<profile>/qem_simplifier.dll`
- Linux：`target/<profile>/libqem_simplifier.so`
- macOS：`target/<profile>/libqem_simplifier.dylib`
- CLI：`target/<profile>/qem-cli[.exe]`

其中 `<profile>` 为 `debug` 或 `release`。

> 构建时会自动生成 `include/qem_simplifier.h`，并在插件目录存在时同步到：
>
> - `plugins/QEMMeshReduction/Source/ThirdParty/QEMSimplifier/Include/`
> - `plugins/QEMLevelSceneSimplifier/Source/ThirdParty/QEMSimplifier/Include/`

---

## C ABI 接口总览

### 基础能力

- `qem_context_create`
- `qem_context_destroy`
- `qem_get_last_result`
- `qem_simplify`

### 场景图能力（标准入口）

- `qem_scene_graph_extract_features`
- `qem_scene_graph_compute_decisions`
- `qem_scene_graph_apply_decisions`
- `qem_scene_graph_apply_decisions_ex`
- `qem_scene_graph_simplify`
- `qem_scene_graph_simplify_ex`

### 统计与导出

- `qem_scene_compute_statistics`
- `qem_scene_export_statistics_csv`

### 回调

- `register_log_callback`
- `qem_context_set_progress_callback`
- `qem_context_clear_progress_callback`

---

## 场景图标准数据模型

场景输入由三部分组成：

- `QemSceneGraphView.meshes`：可写 mesh 池（执行时原地写回）
- `QemSceneGraphView.nodes`：节点层级（`parent_index` + `local_matrix`）
- `QemSceneGraphView.mesh_bindings`：mesh 与节点绑定（支持实例化）

关键说明：

- 同一 `mesh_index` 可出现在多个 binding 中（共享几何，多实例）
- 当 `QemScenePolicy.use_world_scale != 0` 时，权重会结合节点累计尺度
- 若 `QemSceneGraphMeshBindingView.use_mesh_to_node_matrix != 0`，将叠加该绑定矩阵参与尺度评估

---

## 推荐调用流程

### A. 分步流程（推荐生产）

1. `qem_scene_graph_extract_features`
2. `qem_scene_graph_compute_decisions`
3. （可选）业务侧审阅或覆盖决策
4. `qem_scene_graph_apply_decisions_ex`
5. `qem_scene_compute_statistics`
6. `qem_scene_export_statistics_csv`

适合：需要审计、预算复核、策略回放的离线或平台流程。

### B. 一步流程（快速接入）

1. `qem_scene_graph_simplify_ex`
2. （可选）`qem_scene_compute_statistics`

适合：工具化场景、快速集成验证。

---

## 容量契约（务必遵守）

凡是带 `out_xxx + capacity + out_xxx_count` 的接口，均支持两段式调用：

1. 首次调用仅探测数量（缓冲区可空或容量为 0）
2. 按返回数量分配后再次调用

`qem_scene_export_statistics_csv` 同理：

1. `out_buffer = nullptr` 获取 `out_required_size`
2. 分配后再次写入

容量不足返回：`QEM_STATUS_INSUFFICIENT_BUFFER`。

---

## 写回与失败语义

- 成功时：执行类接口会原地回写 mesh 几何前缀，并更新 `num_vertices/num_indices`
- 失败时：计数字段保持调用前值，但数组可能存在部分中间写入
- 场景执行按 mesh 独立记录 `QemSceneMeshResult`，建议按 mesh 粒度处理回滚

---

## 状态码

- `QEM_STATUS_SUCCESS = 0`
- `QEM_STATUS_INVALID_ARGUMENT = -1`
- `QEM_STATUS_PANIC = -2`
- `QEM_STATUS_INSUFFICIENT_BUFFER = -3`

建议同时检查“函数返回值 + out_result.status”。

---

## CLI 使用

查看帮助：

```bash
cargo run --release -- --help
```

单网格简化：

```bash
cargo run --release -- model -i ./data/input.obj -o ./data/output.obj -r 0.5
```

场景图简化（GLB/GLTF 输入，GLB 输出）：

```bash
cargo run --release -- scene -i ./data/Demo.glb -o ./data/demo.out.glb -r 0.5
```

开启详细日志：

```bash
cargo run --release -- -v scene -i ./data/Demo.glb -o ./data/demo.out.glb -r 0.5 --dry-run
```

---

## 开发验证

```bash
cargo test --release
```

可选：

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

---

## 文档导航

- 接入与写回契约：`docs/接入指南.md`
- 数据结构标准：`docs/数据结构标准.md`
- C 头文件权威定义：`include/qem_simplifier.h`

