# 移除 Web 版并收口到单一原生组合根

## Problem Statement

维护者同时维护 native 与 Web 两个组合根。Web 版要求共享层保留 wasm 分支、第二套 Lua 引擎（omniLua）与差分测试、独立 CI 作业和 GitHub Pages 发布。每一次共享代码变更都要为一条不再需要支持的分支付费，而浏览器免安装的收益不足以抵消维护成本。维护者还希望趁这次清理删掉被 web 撑大的死代码、失效 lint 豁免和文档里的双平台叙述，同时保留自己实际使用的插件市场与模板插件。

## Solution

发布 2.0.0：删除 Web 版（wasm32 目标、浏览器组合根、web 平台适配、omniLua 引擎与差分测试、Web CI、Pages 部署、Trunk 工程与构建产物、web 文档页），把为双组合根存在的单实现抽象收口到 native `Workbench`，清理 web 独占的死代码与 lint 豁免。原生桌面功能、原生网络串口、插件市场与模板插件保持不变。`v1.3.2` 作为最后一个包含 Web 版的版本保留；GitHub Pages 停用；2.0.0 发布说明写明在线版终止。

## User Stories

1. As a maintainer, I want the wasm32 target and the browser composition root removed, so shared changes no longer carry wasm branches.
2. As a maintainer, I want the browser-only platform adapters removed, so `tool-platform` has one implementation per capability.
3. As a maintainer, I want the second Lua engine (omniLua) and its parity tests deleted, so only `lua_host` is maintained.
4. As a maintainer, I want the web-only plugin host/engine surface removed, so `tool-plugin-api` contains only the native plugin contract.
5. As a maintainer, I want `AppRuntime` and the capability matrix collapsed into `Workbench`, so the UI binds one composition root.
6. As a maintainer, I want `CommandEffects` kept, so command decisions stay separated from effects.
7. As a maintainer, I want web-only dead code and lint exemptions removed, so `-D warnings` reflects the real codebase.
8. As a maintainer, I want a source guard that forbids `wasm32` cfg, so the deletion is not silently reintroduced.
9. As a maintainer, I want CI to stop building/linting wasm and deploying Pages, so CI time and failure surface shrink.
10. As a maintainer, I want the Trunk project, its generated output, and the wasm cargo config removed, so no stale build path remains.
11. As a maintainer, I want docs to describe only the native app, so readers are not sent to a dead online edition.
12. As a maintainer, I want historical release notes and CHANGELOG entries kept, so v1.3.2 users can still understand the last web release.
13. As a web user, I want the 2.0.0 release note to state that the online edition ended at v1.3.2, so I know where to go.
14. As a desktop user, I want native behavior unchanged, so removing web does not regress serial, network serial, recording/replay, or plugins.
15. As a plugin author, I want the native Lua API and plugin manifest unchanged, so existing plugins keep working.
16. As a marketplace user, I want registry URLs and installed plugins unaffected, so installs do not break.
17. As a maintainer, I want a 2.0.0 version bump and release note, so the platform removal is communicated.
18. As a maintainer, I want a final smoke run and Pages disable, so the release is verified and the old URL stops serving a stale build.
19. As a maintainer, I want the `v1.3.2` tag to remain the recovery point, so no history rewrite is needed.
20. As a maintainer, I want each ticket to leave main green on Windows and Linux, so direct-to-main work stays safe.

## Implementation Decisions

- Target version 2.0.0.
- Remove the browser composition root, the app web shell, the platform web adapters, and the core wasm clock branches. Native network serial is not part of the Web 版 and stays.
- Delete the `tool-plugin-runtime` crate, the `omnilua` workspace dependency, and the web/native parity tests.
- Collapse `AppRuntime` into inherent `Workbench` methods; delete `Capability`/`AppCapabilities` and panel capability gating. Keep the DTOs (`TransportView`, `PortView`, etc.) because `tool-panels` must not depend on domain crates.
- Keep `CommandEffects`: it is the platform-neutral command/effect boundary, not a web artifact.
- Delete the web-only engine/host surface from `tool-plugin-api`; keep the value/permission/ctx/error types used by `lua_host` and `extension`.
- Remove the wasm CI job and the Pages workflow; the release job depends on the Windows test job and the Ubuntu `.deb` job only. Keep the existing Linux app clippy exception (updater cfg gap, unrelated to web).
- Remove the wasm rustflags config, the Trunk project, its generated output, and the web documentation page.
- Sync README, ARCHITECTURE, known-gaps, RELEASE, and the docs index to the single-root reality. Keep historical release notes and CHANGELOG entries. Add a 2.0.0 release note stating the online edition ended at `v1.3.2`.
- Keep `plugin-marketplace/` and `plugins/` templates: their registry URLs are live download paths and the maintainer uses them.
- Add a `no_wasm_target` source guard to the architecture tests, scanning workspace sources with comments stripped.
- Disable GitHub Pages after final verification; tag `v2.0.0` only after the full suite and native smoke pass.

## Testing Decisions

- Reuse existing seams: workspace tests, the architecture dependency guards, the headless application contract tests, and a native smoke run.
- Remove the web source-shape guards; add the `no_wasm_target` guard in the same architecture test file.
- Gates: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` on Windows; `cargo test --workspace --all-targets`; Linux `cargo check`, tests, and clippy.
- Run `cargo machete`; fix real unused dependencies, document false positives.
- Residue check: `git grep` for `wasm32`, `wasm-bindgen`, `web-sys`, `js-sys`, `omnilua`, `plugin_runtime`, `WebApplication`, `WebRuntime`, `web_serial`, `web_network`, `web_fetch`; historical allowlist is `CHANGELOG.md`, `docs/releases/v1.3.*`, `docs/adr/`, and `GLOSSARY.md`.
- Native smoke: launch the app, verify devices/terminal/sender panels render and the serial list refreshes (no hardware required); capture a screenshot.
- No new behaviour tests: the change is deletion, verified by compilation, existing contract tests, and the smoke run.

## Out of Scope

- Fixing the Linux updater clippy gap (keep the documented exception).
- Deleting marketplace entries, versioned zips, or template plugins.
- Rewriting git history.
- New features or refactors unrelated to removing the Web 版.
- macOS support.
- Changing native network serial behaviour.

## Further Notes

- `v1.3.2` is the last web-capable release; the tag stays and is the recovery point.
- Disabling Pages makes the old online URL return 404; the 2.0.0 release note is the user-facing notice.
- Tickets live under `.scratch/remove-web/issues/`; work is direct on `main`, one ticket at a time, each leaving main green.
- CI cleanup lands first so the wasm job does not go red while the code is removed.
