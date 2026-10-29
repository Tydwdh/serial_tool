# 00: 退役 Web 交付

**What to build:** CI 不再构建或发布 Web 版；Trunk 工程、生成产物、wasm cargo 配置和公开的在线入口消失，native 流水线保持绿色。

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] `.github/workflows/web.yml` 删除，`ci.yml` 的 `wasm` 作业删除，`release` 只依赖 `test` 与 `linux-deb`。
- [ ] wasm 专用 cargo 配置删除；Trunk 工程目录与 `dist-web/` 生成产物不存在。
- [ ] README 不再包含在线模式链接、说明或 web 构建步骤。
- [ ] `docs/web.md` 删除；`docs/README.md` 索引不再链接它；`docs/RELEASE.md` 不再列出 wasm 作业或 web 文档。
- [ ] `cargo fmt --all --check` 与 `cargo check -p hardware-workbench-app` 通过。
