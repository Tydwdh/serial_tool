# 已知差异与残余工作（登记册）

本文登记**已知且刻意保留**的差异、构建/测试缺口与未闭合工作。源码注释与 CI 注释会按条目名引用本文——**改条目名要同步改引用处**。每条都给出代码位置；行号会漂移，按符号与文件名核对。

## 发送路径的残留差异

`tool_application::send_plan` 收敛了**判定**与编码（`crates/application/src/send_plan.rs`），但没有收敛判定的**输入**与披露方式。以下三项是实测后刻意保留的。

### 错误文案

- `AppError` 逐层加中文分类前缀（`传输错误：` `录制错误：` 等，`crates/application/src/error.rs`）。
  非法 HEX 的完整渲染串被
  `crates/application/tests/headless.rs::send_routing_dispatches_by_port_kind_and_rejects_invalid_hex`
  逐字钉住：
  `传输错误：HEX 解析失败：严格模式: "C" 规范化后为 1 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式`。
- 用户可见站点：发送红字标签（`crates/app/src/ui/bottom_panel.rs` 写 `SendView.error`，
  由 `crates/panels/src/sender.rs` 的 `colored_label` 渲染）；状态栏的活错误站点只来自 DTR/RTS。

### 预检的规则来源

- 按钮门禁是**共享面板** `crates/panels/src/sender.rs`：它传 `*view.hex_strict`（默认 `true`，
  `crates/app/src/state.rs`），面板与命令走同一个组件。
- 门禁与 `send_plan::decode_hex` 在 `"0x0xAB"` 这类输入上不一致：门禁判非法、`dispatch` 实际接受
  （按钮灰着而命令能发）。两侧各自的实际行为被测试钉住（`crates/panels/src/sender.rs::gate_and_decoder_disagree_on_repeated_0x_prefix`
  与 `crates/application/tests/headless.rs` 的渲染串断言）。统一判定会移动用户可见的按钮状态，登记为后续项而非顺手改。

### `is_network` 的计算规则

- 唯一路由输入 `is_network: bool`（`plan_send(command, is_network)`）由 native 组合根计算：
  比对 `app_config.network_ports` 字符串（同时认 `display_name()` 与 `port_id()` 两种形态）。
- 两种形态**都被持久化**（`workspace.json`），统一命名会打断已存工作区，故不统一。
- **候选（下一轮）**：`plan_send` 改收类型化目标 `SendTarget::{Serial, Network}`（来源
  `tool_platform::PortDescriptor::kind`），让"端口根本不存在"不再被折叠成"串口"。

## 平台与构建

### Linux clippy 的 `-D warnings` 缺口

- Linux 作业对 `hardware-workbench-app` **不加** `-D warnings`（`.github/workflows/ci.yml` 的 `linux-deb`）：
  自动更新链路被 `cfg(not(target_os = "linux"))` 关掉后，`crates/app` 留 9 处死代码/死读诊断：
  - `runtime/update.rs`：`start_update_check` / `start_update_download` / `force_check_update` never used；
  - `state.rs`：`UpdateState` 多字段、`CheckResult` 的 `version`/`download_url`/`sha256`/`cached`/`changelog`、
    `pinned_update_sha256`、`cached_check_result` never used / never read；
  - `ui/status_bar.rs`：`clippy::needless_return`（Linux 早返回块）、`draw_update_icon` never used；
  - `workbench_app.rs`：`update_state` never read。
- 复现配方（Windows 宿主即可，无需交叉编译）：把源码里 `#[cfg(windows)]` / `#[cfg(target_os = "windows")]`
  换成 `#[cfg(target_os = "none")]`（本机为假），`#[cfg(not(windows))]` / `#[cfg(target_os = "linux")]`
  换成 `#[cfg(not(target_os = "none"))]`（本机为真），然后跑
  `cargo clippy --workspace --all-targets -- -D warnings`——报错只会落在 `hardware-workbench-app`。
  复刻完用 `git checkout -- crates/` 还原。
- 收口条件：给更新链路补 cfg 或让 Linux 真正启用更新器后，删掉 CI 里的 `--exclude`，本条目随之关闭。

### Linux 死代码与豁免

- `#[allow]` 清单的**唯一真相**是 `crates/app/tests/lint_exemptions.rs` 里钉住的 `EXPECTED`
  （当前 3 条：2 条 clippy —— `crates/app/src/config.rs::large_enum_variant`、
  `crates/lua_host/src/lib.rs::too_many_arguments`；1 条 `dead_code` —— `state.rs`，刻意暂停、
  尚无调用者的方法）。本文不重复维护逐条清单；加一条豁免必须显式改那份清单，测试会红。
- 「豁免是不是空转」：把 `crates/` 下的豁免一次摘光，跑 native clippy（去掉 `-D warnings`
  收集全量），仍报的就是需要的。

### 串口权限错误的逐平台修法（`Serial` 臂）

- `tool_transport::translate_error` 的 `Io` 臂已按 kind 判定（权限格并进 `match e.kind()`，OS 原话留在冒号后）。
- 仍未闭合的是 `Serial` 臂：serialport 的 Windows 后端把 `ERROR_ACCESS_DENIED` 折进
  `ErrorKind::NoDevice`（与 FILE/PATH_NOT_FOUND 共用），权限信息在依赖内部就已销毁——
  Windows 半边按 kind 分不出「端口被占用」与「设备不存在」。
- 逐平台修法需要先把 `serialport::Error` 的 POSIX / Windows 两套形状写进用例，再做分叉。
  位置：`crates/transport/src/lib.rs::is_permission_denied` 附近。

## 测试

### 场景测试缺的半条契约（`ExpectNoTx` 窗口）

- `crates/extension/src/scenario.rs` 的取消用例第 8 步 `ExpectNoTx { timeout_ms: Some(80) }`
  既证明不了「取消让循环停下来」，又是该用例现在的头号红因：取消命令的派发实测最坏 ≈3.3s，
  而窗口只有 80ms；且 `ExpectNoTx` 把步前就躺在 backlog 里的旧 TX 也算违规（加大窗口救不了可靠）。
- 三角约束：可靠需要「写 → 被观测」的往返 < 循环周期；有区分力需要缺席窗口 > 循环周期；
  合起来窗口就得 > 派发延迟，而缺席检查每次**通过**都要白等那么久。
- 关闭它的代价（择一，都要动生产/DSL）：`ExpectNoTx` 只统计**本步开始之后**发布的 TX；
  或给 DSL 一个排空步、让 `Cancel` 等到插件真处理完再返回。测试侧唯一自洽的形状是把 fixture 换成
  **永不自行结束**的循环（`while true` + 取消后写一条终止标记），把「停了」变成正向期望——那是重写用例，不是改窗口。
