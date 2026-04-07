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
- 通过 C ABI 可跨语言调用

## 仓库结构

- `src/lib.rs`：核心库与 C ABI 导出
- `src/main.rs`：CLI 工具入口（`qem-cli`）
- `include/qem_simplifier.h`：C 接口头文件
- `docs/接入指南.md`：跨引擎接入说明
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
- `qem_get_abi_version`
- `qem_context_create`
- `qem_context_destroy`
- `qem_get_last_result`
- `qem_simplify`

关键结构体：

- `QemMeshView`
- `QemSimplifyOptions`
- `QemSimplifyResult`

状态码：

- `QEM_STATUS_SUCCESS = 0`
- `QEM_STATUS_INVALID_ARGUMENT = -1`
- `QEM_STATUS_PANIC = -2`

## 跨引擎接入

请直接阅读：`docs/接入指南.md`

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

- 当前 ABI 版本：`3`（通过 `qem_get_abi_version()` 获取）
- 对接方建议在加载后先做 ABI 版本校验

