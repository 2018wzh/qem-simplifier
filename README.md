# qem-simplifier

一个面向实时与离线场景的网格简化库，基于 QEM（Quadric Error Metrics）实现，提供：

- Rust API（`rlib`）
- C ABI 动态库（`cdylib`）
- 命令行工具（`qem-cli`）

可用于接入任意引擎或工具链（自研引擎、Unity 原生插件、Godot、离线处理管线等）。

## 特性

- 支持按目标三角形数/顶点数/误差阈值进行简化
- 支持最小保留下限（`min_vertices` / `min_triangles`）
- 支持表面积保护（`preserve_surface_area`）
- 支持顶点附加属性与权重（法线、UV、颜色等）
- 支持场景级输入（场景树/通用场景图）并进行全局预算分配
- 支持“决策算法（分配比例）”与“按比例执行简化”解耦
- 支持“场景特征提取 → 预算求解 → 并行局部QEM执行”的分步管线
- 支持场景统计计算与 CSV 文本导出（用于审计与可观测）
- 通过 C ABI 可跨语言调用

## 仓库结构

- `src/lib.rs`：核心库与 C ABI 导出
- `src/main.rs`：CLI 工具入口（`qem-cli`）
- `include/qem_simplifier.h`：C 接口头文件
- `docs/接入指南.md`：跨引擎接入说明
- `docs/数据结构标准.md`：ABI 数据结构标准与版本约束
- `build.rs` + `cbindgen.toml`：头文件自动生成

## 环境要求

- Rust 工具链（建议稳定版）
- Cargo

## 构建

### 调试构建

```bash
cargo build
```

### 发布构建

```bash
cargo build --release
```

构建产物（按平台）：

- Windows: `target/<profile>/qem_simplifier.dll`
- Linux: `target/<profile>/libqem_simplifier.so`
- macOS: `target/<profile>/libqem_simplifier.dylib`

其中 `<profile>` 为 `debug` 或 `release`。

## 运行 CLI

当前二进制名：`qem-cli`。

查看帮助：

```bash
cargo run --bin qem-cli -- --help
```

示例（OBJ）：

```bash
cargo run --bin qem-cli -- \
  --input input.obj \
  --output output.obj \
  --ratio 0.5 \
  --target-triangles 10000 \
  --min-triangles 1000
```

示例（GLB/GLTF）：

```bash
cargo run --bin qem-cli -- \
  --input input.glb \
  --output output.obj \
  --target-triangles 20000
```

> CLI 当前输出为 OBJ。

## C ABI 快速说明

核心导出函数：

- `register_log_callback`
- `qem_context_set_progress_callback`
- `qem_context_clear_progress_callback`
- `qem_get_abi_version`
- `qem_context_create`
- `qem_context_destroy`
- `qem_get_last_result`
- `qem_simplify`
- `qem_scene_compute_decisions`
- `qem_scene_extract_features`
- `qem_scene_apply_decisions`
- `qem_scene_apply_decisions_ex`
- `qem_scene_simplify`
- `qem_scene_simplify_ex`
- `qem_scene_graph_compute_decisions`
- `qem_scene_graph_extract_features`
- `qem_scene_graph_apply_decisions`
- `qem_scene_graph_apply_decisions_ex`
- `qem_scene_graph_simplify`
- `qem_scene_graph_simplify_ex`
- `qem_scene_compute_statistics`
- `qem_scene_export_statistics_csv`

关键结构体：

- `QemMeshView`
- `QemProgressEvent`
- `QemSimplifyOptions`
- `QemSimplifyResult`
- `QemSceneMeshView`
- `QemSceneMeshFeature`
- `QemSceneNodeView`
- `QemSceneView`
- `QemSceneGraphNodeView`
- `QemSceneGraphMeshBindingView`
- `QemSceneGraphView`
- `QemScenePolicy`
- `QemSceneMeshDecision`
- `QemSceneMeshResult`
- `QemSceneSimplifyResult`
- `QemSceneExecutionOptions`
- `QemSceneMeshStatistics`
- `QemSceneStatisticsSummary`

状态码：

- `QEM_STATUS_SUCCESS = 0`
- `QEM_STATUS_INVALID_ARGUMENT = -1`
- `QEM_STATUS_PANIC = -2`
- `QEM_STATUS_INSUFFICIENT_BUFFER = -3`

场景权重模式常量：

- `QEM_SCENE_WEIGHT_UNIFORM`
- `QEM_SCENE_WEIGHT_MESH_VOLUME`
- `QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES`
- `QEM_SCENE_WEIGHT_EXTERNAL`

进度回调常量：

- `QEM_PROGRESS_SCOPE_MESH`
- `QEM_PROGRESS_SCOPE_SCENE`
- `QEM_PROGRESS_STAGE_BEGIN`
- `QEM_PROGRESS_STAGE_UPDATE`
- `QEM_PROGRESS_STAGE_END`

写回契约（摘要）：

- `qem_simplify` 成功后会回写 `QemMeshView.num_vertices/num_indices`，且数组有效数据位于前缀。
- 场景执行接口会逐 mesh 回写 `QemSceneView.meshes[i].mesh.num_vertices/num_indices`。
- 场景图执行接口会逐 mesh 回写 `QemSceneGraphView.meshes[i].mesh.num_vertices/num_indices`。
- 失败时计数字段保持调用前值；如需强一致回滚，建议调用方做输入快照。

## 场景简化框架（分层）

框架分为三层：

1. **场景输入层**：
  - `QemSceneView`（legacy）：`meshes + nodes`，每个节点最多绑定一个 mesh；
  - `QemSceneGraphView`（推荐）：`meshes + nodes + mesh_bindings`，节点与 mesh 绑定解耦，支持多实例。  
2. **决策层**：`qem_scene_compute_decisions` 根据 `QemScenePolicy` 计算每个 mesh 的 `keep_ratio/target_triangles`。  
3. **执行层**：`qem_scene_apply_decisions` 对各 mesh 做局部 QEM，并行执行（多核 CPU）。  

这使得“如何分配预算”可以按业务替换，而“如何执行简化”保持稳定。

分步调用建议：

1. `qem_scene_extract_features`：提取体积/实例规模/初始面数等特征；
2. `qem_scene_compute_decisions`：基于非线性权重与下限钳制计算 `Target_i`；
3. `qem_scene_apply_decisions`：并发执行局部 QEM。

若输入已是通用场景图（引擎层级 + 绑定关系）：

1. `qem_scene_graph_extract_features`
2. `qem_scene_graph_compute_decisions`
3. `qem_scene_graph_apply_decisions`

可选增强入口：

- `qem_scene_apply_decisions_ex`
- `qem_scene_simplify_ex`

通过 `QemSceneExecutionOptions` 配置并行线程上限、失败重试次数、目标回退步长。

## 引擎与 CLI 的 FBX 离线接入建议

- 引擎侧（UE/Unity/自研）可先将 FBX 解析为统一场景内存结构（mesh + node tree），再调用场景 API。  
- 推荐优先映射到 `QemSceneGraphView`：节点层级与 mesh 绑定分离，更贴合 FBX/引擎实例化语义。  
- CLI 侧建议通过 FBX 解析器（如 Assimp/FBX SDK/引擎导出流程）生成统一场景数据后调用：
  - 先 `qem_scene_graph_compute_decisions` 导出审阅结果；
  - 再 `qem_scene_graph_apply_decisions` 执行离线简化；
  - 如需一步到位可直接 `qem_scene_graph_simplify`。

## 跨引擎接入

请直接阅读：`docs/接入指南.md`

数据结构标准请阅读：`docs/数据结构标准.md`

该文档包含：

- 完整数据契约（数组布局、长度要求、原地写回规则）
- 通用调用流程（create → simplify → destroy）
- 线程与生命周期建议
- 最小验收清单

## 开发与验证

运行测试：

```bash
cargo test
```

格式与质量建议（可选）：

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

## 版本与兼容性

- 当前 ABI 版本：`6`（通过 `qem_get_abi_version()` 获取）
- 对接方建议在加载后先做 ABI 版本校验

