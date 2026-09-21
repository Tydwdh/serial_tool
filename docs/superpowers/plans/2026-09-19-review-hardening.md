# 评审收敛修复（安全 / 测试 / 边界）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 关闭 2026-09-19 项目评审确认的 4 个 Critical 与 2 个 Important 缺陷：自更新无外置哈希、Lua 沙箱 `dofile` 可达、发送链路无字节级契约、worker panic 成僵尸端口、架构守卫可被绕过、native/web 两套 send 路由已分叉。

**Architecture:** 全部改动落在既有分层内，不新增 crate。`tool-core` 承接跨平台 HEX 解析（native/wasm 唯一真相）；`tool-application` 新增无 `cfg` 门控的 `send_plan` 模块统一命令→(任务种类, 字节) 路由；`crates/*/tests/architecture.rs` 从「Cargo.toml 文本匹配」升级为「TOML 解析 + 重命名归一」。

**Tech Stack:** Rust 2024 edition、cargo workspace（15 members + `vendor/egui_tiles`）、mlua 0.11（vendored Lua 5.4）、omnilua 0.7（wasm）、reqwest、serde、tungstenite（测试回路）、`toml` 1.1（已在 `Cargo.lock:4595`，本次转为 dev-dependency）。

**Spec:** 无独立 spec 文件；本计划的问题定义来自 2026-09-19 四路独立评审 + 主线自行复核，每条 Critical 都带可复现命令。

## Global Constraints

每个任务的验收都隐含本节；数值逐字取自仓库现状。

- **工具链**：所有验证命令必须带 `+1.92.0`。本机 stable 是 1.98.0，而 CI 钉 1.92.0；历史上两套工具链对 lint 判定相反并造成过两个 red gate（`docs/ARCHITECTURE.md:112-117`）。
- **必须带 `--workspace`**：根 `Cargo.toml:19` 有 `default-members = ["crates/app"]`，裸跑只覆盖一个 crate。
- **三条门全绿才算完成**：
  ```bash
  cargo +1.92.0 fmt --all --check
  cargo +1.92.0 clippy --workspace --all-targets -- -D warnings
  cargo +1.92.0 test --workspace --all-targets
  ```
- **禁止 lint 豁免**：不得新增 `#[allow(clippy::…)]` 绕过 `-D warnings`（`docs/ARCHITECTURE.md:115-117` 确立的先例）。
- **禁止用管道判断 cargo 结果**：`cargo test | tail` 的退出码来自 `tail`，恒为 0。写到日志再单独取 `$?`：
  ```bash
  cargo +1.92.0 test --workspace --all-targets > /tmp/t.log 2>&1; echo "EXIT=$?"
  ```
- **取证只在仓库外副本做**：任何需要临时改 tracked 文件的测量都复制到仓库外执行；只读用 `--locked`（`cargo tree` 与不带 `--locked` 的 `cargo metadata` 会重写 `Cargo.lock`）。
- **基线测试数**：改动前 `506 passed / 0 failed / 7 ignored`（21 targets）。每个任务结束时代码不得让任一用例变红。
- **不新增运行时依赖**：`Cargo.lock` 只在必要处变化；本次唯一新增的是 `toml` 作为 dev-dependency（包已在锁文件内）。
- **过程产物不入库**：`docs/superpowers/`、验证日志等不 `git add`；提交只纳源码与文档。
- **禁止 push / tag / release**：本计划全程只建本地提交。

---

## 文件结构

| 动作 | 路径 | 职责 |
|---|---|---|
| Modify | `crates/updater/src/update_info.rs` | `UpdateInfo` 增必填 `sha256` + 十六进制校验 |
| Modify | `crates/updater/src/lib.rs` | 下载流哈希与外置 pinned 值比对；抽取可测的 `verify_stream_sha256` |
| Modify | `crates/app/src/state.rs` | `CheckResult`/`UpdateState` 携带 `expected_sha256` |
| Modify | `crates/app/src/runtime/update.rs` | 下载与写 manifest 都使用 pinned 值 |
| Modify | `.github/workflows/ci.yml` | 发布作业从本地产物算 sha256 写进 `update.json` |
| Modify | `docs/RELEASE.md` | 更正「SHA256 校验」与「BASE 未启用」两处失真描述 |
| Modify | `crates/lua_host/src/lib.rs` | `harden_globals()` 抹掉 `dofile/loadfile/load`；重写恒真沙箱测试 |
| Modify | `crates/lua_host/src/mlua_engine.rs` | `Lua::new()` → 与沙箱同一 stdlib 子集 |
| Modify | `crates/transport/src/lib.rs` | `AliveGuard`（Drop 置 false）+ `port_is_dead()` 双信号 |
| Modify | `crates/recorder/src/recorder.rs` | 每帧轮询 `join.is_finished()`，panic 不再伪装成"正在录制" |
| Modify | `crates/core/src/lib.rs` | 承接 `parse_hex` / `parse_hex_strict`（唯一真相，native+wasm 均可用） |
| Modify | `crates/transport/src/lib.rs` | 改调 `tool_core` 的解析，删除本 crate 内的实现 |
| Create | `crates/application/src/send_plan.rs` | 无 `cfg` 门控的 `plan_send()`：命令 → `PlannedSend{task_kind, bytes}` |
| Modify | `crates/application/src/lib.rs` | 注册 `pub mod send_plan;`（不受 cfg 门控） |
| Modify | `crates/application/src/workbench.rs` | 三个 Send* 分支改调 `plan_send` |
| Modify | `crates/application/src/web.rs` | 三个 Send* 分支改调 `plan_send`；删除 `:2290`/`:2334` 两份手抄 HEX 解析；新增 `WebApplication::validate_hex` |
| Modify | `crates/app/src/ui/bottom_panel.rs`、`crates/app/src/web.rs` | 改调 application 暴露的 `validate_hex`，消掉第 3/4 份实现 |
| Modify | `crates/app/Cargo.toml` | **不删** `tool-transport`：收敛 HEX 后仍剩 `RepaintWaker`(`app/mod.rs:15`) 与 `natural_sort_key`(`commands.rs:312`) 两处；仅 `[dev-dependencies]` 增 `toml` |
| Create | `crates/app/tests/manifest_deps.rs` | TOML 解析式依赖判定 + 其单元测试 |
| Modify | `crates/app/tests/architecture.rs`、`crates/application/tests/architecture.rs` | 改用真实依赖名判定；禁令覆盖全部可达 crate |
| Modify | `Cargo.toml`（根） | `members` 补 `crates/recorder`；workspace 增 `toml` |

任务顺序按依赖排：Task 1、2、3、4 相互独立可并行；**Task 5 必须先于 Task 6**（Task 6 靠 Task 5 建立的字节级契约证明重构等价）。

---

## Task 1: 自更新哈希改为外置 pinned 值

**为什么**：`update.json` 与 `UpdateInfo`（`crates/updater/src/update_info.rs:9-19`）四个字段里没有任何哈希；`download_update_*` 返回**刚收下那串字节**的哈希（`crates/updater/src/lib.rs:1084`），app 把这个自算值写进 manifest（`crates/app/src/runtime/update.rs:38,110`），`apply_pending_update_impl` 再拿磁盘文件与它比（`crates/updater/src/lib.rs:675-688`）。该比对只能发现下载完成后的磁盘损坏，**从不约束下载了什么内容**。插件市场那条路径是对的（registry 带 `sha256`、与下载流比对），本任务把更新器拉平到同一标准。

**失败关闭的取舍**：`sha256` 设为必填后，现存线上那份不含该字段的 `update.json` 会解析失败 → UI 报「检查更新失败」而不是静默接受。这是刻意的：宁可不更新，不可更新未固定的字节。发布下一次带 `sha256` 的 `update.json` 后自然恢复。

**Files:**
- Modify: `crates/updater/src/update_info.rs:9-19`
- Modify: `crates/updater/src/lib.rs:1078-1088`（下载收尾）
- Modify: `crates/app/src/state.rs:322-328`（`CheckResult`）、`:330`（`impl Default for UpdateState`）
- Modify: `crates/app/src/runtime/update.rs:22-43`、`:68-82`、`:106-112`、`:235` 附近
- Modify: `.github/workflows/ci.yml:240-272`
- Modify: `docs/RELEASE.md:187`
- Test: `crates/updater/src/update_info.rs`（同文件 `mod tests`）、`crates/updater/src/lib.rs`（同文件 `mod tests`，位于 `:1134-1305`）

**Interfaces:**
- Consumes: 无
- Produces：
  - `pub fn is_pinned_sha256(value: &str) -> bool` @ `tool_updater::update_info`
  - `pub fn verify_stream_sha256(actual: &str, expected: &str) -> Result<(), String>` @ `tool_updater`
  - `UpdateInfo.sha256: String`（必填）
  - `CheckResult.sha256: String`
  - `download_update_with_network_settings(url, network, on_progress, expected_sha256: &str)`

- [ ] **Step 1: 写失败测试 —— 缺 `sha256` 的清单必须解析失败**

在 `crates/updater/src/update_info.rs` 的 `mod tests` 中追加（该模块已有 `parse_update_info_empty_changelog` 等用例，沿用其风格）：

```rust
#[test]
fn update_info_requires_sha256_field() {
    // 缺 sha256 必须硬失败：静默接受 = 自我循环校验的旧行为回来了。
    let missing = r#"{"version":"1.3.0","date":"2026-09-19","download_url":"https://github.com/Tydwdh/serial_tool/releases/download/v1.3.0/hardware-workbench-app.zip","changelog":[]}"#;
    let error = serde_json::from_str::<UpdateInfo>(missing)
        .expect_err("缺少 sha256 字段的 update.json 必须解析失败");
    assert!(
        error.to_string().contains("sha256"),
        "错误信息应点名缺失字段，实际：{error}"
    );
}

#[test]
fn update_info_rejects_malformed_sha256() {
    for bad in ["", "deadbeef", &"a".repeat(63), &"g".repeat(64)] {
        assert!(
            !is_pinned_sha256(bad),
            "{bad:?} 不是合法的 64 位十六进制摘要"
        );
    }
    assert!(is_pinned_sha256(&"a".repeat(64)));
    assert!(is_pinned_sha256(&"0123456789ABCDEF".to_owned() + &"f".repeat(48)));
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cargo +1.92.0 test -p tool-updater update_info_ > /tmp/t1.log 2>&1; echo "EXIT=$?"
```
预期：编译失败（`is_pinned_sha256` 未定义）→ 这就是本步的"失败"。

- [ ] **Step 3: 实现必填字段与谓词**

把 `crates/updater/src/update_info.rs:9-19` 的结构体改为：

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct UpdateInfo {
    /// 最新版本号，如 "0.3.0"
    pub version: String,
    /// 发布日期，如 "2026-06-25"
    pub date: String,
    /// 下载 URL（指向 GitHub Release 的 zip）
    pub download_url: String,
    /// 发布产物的 SHA256。**必填**：更新包字节必须与这个外置固定值一致，
    /// 否则校验退化成"拿下载内容跟下载内容比"，只约束发布仓库写权限之外的人。
    pub sha256: String,
    /// 更新日志
    #[serde(default)]
    pub changelog: Vec<String>,
}

/// 是否为可信的 SHA256 固定值：恰好 64 个十六进制字符。
pub fn is_pinned_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
```

在 `fetch_update_info_with_network_settings` 解析出 `UpdateInfo` 之后（该函数以 `let info = serde_json::from_str(...)` 收尾，位于文件 `:30` 之后）插入立即拒绝：

```rust
    if !is_pinned_sha256(&info.sha256) {
        return Err(format!(
            "update.json 的 sha256 不是合法的 64 位十六进制摘要（得到 {:?}），拒绝更新",
            info.sha256
        ));
    }
```

- [ ] **Step 4: 抽出可测的比对函数并在下载收尾调用**

`crates/updater/src/lib.rs` 中，`sha256_file` 附近（`:1084` 上方）新增公开谓词：

```rust
/// 下载流哈希与外置 pinned 值的唯一比对点。
/// 独立成函数是为了让"不匹配必须拒绝"这一条可被单测直接命中。
pub fn verify_stream_sha256(actual: &str, expected: &str) -> Result<(), String> {
    if !is_pinned_sha256(expected) {
        return Err(format!("拒绝校验：更新清单的 sha256 不合法（{expected:?}）"));
    }
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "更新包校验失败：SHA256 不匹配（清单声明 {expected}，实际下载 {actual}）"
        ));
    }
    Ok(())
}
```

在 `crates/updater/src/lib.rs` 顶部 `use` 处补 `use update_info::is_pinned_sha256;`（若已通过 `pub use update_info::*` 引入则无需改动——先跑 `grep -n 'update_info::' crates/updater/src/lib.rs | head` 确认）。

把 `download_update_with_network_settings`（签名在 `:978`）的结尾 `:1083-1086`：

```rust
    let hash = format!("{:x}", hasher.finalize());
    Ok(hash)
}
```

改为（新增 `expected_sha256` 参数，校验发生在 `.part → 最终名` rename **之前**）：

```rust
    let hash = format!("{:x}", hasher.finalize());
    // 先比对 rename：不匹配就不留下任何可被 apply 读到的文件。
    verify_stream_sha256(&hash, expected_sha256)?;
    Ok(hash)
}
```

并把 `std::fs::rename(&part_path, dest_path)` 那一段（`:1078-1082`）移到校验之后 —— 即最终形如：

```rust
    let hash = format!("{:x}", hasher.finalize());
    verify_stream_sha256(&hash, expected_sha256)?;

    if let Err(e) = std::fs::rename(&part_path, dest_path) {
        cleanup_partial_download(&part_path);
        return Err(format!("重命名下载文件失败：{e}"));
    }
    Ok(hash)
```

同步 `download_update`（`:974`）的签名与转发：

```rust
pub fn download_update(url: &str, on_progress: impl Fn(u64, u64), expected_sha256: &str) -> Result<String, String>
```

- [ ] **Step 5: 写比对与 apply 拒绝的失败测试**

`crates/updater/src/lib.rs` 的 `mod tests`（用例区在 `:1134-1305`）追加：

```rust
#[test]
fn verify_stream_sha256_rejects_mismatch_and_malformed_pin() {
    let pin = "a".repeat(64);
    assert!(verify_stream_sha256(&pin, &pin).is_ok());
    assert!(verify_stream_sha256(&"b".repeat(64), &pin).is_err());
    assert!(verify_stream_sha256(&pin, &pin.to_uppercase()).is_ok(), "比对须大小写无关");
    // 关键：pinned 值本身不合法时不得放行 —— 空串/短串都不能当通行证。
    assert!(verify_stream_sha256(&pin, "").is_err());
    assert!(verify_stream_sha256(&pin, "deadbeef").is_err());
}
```

- [ ] **Step 6: 跑测试确认通过**

```bash
cargo +1.92.0 test -p tool-updater > /tmp/t1.log 2>&1; echo "EXIT=$?"; grep -E 'test result|FAILED' /tmp/t1.log
```
预期：`tool-updater` 全绿（原 23 条 + 新增 3 条）。

- [ ] **Step 7: 把 pinned 值穿过 app**

`crates/app/src/state.rs:322-328` 的 `CheckResult` 增字段：

```rust
pub(crate) struct CheckResult {
    pub(crate) version: String,
    pub(crate) download_url: String,
    /// 来自 update.json 的外置固定摘要：下载与写 manifest 都以它为准。
    pub(crate) sha256: String,
    pub(crate) changelog: Vec<String>,
    /// 是否已缓存跳过（无需更新 UI）
    pub(crate) cached: bool,
}
```

`UpdateState`（同文件，`impl Default for UpdateState` 在 `:330`）新增：

```rust
    /// 本次待装包的 pinned 摘要（来自 update.json），**不是**下载自算值。
    pub(crate) expected_sha256: Option<String>,
```
并在 `Default` 构造里补 `expected_sha256: None,`。

`crates/app/src/runtime/update.rs`：
- 收割检查结果的 `Ok(Ok(result))` 分支（`:68-82`）里，在 `self.update_state.update_available = true;` 旁加 `self.update_state.expected_sha256 = Some(result.sha256.clone());`
- 启动下载处（`:235` 的 `thread::spawn`）把 pinned 值 move 进闭包并传给 `download_update(..., expected)`；闭包外的 `self.update_state.expected_sha256.clone()` 若为 `None`，直接置 `error` 并 return，**不得**回退到自算值。
- 用户点「更新并重启」分支（`:22-27`）：把 `.zip(self.update_state.downloaded_sha256.as_ref())` 改为 `.zip(self.update_state.expected_sha256.as_ref())`。
- `:106` 的 `Ok(Ok(actual_sha256))` 仅用于 UI 进度，保留；但不再参与 `write_update_manifest`。

- [ ] **Step 8: 让 `check_update` 回填 sha256**

`crates/app/src/runtime/update.rs:162` 的 `check_handle = Some(std::thread::spawn(...))` 闭包内构造 `CheckResult` 处，把 `UpdateInfo.sha256` 带上。缓存分支（`result.cached == true`）也必须有值：`CheckCache` 存的版本号需同时校验其 `sha256` 非空，否则当作未命中重新拉取 —— 先 `grep -n 'struct CheckCache' -A 10 crates/updater/src/lib.rs` 确认字段，若无 `sha256` 则在该结构上加 `pub sha256: String` 并沿用 `#[serde(default)]`（缓存是本机可丢弃数据，不是信任根，允许默认值触发重取）。

- [ ] **Step 9: 编译门 + 全量测试**

```bash
cargo +1.92.0 fmt --all && cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c1.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t1.log 2>&1; echo "TEST=$?"
grep -E 'test result:.*[1-9] failed|FAILED' /tmp/t1.log | head
```
预期：两个 0；`test result` 无 failed 行；总数 ≥ 506 + 3。

- [ ] **Step 10: CI 发布步骤算出真实摘要写进 update.json**

`.github/workflows/ci.yml` 的 release 作业内，`$downloadUrl = ...` 那行（`:245`）之后插入：

```powershell
          $zipPath = "dist/hardware-workbench-app.zip"
          if (-not (Test-Path -LiteralPath $zipPath)) {
            throw "release 产物缺失：$zipPath"
          }
          $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $zipPath).Hash.ToLowerInvariant()
          if ($sha256.Length -ne 64) { throw "SHA256 计算异常：$sha256" }
          Write-Host "update.json pinned sha256: $sha256"
```

并把 `$json = @{ ... }`（`:265-270`）补上一项：

```powershell
          $json = @{
            version = $version
            date = $date
            download_url = $downloadUrl
            sha256 = $sha256
            changelog = @($changelog)
          } | ConvertTo-Json
```

- [ ] **Step 11: 更正文档中不实的两处描述**

`docs/RELEASE.md:187` 附近把"SHA256 校验"改写为：更新包摘要来自 `update.json` 的 `sha256` 字段（发布作业由 `dist/hardware-workbench-app.zip` 计算后写入），下载流与 apply 前各比对一次；并明确当前**无代码签名**，信任根是 GitHub 仓库写权限 + TLS。

- [ ] **Step 12: 提交**

```bash
git add crates/updater/src/update_info.rs crates/updater/src/lib.rs crates/app/src/state.rs \
        crates/app/src/runtime/update.rs .github/workflows/ci.yml docs/RELEASE.md Cargo.toml
git commit -m "fix: pin update payload to a hash published outside the download"
```

**反向自检（评审员会做）**：把 `verify_stream_sha256` 的不匹配分支整块删掉 → `verify_stream_sha256_rejects_mismatch_and_malformed_pin` 必须变红；把 `UpdateInfo.sha256` 改回 `#[serde(default)]` → `update_info_requires_sha256_field` 必须变红。两处都验完再还原。

---

## Task 2: 抹掉沙箱里 base 的文件加载全局，并让测试真的能失败

**为什么**：`crates/lua_host/src/lib.rs:2107` 的 `sandbox_dofile_not_available` 断言的是 `pcall` **报错了**，而 `crates/lua_host/secret.txt` 不存在（已核实），所以 `dofile` 被禁与 `dofile` 能用两种状态都让 `ok == false`。更根本的是：测试体自己用了 `pcall` 和 `assert`，二者同属 Lua 5.4 `base_funcs`，**测试能通过恰好证明 `base` 已加载** —— mlua 在 `state/raw.rs:139` 无条件 `luaL_requiref(state, "_G", luaopen_base, 1)`，`StdLib` 位掩码只能增加标准库、删不掉 base。因此 `dofile`/`loadfile`/`load` 对任何插件可达：可读并执行宿主上任意 `.lua`。注释里 `StdLib::BASE 未启用` 指的是 mlua 0.11 中不存在的 API。

**Files:**
- Modify: `crates/lua_host/src/lib.rs`（两处 `Lua::new_with`：`:415-423`、`:940-948`；测试：`:2087-2126`）
- Modify: `crates/lua_host/src/mlua_engine.rs:53-58`
- Modify: `docs/RELEASE.md:200-203`（承诺了 `BASE` 被禁用）
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: 无
- Produces: `fn harden_globals(lua: &Lua) -> mlua::Result<()>`（`crates/lua_host/src/lib.rs` 内 `pub(crate)`）

- [ ] **Step 1: 写能真正区分的失败测试**

在 `crates/lua_host/src/lib.rs` 的 `mod tests` 内，**替换** `sandbox_dofile_not_available`（`:2107-2126`）为下面三条。注意：断言必须是「函数不存在」，且同时确认 `pcall`/`assert` 仍在（否则我们只是把 VM 弄坏了）。

```rust
#[test]
fn sandbox_base_file_loaders_are_absent() {
    let bus = DataBus::new();
    let transport = TransportManager::new(bus.clone());
    // 断言"全局不存在"，不是"调用报错了"：后者在文件缺失时与被禁不可区分。
    let result = run_script_for_test(
        r#"
assert(dofile == nil, "dofile must be nil, got " .. type(dofile))
assert(loadfile == nil, "loadfile must be nil, got " .. type(loadfile))
assert(load == nil, "load must be nil, got " .. type(load))
-- base 本身不能没：pcall/assert/type 都是 base_funcs 的成员
assert(type(pcall) == "function", "pcall must survive")
assert(type(assert) == "function", "assert must survive")
"#,
        bus,
        transport,
    );
    assert!(result.is_ok(), "沙箱基线断言失败：{result:?}");
}

#[test]
fn sandbox_cannot_require_os_or_io() {
    let bus = DataBus::new();
    let transport = TransportManager::new(bus.clone());
    // PACKAGE 是启用的（:419/:944），所以 require 是真实逃逸面。
    let result = run_script_for_test(
        r#"
for _, name in ipairs({"os", "io", "debug", "ffi"}) do
    local ok, mod = pcall(require, name)
    assert(not ok or mod == nil,
        string.format("require(%q) must not yield a live module", name))
end
assert(pcall(require, "hw.codec") == true or true)
"#,
        bus,
        transport,
    );
    assert!(result.is_ok(), "require 逃逸用例失败：{result:?}");
}

#[test]
fn sandbox_cannot_rebind_globals_to_escape_harden() {
    let bus = DataBus::new();
    let transport = TransportManager::new(bus.clone());
    // load(string) 被抹掉后，插件不得还有办法从字符串造 chunk。
    let result = run_script_for_test(
        r#"
assert(load == nil and loadfile == nil and dofile == nil)
assert(pcall(function() return loadstring("return 1") end) == false)
assert(package.loadlib == nil or pcall(package.loadlib, "x.so", "y") == false)
"#,
        bus,
        transport,
    );
    assert!(result.is_ok(), "字符串→chunk 逃逸未被挡住：{result:?}");
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cargo +1.92.0 test -p tool-lua-host sandbox_ > /tmp/t2.log 2>&1; echo "EXIT=$?"; grep -E 'test result|panicked|must be nil' /tmp/t2.log | head
```
预期：`sandbox_base_file_loaders_are_absent` **FAIL**，报错信息含 `dofile must be nil, got function`。这一步是立论证据，必须留档到提交信息里。

- [ ] **Step 3: 实现 harden_globals**

`crates/lua_host/src/lib.rs` 中，紧接 `plugin_event_loop` 之前新增：

```rust
/// mlua 在建 state 时无条件打开 `base`（`luaL_requiref(..., luaopen_base, ...)`），
/// `StdLib` 位掩码只能增删其它标准库、无法移除 base。Lua 5.4 的 `dofile`、`loadfile`、
/// `load` 都在 `base_funcs` 里，因此必须显式抹掉：否则插件可读取并执行宿主文件系统上
/// 的任意 `.lua`（含其它插件源码），沙箱边界形同虚设。
/// Web 端 `omnilua` 走 `remove_globals` 配置（`plugin_runtime/src/web_lua.rs:84-97`），
/// 本函数让 native 与它对等。
pub(crate) fn harden_globals(lua: &Lua) -> mlua::Result<()> {
    for name in ["dofile", "loadfile", "load"] {
        lua.globals().set(name, mlua::Value::Nil)?;
    }
    Ok(())
}
```

在 **两处** `Lua::new_with(...)` 成功之后立刻调用：
- `plugin_event_loop`：`:426-434` 的 `Ok(lua) => { ... }` 之前无法插入（那是 `match` 分支），故在 `let lua = match ... };` 整段之后、`// 安装指令 hook` 之前加：
  ```rust
    if let Err(error) = harden_globals(&lua) {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("加固 Lua 全局失败：{error}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }
  ```
- `run_script_blocking`：`:940-948` 的 `let lua = Lua::new_with(...)?;` 之后加 `harden_globals(&lua)?;`

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo +1.92.0 test -p tool-lua-host > /tmp/t2.log 2>&1; echo "EXIT=$?"; grep -E 'test result|FAILED' /tmp/t2.log
```
预期：`tool-lua-host` 119 + 3 全绿。**若 `sandbox_cannot_require_os_or_io` 变红**（即 `require("os")` 真返回了模块），不要放宽测试 —— 转而把 `StdLib::PACKAGE` 的 searcher 收紧（`package.loaders`/`searchers` 只保留 preload 与已冻结项），并留一份说明。

- [ ] **Step 5: 摘掉 mlua_engine 里的上膛枪**

`crates/lua_host/src/mlua_engine.rs:53-58` 现在用 `Lua::new()`，即 `ALL_SAFE`，含 `IO` 与 `OS`（`os.execute` 可用）。当前无生产构造点，但它是 `pub use` 导出的（`crates/lua_host/src/lib.rs:46`）且 `docs/plugin-api-v2.md` 把它描述为 native 运行时适配入口。改为与沙箱同一子集：

```rust
        // 与 plugin_event_loop / run_script_blocking 同一 stdlib 子集：
        // Lua::new() 会开 ALL_SAFE（含 IO/OS），marketplace 插件一旦接上就是 os.execute。
        let lua = Lua::new_with(
            StdLib::TABLE
                | StdLib::STRING
                | StdLib::MATH
                | StdLib::UTF8
                | StdLib::PACKAGE
                | StdLib::COROUTINE,
            LuaOptions::default(),
        )?;
        crate::harden_globals(&lua)?;
```
若 `harden_globals` 的返回类型是 `mlua::Result`，按其错误类型调整 `?`；`mlua_engine` 里若已有 `.map_err(...)` 约定则沿用。`StdLib`/`LuaOptions` 未 import 时补 `use mlua::{Lua, LuaOptions, StdLib};`。

- [ ] **Step 6: 加一条静态守卫，禁止再出现 `Lua::new()`**

`crates/lua_host/src/lib.rs` 的 `mod tests` 追加：

```rust
#[test]
fn production_code_never_opens_all_stdlibs() {
    // Lua::new() == ALL_SAFE（含 IO/OS）。沙箱只允许 new_with(子集) + harden_globals。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut checked = 0usize;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("读取 src 目录失败") {
            let path = entry.expect("目录项").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            checked += 1;
            let text = std::fs::read_to_string(&path).expect("读取源文件失败");
            for (index, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains("Lua::new()") {
                    offenders.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        }
    }
    assert!(checked >= 20, "扫描到的 .rs 文件过少（{checked}），守卫可能失效");
    assert!(offenders.is_empty(), "生产代码不得用 Lua::new() 打开全部标准库：{offenders:?}");
}
```
确认该测试模块能用到 `std::path::Path`（文件头若只 `use std::sync::Arc` 之类别名，则在测试内 `use std::path::Path;`）。

- [ ] **Step 7: 更正注释与文档**

删除 `crates/lua_host/src/lib.rs:2111` 的 `(StdLib::BASE 未启用)` 说法（该 API 在 mlua 0.11 不存在）；`docs/RELEASE.md:200-203` 的"BASE 未启用 / dofile、loadfile 不可用"改写为实际机制：base 由 mlua 无条件打开，`dofile/loadfile/load` 由 `harden_globals()` 显式置 nil。

- [ ] **Step 8: 全量门 + 提交**

```bash
cargo +1.92.0 fmt --all && cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c2.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t2.log 2>&1; echo "TEST=$?"
git add crates/lua_host/src/lib.rs crates/lua_host/src/mlua_engine.rs docs/RELEASE.md
git commit -m "fix: remove base file loaders from the Lua sandbox and unbreak its test"
```

**反向自检**：临时把 `harden_globals` 的循环体注释掉 → `sandbox_base_file_loaders_are_absent` 必须 FAIL（错误含 `dofile must be nil, got function`）。还原后确认 `git status` 干净。

---

## Task 3: worker panic 不再留下僵尸端口 / 假录制

**为什么**：串口的 `alive.store(false)` 只写在显式 `return` 处（`crates/transport/src/lib.rs:1154`、`:1199`、尾部 `:1202`），panic 展开会跳过全部三处。后果是链式的：`reap_dead_ports` 用 `!h.alive` 过滤（`:1051`）→ 僵尸项永不匹配；`open_serial` 见 `existing.alive==true` 且配置相同就**复用死句柄并返回 Ok**（`:444-446`）→ 用户连"关掉重开"都做不到；发送命令进 `bounded(1024)` 通道无人取，UI 一路显示已发直到通道满。全工作区**没有任何 panic hook**。recorder 同构：`is_running()` 返回 `self.worker.is_some()`（`crates/recorder/src/recorder.rs:445-447`），即"用户没点停止"，而 worker 的 `finished` 标志同样只在闭包尾部 `:342` 写入。

**Files:**
- Modify: `crates/transport/src/lib.rs`（`PortHandle` `:309-320`、`reap_dead_ports` `:1047-1060`、`serial_worker_loop_impl` `:1138-1203`）
- Modify: `crates/recorder/src/recorder.rs`（`tick_backpressure` `:403`、`is_running` `:445`）
- Test: 两文件各自 `mod tests` / `#[cfg(test)]` 区（transport 已有 `serial_worker_loop_impl` 直调测试：`:1763`、`:1822`、`:1875`）

**Interfaces:**
- Consumes: 无
- Produces:
  - `struct AliveGuard(Arc<AtomicBool>)` + `impl Drop`（`crates/transport/src/lib.rs`，私有）
  - `fn port_is_dead(handle: &PortHandle) -> bool`（同文件，私有但可单测）
  - `JsonlRecorder::worker_panicked(&self) -> bool`（`crates/recorder/src/recorder.rs`，私有）

- [ ] **Step 1: 写失败测试 —— panic 的 worker 必须让 alive 变 false**

`crates/transport/src/lib.rs` 的测试区（紧邻 `:1763` 那组 `serial_worker_loop_impl` 直调测试）。先复制 `MockSerialPort`（`:1680-1732`）改出一个 `PanickingPort`，其 `read` 第一次调用即 panic：

```rust
    #[test]
    fn panicked_worker_clears_alive_flag() {
        // 现场复现 C4：worker panic 时显式 alive.store 全被跳过，
        // 端口既不被回收也不能重开。Drop 守卫必须补上这条路径。
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = Arc::clone(&alive);
        let (writer, command_rx) = bounded::<SerialCommand>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let join = std::thread::spawn(move || {
            serial_worker_loop_impl(
                PanickingPort,
                command_rx,
                stop,
                thread_alive,
                DataBus::new(),
                "serial:PANIC_TEST".to_owned(),
                None,
            )
        });
        assert!(join.join().is_err(), "mock 端口必须让 worker panic");
        assert!(
            !alive.load(Ordering::Acquire),
            "worker panic 后 alive 必须为 false，否则端口成为不可恢复的僵尸"
        );
        drop(writer);
    }
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cargo +1.92.0 test -p tool-transport panicked_worker > /tmp/t3.log 2>&1; echo "EXIT=$?"; grep -E 'alive 必须为 false|test result' /tmp/t3.log
```
预期：FAIL，断言消息 `worker panic 后 alive 必须为 false`。

- [ ] **Step 3: 实现 AliveGuard 并改用双信号回收**

`crates/transport/src/lib.rs` 内，`serial_worker_loop_impl` 上方加：

```rust
/// worker 线程退出（正常返回 **或 panic 展开**）时把 alive 置 false。
/// 显式 `alive.store(false)` 只覆盖 return 路径，panic 会全部跳过，
/// 于是 reap 匹配不到、同名重开复用死句柄 —— 端口成为不可恢复的僵尸。
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
```

`serial_worker_loop_impl` 函数体第一行（`:1148` 的 `let mut buffer = [0_u8; 4096];` 之前）：

```rust
    let _alive_guard = AliveGuard(Arc::clone(&alive));
```

删除三处冗余显式写入，只保留 `return` 与循环出口：`:1154` 的 `alive.store(false, Ordering::Release);`、`:1199` 同名行、以及函数尾 `:1202` 的同名行。改完后：
- `:1153-1155` 的失败分支变成 `{ return; }` 形式（保留 `if ... .is_err() { return; }`）
- `Err(error) => { bus.publish(...); return; }`
- 循环正常结束后函数直接结束

`PortHandle` 判定改为两个独立信号（`reap_dead_ports` 内 `:1051` 的过滤）：

```rust
/// 端口是否已死亡。两个独立信号，任一成立即回收：
/// `alive`（worker 主动/被 Drop 置位）与 `join.is_finished()`（含 panic 展开，
/// 不依赖 worker 自己写了什么）。只有前者时会因遗忘置位而漏掉。
fn port_is_dead(handle: &PortHandle) -> bool {
    !handle.alive.load(Ordering::Acquire)
        || handle
            .join
            .as_ref()
            .is_some_and(|join| join.is_finished())
}
```

并把 `reap_dead_ports` 里的闭包换成它：

```rust
                .filter(|(_, h)| port_is_dead(h))
```

- [ ] **Step 4: 单测 port_is_dead 的两个信号**

```rust
    #[test]
    fn port_is_dead_accepts_finished_join_without_alive_flag() {
        let handle = PortHandle {
            config: SerialConfig::default(),  // 若无可用的 Default，用测试内已有的构造
            connecting: Arc::new(AtomicBool::new(false)),
            writer: bounded::<SerialCommand>(1).0,
            #[cfg(windows)]
            wake: None,
            stop: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(true)), // 故意留在 true：模拟 worker 忘了置位
            join: Some(std::thread::spawn(|| {})),
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(port_is_dead(&handle), "线程已退出即视为端口死亡，与 alive 标志无关");
    }
```

`SerialConfig` 若无 `Default`，改用该文件测试区里已存在的构造方式（先 `grep -n 'fn default\|impl Default for SerialConfig\|SerialConfig {' crates/transport/src/lib.rs | head`）。

- [ ] **Step 5: recorder 每帧轮询 worker 存活**

`crates/recorder/src/recorder.rs`：

```rust
    /// worker 线程已退出但用户从未点停止 ⇒ panic 展开。
    /// 只看 `worker.is_some()` 会把崩溃的 recorder 报告成"正在录制"，
    /// 用户失去的正是他此行的目的（本次会话的抓取）。
    fn worker_panicked(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            worker
                .join
                .as_ref()
                .is_some_and(|join| join.is_finished())
        })
    }
```

在 `tick_backpressure`（`:403`）**最开头**、早于 `let Some(backlog) = self.backlog.clone() else { return; }` 插入（该早退会让未启用积压监控的录制路径完全跳过存活检查）：

```rust
        if self.worker_panicked() {
            let reason = "录制线程异常退出（panic），本次录制不完整".to_owned();
            self.stop_with_reason(true, Some(reason.clone()));
            self.bus
                .publish(Event::system_log(LogLevel::Error, "recorder", reason));
            return;
        }
```

`stop_with_reason`（`:367`）内部 `self.worker.take()` 在 panic 路径上安全（只置 `stop` 并移交 join，不 `join()` UI 线程）。

- [ ] **Step 6: 单测 worker_panicked 谓词**

```rust
    #[test]
    fn worker_panicked_is_false_before_start_and_true_after_thread_exit() {
        let bus = DataBus::new();
        let mut recorder = JsonlRecorder::new(bus.clone());
        assert!(!recorder.worker_panicked(), "未启动时不得报告 panic");

        let path = std::env::temp_dir().join(format!("hw-rec-panic-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        recorder.start(path.clone()).expect("启动录制");
        assert!(!recorder.worker_panicked(), "正常录制中不得误报");
        recorder.stop();
        std::fs::remove_file(&path).ok();
    }
```

若 `JsonlRecorder::new`/`start`/`stop` 签名与此不符，以该文件既有测试（`crates/recorder/src/recorder.rs` 测试区）的实际调用形式为准，但**必须保留"未启动=false"**这条断言。谓词的 panic=true 分支由 Step 5 的实现与 `is_finished()` 语义保证，本任务不构造真实 worker panic（无法从外部触发），在提交信息中如实写明"wired path verified by inspection"。

- [ ] **Step 7: 全量门 + 提交**

```bash
cargo +1.92.0 fmt --all && cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c3.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t3.log 2>&1; echo "TEST=$?"
grep -E 'test result:.*[1-9] failed|FAILED' /tmp/t3.log | head
git add crates/transport/src/lib.rs crates/recorder/src/recorder.rs
git commit -m "fix: treat a panicked transport or recorder worker as a dead port"
```

**反向自检**：删掉 `AliveGuard` 的 `Drop` 实现体 → `panicked_worker_clears_alive_flag` 必须 FAIL。把 `port_is_dead` 的 `is_finished()` 半边删掉、并把 mock 的 alive 手工留 `true` → `port_is_dead_accepts_finished_join_without_alive_flag` 必须 FAIL。

---

## Task 4: 架构守卫从「文本匹配」改为「解析后的真实依赖名」

**为什么**：两份守卫都在拿 `Cargo.toml` 做字符串判定，实证有两个绕过向量（评审员在仓库外副本验证）：
```toml
my-transport = { package = "tool-transport", path = "../transport" }   # 判定为「无该依赖」
"tool-transport" = { path = "../transport" }                            # 判定为「无该依赖」
```
两者都是合法清单，都能让 `panels_manifest_has_no_domain_dependencies` 保持绿色而 `crates/panels` 拿到完整领域访问权。同时 `application/tests/architecture.rs:28-33` 只查 11 个可达内部 crate 中的 4 个，而 `tool-platform` 是 `tool-application` 与 `tool-panels` 的**无条件共同直接依赖**（`application/Cargo.toml:24`、`panels/Cargo.toml:26`）—— 它一旦引入 egui，边界的传递闭包立刻被污染且两份守卫都不会红。另一个矛盾：`application/tests/architecture.rs:19` 用裸 `text.contains(banned)`，而同仓 `app/tests/architecture.rs:20-21` 明确写了裸子串会给出「恒真（或恒假）的假证据」。

**Files:**
- Modify: `Cargo.toml:32-49`（workspace deps 增 `toml`）
- Modify: `crates/app/Cargo.toml`（dev-deps 增 `toml`）
- Create: `crates/app/tests/manifest_deps.rs`（共享判定 + 其单元测试）
- Modify: `crates/app/tests/architecture.rs`
- Modify: `crates/application/tests/architecture.rs`
- Modify: `crates/application/Cargo.toml`（dev-deps 增 `toml`）

**Interfaces:**
- Consumes: `toml` crate（已在 `Cargo.lock:4595`，版本 1.1.2）
- Produces（`crates/app/tests/manifest_deps.rs`，`#[path]` 或 `mod` 引入）:
  - `pub fn dependency_names(manifest: &str) -> std::collections::BTreeSet<String>`
  - `pub fn ui_dependency_bans() -> &'static [&'static str]`
  - `pub fn domain_crates_forbidden_in_presentation() -> &'static [&'static str]`

- [ ] **Step 1: 声明 dev-dependency**

根 `Cargo.toml` 的 `[workspace.dependencies]`（`:32-49`）加一行，位置紧随 `serde_json` 保持字母序无破坏：

```toml
toml = "1.1"
```

`crates/app/Cargo.toml` 与 `crates/application/Cargo.toml` 的 `[dev-dependencies]` 各加：

```toml
toml.workspace = true
```

确认 `Cargo.lock` 变化仅为「把 `toml` 挂到两个包的 deps 列表」，无版本升降：
```bash
cargo +1.92.0 metadata --locked --format-version 1 > /dev/null; echo "META=$?"
git diff --stat Cargo.lock   # 预期：只增若干行 "toml"
```

- [ ] **Step 2: 写判定本身的失败测试（先定义「正确」长什么样）**

`crates/app/tests/manifest_deps.rs`：

```rust
//! Cargo.toml 依赖判定：返回**解析后的真实 crate 名**，不是文本包含。
//!
//! 为什么要解析而不是扫行：`{ package = "tool-transport" }` 重命名与
//! `"tool-transport" = {...}` 加引号的键都是合法清单，按声明键文本匹配会漏检，
//! 使 presentation 边界守卫静默保持绿色。

use std::collections::BTreeSet;

/// 收集清单里所有依赖表中的真实 crate 名。
/// 覆盖 `[dependencies] / [dev-dependencies] / [build-dependencies]`
/// 及任意 `[target.<cfg>.*dependencies]`；重命名以 `package` 字段为准。
pub fn dependency_names(manifest: &str) -> BTreeSet<String> {
    let value: toml::Value = manifest
        .parse()
        .unwrap_or_else(|error| panic!("Cargo.toml 解析失败：{error}"));
    let root = value.as_table().expect("Cargo.toml 根必须是 table");
    let mut names = BTreeSet::new();
    collect_dependencies(root, false, &mut names);
    names
}

fn collect_dependencies(table: &toml::table::Table, in_deps: bool, out: &mut BTreeSet<String>) {
    for (key, value) in table {
        let is_deps_table = in_deps
            || key == "dependencies"
            || key == "dev-dependencies"
            || key == "build-dependencies";
        match value {
            toml::Value::Table(inner) if is_deps_table => {
                // 依赖表：键是声明名，真实包名可能被 `package` 覆盖。
                for (dep_key, dep_value) in inner {
                    let real = match dep_value {
                        toml::Value::Table(spec) => spec
                            .get("package")
                            .and_then(|p| p.as_str())
                            .unwrap_or(dep_key.as_str())
                            .to_owned(),
                        _ => dep_key.to_owned(),
                    };
                    out.insert(real);
                }
            }
            toml::Value::Table(inner) if key == "target" || key == "dependencies" => {
                // `target.<cfg>.<...>dependencies` 需要继续下钻。
                collect_dependencies(inner, key == "dependencies", out);
            }
            toml::Value::Table(inner) if in_deps => {
                collect_dependencies(inner, true, out);
            }
            _ => {}
        }
    }
}

/// presentation 禁止直连的领域 crate（唯一真相，两份守卫共用）。
pub const DOMAIN_CRATES: [&str; 6] = [
    "tool-transport",
    "tool-recorder",
    "tool-extension",
    "tool-marketplace",
    "tool-lua-host",
    "tool-updater",
];

/// 任何非 presentation crate 都不得出现的 UI/窗口依赖。
pub const UI_BANS: [&str; 8] = [
    "egui",
    "eframe",
    "egui_tiles",
    "egui_extras",
    "egui_material_icons",
    "rfd",
    "winit",
    "softbuffer",
];

#[test]
fn dependency_names_resolve_renamed_and_quoted_deps() {
    let manifest = r#"
[package]
name = "demo"

[dependencies]
tool-application = { path = "../application" }
"tool-transport" = { path = "../transport" }
my-recorder = { package = "tool-recorder", path = "../recorder" }
serde.workspace = true

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
open = "5"
renamed-market = { package = "tool-marketplace", path = "../marketplace" }

[dev-dependencies]
egui_kittest = "0.35"
"#;
    let names = dependency_names(manifest);
    // 两个历史绕过向量都必须被抓到：
    assert!(names.contains("tool-transport"), "加引号的键漏检：{names:?}");
    assert!(names.contains("tool-recorder"), "package 重命名漏检：{names:?}");
    assert!(names.contains("tool-marketplace"), "target 段重命名漏检：{names:?}");
    assert!(names.contains("tool-application"));
    assert!(names.contains("serde"));
    assert!(names.contains("open"));
    assert!(names.contains("egui_kittest"));
    // 反面：不得把 `[package] name` 或声明名误当成依赖。
    assert!(!names.contains("demo"));
    assert!(!names.contains("my-recorder"), "重命名后应只报告真实包名");
    assert!(!names.contains("renamed-market"));
}
```

- [ ] **Step 3: 跑测试确认失败**

```bash
cargo +1.92.0 test -p hardware-workbench-app --test manifest_deps > /tmp/t4.log 2>&1; echo "EXIT=$?"; grep -E 'test result|漏检' /tmp/t4.log | head
```
预期：编译通过则测试 FAIL（`collect_dependencies` 的下钻逻辑是首版，按报错微调 `target` 分支）。若编译错（`toml::table::Table` 路径不对），改为 `toml::value::Table` 或 `toml::map::Map<String, toml::Value>`。

- [ ] **Step 4: 用判定改写 app 侧守卫**

`crates/app/tests/architecture.rs`：删掉 `declares_dependency`（`:22-26`）与其元测试 `dependency_line_detection_is_exact`（`:29-43`），改为 `mod manifest_deps;`（同目录文件会被 cargo 当作独立 test target，因此正确做法是把 `manifest_deps.rs` 作为**模块 include**）：

```rust
#[path = "manifest_deps.rs"]
mod manifest_deps;

use manifest_deps::{DOMAIN_CRATES, UI_BANS, dependency_names};
```

`panels_manifest_has_no_domain_dependencies` 改为：

```rust
#[test]
fn panels_manifest_has_no_domain_dependencies() {
    let manifest = read_manifest("../panels/Cargo.toml");
    let names = dependency_names(&manifest);
    for forbidden in DOMAIN_CRATES {
        assert!(
            !names.contains(forbidden),
            "crates/panels 必须经由 tool-application 的 DTO 访问领域能力，\
             但 Cargo.toml 实际依赖了 {forbidden}（解析后集合：{names:?}）"
        );
    }
    assert!(
        names.contains("tool-application"),
        "crates/panels 必须显式声明 tool-application（否则是把能力删掉了，而不是收敛到边界）"
    );
}
```

`app_manifest_keeps_the_application_boundary` / `app_manifest_does_not_redeclare_pruned_dependencies` 同步改用 `dependency_names`，并在前者追加 `for forbidden in ["tool-recorder", "tool-extension", "tool-testing"]`。`manifest_deps.rs` 顶部**不能**有 `#[test]` 以外的 test-target 假设 —— 它通过 `#[path]` 被两个 test target 各自编入，因此 `dependency_names_resolve_renamed_and_quoted_deps` 会在两个 target 中各跑一次，这是期望行为。

- [ ] **Step 5: UI 禁令覆盖到全部可达内部 crate**

`crates/application/tests/architecture.rs` 整体重写：删掉硬编码的 4 项清单与裸 `text.contains(banned)`，改为扫描工作区成员、跳过 presentation 两个 crate：

```rust
//! Architecture contract：非 presentation crate 不得引入 UI/窗口依赖。
//!
//! 判定基于解析后的真实依赖名（含 `package = ` 重命名与 `[target.*]` 段），
//! 不是清单文本包含 —— 后者可被两种合法写法绕过，见 manifest_deps.rs 的元测试。

#[path = "../../app/tests/manifest_deps.rs"]
mod manifest_deps;

use manifest_deps::{UI_BANS, dependency_names};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// presentation 层：允许（本就该）依赖 UI。
const PRESENTATION: [&str; 2] = ["crates/app", "crates/panels"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<name> 之上应恰好两级到工作区根")
        .to_path_buf()
}

#[test]
fn no_non_presentation_crate_depends_on_ui() {
    let root = workspace_root();
    let crates_dir = root.join("crates");
    let manifests: Vec<PathBuf> = fs::read_dir(&crates_dir)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", crates_dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path().join("Cargo.toml")))
        .filter(|path| path.is_file())
        .filter(|path| {
            let text = path.display().to_string();
            !PRESENTATION.iter().any(|skip| text.contains(skip))
        })
        .collect();

    // 空集合会让下面的循环变成零断言 —— 正是本仓刚修过的缺陷类别。
    assert!(
        manifests.len() >= 13,
        "只扫到 {} 份清单，路径或过滤条件出错",
        manifests.len()
    );

    let mut checked = 0usize;
    for manifest_path in &manifests {
        let text = fs::read_to_string(manifest_path)
            .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", manifest_path.display()));
        assert!(
            text.contains("[package]"),
            "{} 不是有效的包清单",
            manifest_path.display()
        );
        let names = dependency_names(&text);
        assert!(
            !names.is_empty(),
            "{} 解析出零依赖，判定可能已失效",
            manifest_path.display()
        );
        for banned in UI_BANS {
            assert!(
                !names.contains(banned),
                "{} 不得依赖 {banned}（实际依赖：{names:?}）",
                manifest_path.display()
            );
        }
        checked += 1;
    }
    assert!(checked >= 13, "实际检查 {checked} 份清单，少于预期");
}

#[test]
fn tool_platform_is_ui_free_so_the_boundary_rests_on_it() {
    // tool-platform 是 application 与 panels 的共同基座：它一旦被污染，
    // 两条边的传递闭包同时失守，而任何单清单检查都看不见。
    let root = workspace_root();
    let path = root.join("crates/platform/Cargo.toml");
    let text = fs::read_to_string(&path).expect("读取 tool-platform 清单失败");
    let names = dependency_names(&text);
    for banned in UI_BANS {
        assert!(
            !names.contains(banned),
            "tool-platform 不得依赖 {banned}（实际：{names:?}）"
        );
    }
}
```

注意 `#[path = "../../app/tests/manifest_deps.rs"]` 跨 crate 引用测试目录是刻意为之（避免第三份实现）。若 cargo 拒绝该相对路径，退路是把 `manifest_deps.rs` 移到 `crates/core/src/manifest_deps.rs` 并在 `tool-core` 的 `mod` 中 `#[cfg(test)]`-无关地 `pub` 导出，两处守卫都 `use tool_core::manifest_deps::*`；**不得**为此复制一份实现。

- [ ] **Step 6: 跑测试确认通过，并证明它能红**

```bash
cargo +1.92.0 test --workspace --all-targets > /tmp/t4.log 2>&1; echo "TEST=$?"
grep -E 'test result' /tmp/t4.log | tail -25
```

反向自检（必须真做到，不能只读代码）：临时往 `crates/panels/Cargo.toml` 的 `[dependencies]` 加
```toml
my-transport = { package = "tool-transport", path = "../transport" }
```
跑 `cargo +1.92.0 test -p tool-panels --test architecture 2>&1 | tail -20`（或直接看退出码），**必须 FAIL 且消息点名 tool-transport**。然后还原并确认：
```bash
git diff --stat   # 必须为空
```

- [ ] **Step 7: 补根清单缺员与守卫盲区说明**

根 `Cargo.toml:3-18` 的 `members` 补 `"crates/recorder",`（它目前靠 path-dep 被隐式纳入 —— `cargo metadata --locked --no-deps` 列出 16 个包，但显式清单一旦断裂就会静默丢覆盖）。`docs/ARCHITECTURE.md` 的「验证」段把守卫机制更新为「解析真实依赖名 + 扫描全部非 presentation 成员 + 共同基座单独锁定」。

- [ ] **Step 8: 全量门 + 提交**

```bash
cargo +1.92.0 fmt --all && cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c4.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t4.log 2>&1; echo "TEST=$?"
git add Cargo.toml crates/app/Cargo.toml crates/application/Cargo.toml \
        crates/app/tests/manifest_deps.rs crates/app/tests/architecture.rs \
        crates/application/tests/architecture.rs docs/ARCHITECTURE.md Cargo.lock
git commit -m "test: resolve real dependency names in the architecture guards"
```

---

## Task 5: 发送链路补字节级契约（必须早于 Task 6）

**为什么**：评审员在 `workbench.rs:1071` 插入 `let bytes: Vec<u8> = Vec::new();`，让每一次 `SendText/SendRaw/SendHex` 都**真的投递 0 字节**，跑 `cargo test --workspace --all-targets` 得到 **506 passed / 0 failed / exit 0**。原因是 `send_routing_dispatches_by_port_kind_and_rejects_invalid_hex`（`crates/application/tests/headless.rs:219-338`）只断言「任务种类 == send_serial」以及「所有任务最终 Failed」—— 端口从未打开，所以它证明的是管道存在且失败得体面，从不证明数据到达。用户按下发送键后什么都不发、CI 仍全绿。

**Files:**
- Modify: `crates/application/tests/headless.rs`（新增 1 条用例；`SendHex` 严格/宽松已有断言保留）
- 参考（只读）：`crates/transport/src/network.rs:294-300` 的 `TcpListener::bind("127.0.0.1:0")` + `tungstenite::accept` 回路

**Interfaces:**
- Consumes: `AppCommand::RegisterNetworkPort { config: NetworkSerialConfig }`、`AppCommand::Connect { port, settings }`、`AppCommand::SendHex { port, hex, strict }`、`tool_platform::SerialSettings::default()`（`crates/platform/src/lib.rs:129-137`）
- Produces: `fn send_bytes_reach_a_connected_network_port()` 测试；Task 6 依赖它作为重构等价性的判据

- [ ] **Step 1: 写失败测试**

`crates/application/tests/headless.rs` 末尾追加。**复用**该文件 `:291-304` 已有的 `NetworkSerialConfig { host, port, api_key }` + `display_name()` 构造，只把 `port: 9` 换成真实监听端口：

```rust
#[test]
fn send_bytes_reach_a_connected_network_port() {
    // 这是 send 路径真正的契约：断言对端**收到了哪些字节**。
    // 在此之前所有断言都止于"任务种类对了、最后失败了"，把发送改成投递 0 字节
    // 也能让全工作区保持 506 passed。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local_addr");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut ws = tungstenite::accept(stream).expect("ws handshake");
        let _identify = ws.read().expect("identify frame");
        let mut frames = Vec::new();
        if let Ok(tungstenite::Message::Text(text)) = ws.read() {
            frames.push(text.as_str().to_owned());
        }
        frames
    });

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: addr.port(),
        api_key: None,
    };
    let name = config.display_name();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });
    expect_done(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(name.clone()),
            settings: SerialSettings::default(),
        },
    );

    // 严格 HEX：`AB CD` → [0xAB, 0xCD]；单 nibble 在严格模式下必须被拒。
    expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: PortId::new(name.clone()),
            hex: "AB CD".to_owned(),
            strict: true,
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| wb
            .task_snapshots()
            .iter()
            .any(|task| task.kind == "send_network"
                && task.state == TaskState::Completed)),
        "HEX 发送任务必须 Completed，实际：{:?}",
        wb.task_snapshots()
    );

    let frames = server.join().expect("server thread");
    assert_eq!(frames.len(), 1, "对端应恰好收到一条 gcode 请求");
    let payload: serde_json::Value =
        serde_json::from_str(&frames[0]).expect("server frame is json-rpc");
    let sent = payload
        .pointer("/params/script")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        sent.contains('\u{ab}') && sent.contains('\u{cd}'),
        "发送内容必须含解码后的 0xAB 0xCD 两字节，实际：{sent:?}"
    );
}
```

需要的 import（该文件头部 `use` 区补齐，缺哪个加哪个）：`tungstenite`、`SerialSettings`（来自 `tool_platform`）、`TaskState`、`Duration`、`serde_json`。`tungstenite` 已是 `tool-transport` 的依赖，若 `tool-application` 的 dev-deps 里没有，则在其 `[dev-dependencies]` 加 `tungstenite.workspace = true`（若 workspace deps 未声明，则照 `crates/transport/Cargo.toml` 中的实际版本写并同步 `Cargo.lock`，只增行不改版本）。

- [ ] **Step 2: 跑测试确认它失败或编译报错**

```bash
cargo +1.92.0 test -p tool-application --test headless send_bytes_reach > /tmp/t5.log 2>&1; echo "EXIT=$?"; grep -E 'test result|assertion|panicked' /tmp/t5.log | head
```
预期：若 `tool-transport` 的 WebSocket 客户端对 `printer.gcode.script` 的封装与断言路径不同，本步可能 FAIL 于 `pointer("/params/script")` —— 这时先 `grep -n 'gcode.script' crates/transport/src/network.rs` 取真实 JSON 结构，改正断言路径，**不要**放宽为 `assert!(sent.len() > 0)`。

- [ ] **Step 3: 跑测试确认通过**

```bash
cargo +1.92.0 test -p tool-application --test headless > /tmp/t5.log 2>&1; echo "EXIT=$?"; grep 'test result' /tmp/t5.log
```
预期：`8 passed`（原 7 + 1）。

- [ ] **Step 4: 变异自检 —— 这条测试必须挡住当初那个 mutation**

```bash
# 在 workbench.rs:1071 注入「投递 0 字节」
python3 - <<'PY'
import re,io
p='crates/application/src/workbench.rs'
s=open(p,encoding='utf-8').read()
s=s.replace("    ) -> Result<CommandOutcome, AppError> {\n        if self.is_network_port(&port_name) {",
            "    ) -> Result<CommandOutcome, AppError> {\n        let bytes: Vec<u8> = Vec::new();\n        if self.is_network_port(&port_name) {",1)
open(p,'w',encoding='utf-8').write(s)
PY
cargo +1.92.0 test -p tool-application --test headless send_bytes_reach > /tmp/t5m.log 2>&1; echo "MUTATED_EXIT=$?"
git checkout -- crates/application/src/workbench.rs
git status --porcelain   # 必须为空
```
预期：`MUTATED_EXIT != 0`（测试变红）。若仍为 0，说明这条用例没真正约束字节，回到 Step 2 收紧断言，不得进入下一步。

- [ ] **Step 5: 全量门 + 提交**

```bash
cargo +1.92.0 fmt --all && cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c5.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t5.log 2>&1; echo "TEST=$?"
git add crates/application/tests/headless.rs crates/application/Cargo.toml crates/transport/Cargo.toml Cargo.toml Cargo.lock
git commit -m "test: assert the bytes a send actually delivers"
```

---

## Task 6: 统一 native/web 的 send 路由与 HEX 解析

**为什么**：`dispatch` 有两套实现，且**已实际分叉**（本计划自行核实）：`workbench.rs` 处理 44 个 `AppCommand` 变体、`application/web.rs` 处理 43 个；仅 native 有 `ExportLog`/`ExportTerminal`/`LoadReplay`，仅 web 有 `InstallMarketplacePlugin`/`LoadReplayText`。HEX 解析一共四份：`tool_transport::parse_hex`（`transport/src/lib.rs:1288`）与 `parse_hex_strict`（`:1311`）、`application/web.rs:2290`/`:2334` 两份手抄、以及 `crates/app/src/ui/bottom_panel.rs:617`/`:707` 与 `crates/app/src/web.rs:377` 的 UI 侧调用。后果之一是**同一串输入在 native 判合法、在 web 判非法**。`docs/ARCHITECTURE.md:51-54` 的「如何新增能力」只教人改 `workbench.rs`，而 72% 的 `tool-application` 是平台专属（wasm-only 38% / native-only 34% / shared 28%）。

**规模提示**：这是本计划最大的一项，涉及 6 个文件与一次跨 crate 的函数迁移。若需要分批落地，Step 1-6（`send_plan` 统一 + 消掉 web 的两份手抄）与 Step 7-9（`crates/app` 侧收敛并摘除 `tool-transport` 直连）可各自成提交。

**Files:**
- Modify: `crates/core/src/lib.rs`（承接 `parse_hex`/`parse_hex_strict`）
- Modify: `crates/transport/src/lib.rs:1288-1340`（删除本地实现，改调 `tool_core`）
- Create: `crates/application/src/send_plan.rs`
- Modify: `crates/application/src/lib.rs:20-46`（注册无 cfg 门控的 `pub mod send_plan;`）
- Modify: `crates/application/src/workbench.rs:423-438`、`:1067-1108`
- Modify: `crates/application/src/web.rs:2028-2037`、`:2290-2360`（删两份手抄）
- Modify: `crates/app/src/ui/bottom_panel.rs:617`、`:707`、`:1104`
- Modify: `crates/app/src/web.rs:377`
- Modify: `docs/ARCHITECTURE.md:187-198`（`tool-transport` 直连缺口从 5 处更正为 2 处）
  注：`crates/app/Cargo.toml` 的 `tool-transport` 本任务**不删**，仍剩 `RepaintWaker` 与 `natural_sort_key` 两处引用
- Test: `crates/core/src/lib.rs`（解析单测迁入）、`crates/application/src/send_plan.rs`（`mod tests`）、`crates/application/tests/headless.rs`（Task 5 的用例作等价性判据）

**Interfaces:**
- Consumes: `tool_core::{parse_hex, parse_hex_strict}`；Task 5 的 `send_bytes_reach_a_connected_network_port`
- Produces:
  - `pub fn parse_hex(input: &str) -> Result<Vec<u8>, String>` @ `tool_core`
  - `pub fn parse_hex_strict(input: &str) -> Result<Vec<u8>, String>` @ `tool_core`
  - `pub struct PlannedSend { pub task_kind: &'static str, pub bytes: Vec<u8> }` @ `tool_application::send_plan`
  - `pub enum SendPlanError { InvalidHex(String), Empty }` @ `tool_application::send_plan`（实现 `Display`）
  - `pub fn plan_send(command: &AppCommand, is_network: bool) -> Result<PlannedSend, SendPlanError>`
  - `Workbench::validate_hex(hex: &str, strict: bool) -> Result<Vec<u8>, AppError>`
  - `WebApplication::validate_hex(hex: &str, strict: bool) -> Result<Vec<u8>, String>`

- [ ] **Step 1: 把 HEX 解析下沉到 tool-core（唯一真相）**

`tool-core` 是 native 与 wasm 都无条件依赖的 crate，放这里可同时被 `tool-transport`、`tool-application`、`crates/app` 两侧使用。把 `crates/transport/src/lib.rs:1288-1340` 的 `parse_hex` / `parse_hex_strict` **函数体原样搬入** `crates/core/src/lib.rs`，错误类型从 `TransportResult<Vec<u8>>` 改为 `Result<Vec<u8>, String>`（transport 侧调用点用 `.map_err(TransportError::Io)` 之类既有约定适配）。

搬完后 `crates/transport/src/lib.rs` 内两处改为 `pub use tool_core::{parse_hex, parse_hex_strict};`？—— **不加 re-export**（会造出第二个真相入口）。直接更新调用点：
```bash
grep -rn 'parse_hex' crates/ --include='*.rs' | grep -v 'fn parse_hex'
```
逐条改成 `tool_core::parse_hex` / `tool_core::parse_hex_strict`。把 `crates/transport/src/lib.rs` 中覆盖这两个函数的单测（`:1593-1632` 附近，含 `abc → [0x0a,0xbc]` 的宽松兼容用例）一并移到 `crates/core/src/lib.rs` 的 `mod tests` —— 用例断言逐字保留，只改函数路径。

- [ ] **Step 2: 确认迁移无行为变化**

```bash
cargo +1.92.0 test -p tool-core -p tool-transport > /tmp/t6a.log 2>&1; echo "EXIT=$?"
grep -E 'test result' /tmp/t6a.log
```
预期：两个 crate 全绿，`tool-core` 用例数 = 原 8 + 迁入的解析用例数；`tool-transport` 用例数不减少（迁走的是它内部的测试，若总数下降，必须确认下降数 = 迁入 tool-core 的数，并在提交信息里写明这个等式）。

- [ ] **Step 3: 写 send_plan 的失败测试**

`crates/application/src/send_plan.rs` 先只写类型 + 测试，不写实现（让它编译失败即为红灯）：

```rust
//! 发送命令 → (任务种类, 待投递字节) 的唯一决策点。
//!
//! 本模块**不受 `cfg(target_arch)` 门控**：native 的 `Workbench` 与 wasm 的
//! `WebApplication` 过去各自实现一遍路由与 HEX 解析，两边已经分叉到
//! 「同一串 HEX 在 native 合法、在 web 非法」。字节如何真正投递仍由各平台负责。

use crate::command::AppCommand;
use std::fmt;

/// 路由结果：任务种类标签与实际待发送的字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSend {
    pub task_kind: &'static str,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendPlanError {
    /// HEX 内容非法（严格模式下的奇数 nibble 等）。
    InvalidHex(String),
    /// 命令不是发送类命令。
    NotASend,
}

impl fmt::Display for SendPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex(message) => write!(f, "HEX 解析失败：{message}"),
            Self::NotASend => write!(f, "该命令不是发送类命令"),
        }
    }
}

impl std::error::Error for SendPlanError {}

/// `is_network` 决定任务种类；字节编码规则两平台共用。
pub fn plan_send(command: &AppCommand, is_network: bool) -> Result<PlannedSend, SendPlanError> {
    let bytes = match command {
        AppCommand::SendText { text, .. } => text.clone().into_bytes(),
        AppCommand::SendHex { hex, strict, .. } => {
            let parsed = if *strict {
                tool_core::parse_hex_strict(hex)
            } else {
                tool_core::parse_hex(hex)
            }
            .map_err(SendPlanError::InvalidHex)?;
            parsed
        }
        AppCommand::SendRaw { bytes, .. } => bytes.clone(),
        _ => return Err(SendPlanError::NotASend),
    };
    Ok(PlannedSend {
        task_kind: if is_network { "send_network" } else { "send_serial" },
        bytes,
    })
}
```

同文件 `mod tests`（真值表就是分叉点，逐格都必须钉住）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::PortId;

    fn hex(value: &str, strict: bool) -> AppCommand {
        AppCommand::SendHex { port: PortId::new("COM1"), hex: value.to_owned(), strict }
    }

    #[test]
    fn network_flag_selects_the_task_kind() {
        let command = AppCommand::SendText { port: PortId::new("COM1"), text: "AT\r\n".into() };
        assert_eq!(
            plan_send(&command, false).unwrap(),
            PlannedSend { task_kind: "send_serial", bytes: b"AT\r\n".to_vec() }
        );
        assert_eq!(
            plan_send(&command, true).unwrap().task_kind,
            "send_network"
        );
    }

    #[test]
    fn strict_and_lenient_agree_on_every_token() {
        // 分叉的真实形状：同一输入两平台判定不同。这里钉住唯一判定。
        for (input, strict_ok) in [("AB CD", true), ("abc", false), ("AB C", false), ("ZZ", false)] {
            let strict = plan_send(&hex(input, true), false).is_ok();
            assert_eq!(strict, strict_ok, "{input:?} 的严格模式判定错了");
            assert!(
                plan_send(&hex(input, false), false).is_ok() || !strict_ok,
                "{input:?} 宽松模式应当比严格模式更宽容"
            );
        }
    }

    #[test]
    fn bytes_are_decoded_not_passed_through() {
        let plan = plan_send(&hex("AB CD", true), false).unwrap();
        assert_eq!(plan.bytes, vec![0xAB, 0xCD], "HEX 必须解码为两字节");
    }

    #[test]
    fn non_send_commands_are_rejected() {
        let command = AppCommand::SetDtr { port: PortId::new("COM1"), value: true };
        assert_eq!(plan_send(&command, false), Err(SendPlanError::NotASend));
    }
}
```

- [ ] **Step 4: 注册模块并跑测试**

`crates/application/src/lib.rs` 的模块声明区（`:20-46` 全部带 `cfg` 的那一段）**之外**加一行，确保两平台都编译它：

```rust
pub mod send_plan;
```

```bash
cargo +1.92.0 test -p tool-application send_plan > /tmp/t6b.log 2>&1; echo "EXIT=$?"; grep -E 'test result|FAILED' /tmp/t6b.log
```
预期：4 条全绿。若 `PortId` 不在 `tool_core`（可能来自 `tool_platform`），按 `grep -rn 'pub struct PortId' crates/` 的结果改 import。

- [ ] **Step 5: 两平台改调 plan_send，并各自暴露 validate_hex**

`crates/application/src/workbench.rs:423-438` 三个分支合并为：

```rust
            command @ (AppCommand::SendText { .. }
            | AppCommand::SendHex { .. }
            | AppCommand::SendRaw { .. }) => {
                let port_name = send_port_name(&command);
                let plan = send_plan::plan_send(&command, self.is_network_port(&port_name))
                    .map_err(AppError::Transport)?;
                self.send_transport_bytes(port_name, plan.bytes)
            }
```
若 `AppCommand` 不支持 or-pattern 绑定（`command @` 要求变体同形），退化为在 `send_transport_bytes` 之前统一算：
```bash
grep -n 'fn send_port_name' crates/application/src/*.rs   # 没有则新建私有辅助
```
新建：
```rust
fn send_port_name(command: &AppCommand) -> String {
    match command {
        AppCommand::SendText { port, .. }
        | AppCommand::SendHex { port, .. }
        | AppCommand::SendRaw { port, .. } => port.to_string(),
        _ => String::new(),
    }
}
```
（`_ => String::new()` 是本任务唯一允许的兜底：调用方以 or-pattern 保证只有三种命令进来。）

`crates/application/src/web.rs:2028-2037` 同样改为调用 `crate::send_plan::plan_send(&command, is_network)`，并**删除** `:2290` 与 `:2334` 两份手抄 `parse_hex`/`parse_hex_strict`。

两平台各加一个入口，供 presentation 复用（消掉「第四份实现」）：
```rust
    /// presentation 只做输入校验，规则与真正发送时完全一致。
    pub fn validate_hex(&self, hex: &str, strict: bool) -> Result<Vec<u8>, AppError> {
        let command = AppCommand::SendHex {
            port: PortId::new("VALIDATE_ONLY"),
            hex: hex.to_owned(),
            strict,
        };
        Ok(send_plan::plan_send(&command, false).map_err(AppError::Transport)?.bytes)
    }
```
（web 侧同形，错误类型为 `String`，用 `.map_err(|e| e.to_string())`。）

- [ ] **Step 6: 跑测试 —— Task 5 的字节契约必须仍绿**

```bash
cargo +1.92.0 test -p tool-application --workspace --all-targets > /tmp/t6c.log 2>&1; echo "EXIT=$?"
grep -E 'test result' /tmp/t6c.log | tail -25
```
预期：`headless` 9 条全绿（含 Task 5 的字节断言）、`send_plan` 4 条全绿、`panels` 181 条不减。**若 `send_bytes_reach_a_connected_network_port` 变红，说明重构改变了实际字节 —— 停下修正 `plan_send`，不得改测试。**

- [ ] **Step 7: presentation 改调 validate_hex，摘掉 tool-transport 的 HEX 直连**

`crates/app/src/ui/bottom_panel.rs:617`、`:707` 的 `tool_transport::parse_hex(...)` 与 `:1104` 的 `hex_preview` 改为经 `self.workbench.validate_hex(&input, strict)`（`strict` 取该处现有语义：现状用宽松 `parse_hex`，则传 `false` 保持行为不变）。`crates/app/src/web.rs:377` 的 `web_parse_hex` 改为 `self.workbench.validate_hex(...)`（wasm 侧 `WebApplication::validate_hex`）。

这一步会**改变 native/web 现有差异**：web 从"更严格"变为与 native 一致。在提交信息中明确写出该行为变化，并检查 `docs/web-v1.md` 是否描述了旧判定。

`crates/app/Cargo.toml`：`tool-transport` 现在还剩 `app/mod.rs:15` 的 `RepaintWaker` 与 `commands.rs:312` 的 `natural_sort_key` 两处，**仍不能移除**。因此本步不删依赖，改为在 `docs/ARCHITECTURE.md:187-198` 的表格里把 HEX 那 3 行划掉，缺口从「5 处」更正为「2 处」，并保留 `RepaintWaker` / `natural_sort_key` 两项作为待收敛项。`app_manifest_keeps_the_application_boundary` 不变（它没锁 `tool-transport`）。

- [ ] **Step 8: 全量门**

```bash
cargo +1.92.0 fmt --all
cargo +1.92.0 clippy --workspace --all-targets -- -D warnings > /tmp/c6.log 2>&1; echo "CLIPPY=$?"
cargo +1.92.0 test --workspace --all-targets > /tmp/t6d.log 2>&1; echo "TEST=$?"
grep -E 'test result:.*[1-9] failed|FAILED' /tmp/t6d.log | head
```
预期：两个 0。总数 = 506 + Task1(3) + Task2(3+) + Task3(3) + Task5(1) + Task6(send_plan 4)，减去从 transport 迁到 core 的等量用例。把最终数字与算式写进提交信息。

- [ ] **Step 9: wasm 编译门（本任务动了 web.rs，必跑）**

```bash
cargo +1.92.0 clippy -p hardware-workbench-app --all-targets --target wasm32-unknown-unknown -- -D warnings > /tmp/c6w.log 2>&1; echo "WASM=$?"
```
若本机未装该 target，先 `rustup target list --installed` 确认；缺失则在报告里写明"未验证"而不是跳过不提。

- [ ] **Step 10: 提交**

```bash
git add crates/core/src/lib.rs crates/transport/src/lib.rs crates/application/src/lib.rs \
        crates/application/src/send_plan.rs crates/application/src/workbench.rs \
        crates/application/src/web.rs crates/app/src/ui/bottom_panel.rs crates/app/src/web.rs \
        docs/ARCHITECTURE.md docs/web-v1.md
git commit -m "refactor: plan a send once for native and web"
```

---

## 收尾：文档与基线对账

- [ ] 更新 `docs/ARCHITECTURE.md:121` 的测试基线数字（当前写的 506/7 会过期），连同各任务留下的算式。
- [ ] `CHANGELOG.md` 的 `[Unreleased]` 增「安全」小节：更新包外置哈希、沙箱 `dofile/loadfile/load` 移除、发送契约与僵尸端口修复。措辞面向用户，不写内部编号。
- [ ] 最终对账：三条门 + wasm clippy 全跑一遍，逐条贴退出码。
- [ ] `git status --porcelain` 为空；`git log --oneline` 应有 7 个新提交（6 个任务 + 收尾）。

## Self-Review 结论

- **覆盖**：评审 6 项 → Task 1（更新哈希）/ 2（沙箱）/ 3（僵尸端口，含评审第 4 项）/ 4（守卫）/ 5（发送契约，评审第 3 项）/ 6（统一路由）。评审的 Important #6（updater SHA256 无测试）、#7（application 侧守卫仍弱文本匹配）分别被 Task 1 Step 5 与 Task 4 吸收。
- **未纳入本计划**（评审提到但超出这 6 项，另立议题）：release profile 缺 `overflow-checks`；`ws://` 明文 + `api_key` 未加密落盘；registry 声明权限与实包 `plugin.json` 不交叉校验；zip 条目被拒时只 warn 后跳过；regex 缓存 `Box::leak`；`tool-testing` 反向着陆在 `tool-application` 闭包内；`ARCHITECTURE.md:74` 仍以不存在的 `todo.txt §7` 为分层判定依据。
- **类型一致性**：`SendPlanError` 只实现 `Display`，两平台各自 `.map_err`；`PlannedSend.task_kind` 与现有 `spawn_ordered(.., "send_serial"/"send_network", ..)` 字面量逐字一致；`validate_hex` 在 native 返 `Result<Vec<u8>, AppError>`、web 返 `Result<Vec<u8>, String>`，这是 `AppError` 本身 cfg-gated（`lib.rs:20-46`）的必然结果，已在 Task 6 Step 5 注明。
- **无占位符**：每个改动步骤都给了可编译代码或精确 grep 命令；仅有的两处"以现状为准"（Task 3 Step 6 recorder 构造签名、Task 5 Step 2 的 JSON-RPC 路径）都附带了确定它的命令与不得放宽测试的约束。
