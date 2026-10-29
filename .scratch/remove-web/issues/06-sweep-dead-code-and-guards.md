# 06: 清扫死代码与守卫

**What to build:** web 独占的死代码、lint 豁免和源码形状守卫清除；新增禁止 wasm 目标的守卫；未用依赖有结论。

**Blocked by:** 03, 04, 05.

**Status:** ready-for-agent

- [ ] `shared_keymap` 中仅 web 使用的项删除；`lint_exemptions` 的期望清单从 5 条降到 3 条。
- [ ] 架构测试中 web 形状守卫删除；新增 `no_wasm_target` 源码守卫（剥注释后扫描工作区源码）。
- [ ] `cargo machete` 运行，真实未用依赖删除，误报记录在 ticket 评论。
- [ ] `git grep` 残留检查通过（历史白名单：CHANGELOG、v1.3.* 发布说明、ADR、GLOSSARY）。
