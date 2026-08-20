# app-slint — Slint 预览壳（POC）

> 分支：`slint` · 与 `crates/app`（egui）并存，不影响主应用编译与打包。

## 目标
- 验证 `tool-core / tool-databus / tool-transport` 等非 UI crate 可被 Slint 原生复用
- 提供最小可跑窗口，后续渐进式迁移：终端 → 发送器 → 图表 → 插件面板

## 运行
```powershell
cargo run -p hardware-workbench-app-slint
```

## 结构
```
crates/app-slint/
  Cargo.toml
  build.rs                 # slint-build 编译 ui/*.slint
  ui/appwindow.slint       # 主窗口（fluent 风格）
  src/main.rs              # 事件绑定（tx-send / open-config-folder）
```

## 后续步骤
1. 引入 `TransportManager` + `DataBus` 订阅，打通真实 RX/TX 流
2. 终端虚拟化（StandardListView / 自定义 ListView）
3. 主题与图标对齐主应用（assets/app-icon-256.png 已复用）
4. 按 panel 逐个抽取为 Slint 组件，保持与 egui 版行为一致再切
