# 发布说明模板

> 用法：发布新版本时复制本文件为 `docs/releases/<tag>.md`（文件名与 git tag 完全一致，如 `v1.4.0.md`）。
> CI 的 `test` 作业在 tag 上要求该文件存在，并把它的正文作为 GitHub Release 内容；
> `update.json` 的 `changelog` 字段则从根 `CHANGELOG.md` 的对应版本小节抓取。发布前两处都要写。

## [vX.Y.Z] - YYYY-MM-DD

### 亮点

- 新功能与值得注意的改进（面向用户表述；内部重构不必逐条列出）。

### 修复

- 用户可见的问题修复。

### 已知问题

- 对用户有影响的已知限制；开源层面的内部缺口记录在 `docs/known-gaps.md`，此处只列需要用户知道的。没有则写「无」。

### 安装与校验

- Windows：`hardware-workbench-app.zip`（便携版）或 `HardwareWorkbenchSetup.exe`（安装器）。
- Linux：`hardware-workbench_<version>_amd64.deb`（Ubuntu 22.04+）。
- 校验：附件 `SHA256SUMS.txt` 与 `update.json` 中的 `sha256` 一致。
