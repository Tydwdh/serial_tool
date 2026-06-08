# 如何放置这一组文件

把本压缩包解压到项目根目录即可。解压后会新增或覆盖这些路径：

```text
docs/
  README.md
  PLUGIN_DEVELOPMENT.md
  lua-plugin-api.md
  plugin-manifest.md
  replay-analyzer.md
  recording-replay.md
  release-checklist.md
  README_放置说明.md

plugins/
  template.hello/
  template.serial-chart/
```

如果你已有同名文档，建议先备份再覆盖。

模板插件不会覆盖已有 `demo.signal-generator`。正式发布时可以保留模板，也可以放到 `plugins/` 目录下供用户复制。
