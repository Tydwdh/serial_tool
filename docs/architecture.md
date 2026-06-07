# 多工作区架构设计

## 目标

每个 COM 端口 = 一个完全独立的工作区，效果等同于打开了多个软件实例。

## 当前架构 (v0.1)

```
WorkbenchApp (单例)
├── DataBus (1个)
├── TransportManager (1个，支持多端口)
├── PluginManager (1个)
├── TerminalPanel (1个，port_ui按端口过滤)
├── DynamicPanels (1个)
└── 所有面板共享同一个 DataBus
```

**问题：** 端口之间共享 DataBus → 插件/面板数据无法隔离。

## 目标架构 (v0.2)

```
WorkbenchApp
├── 全局层 (共享)
│   ├── ports list (串口枚举，全局)
│   ├── recorder (录制所有workspace事件)
│   ├── theme, fonts
│   └── activity_order (活动栏顺序)
│
├── Workspace 实例 (每个 COM 端口一个)
│   ├── DataBus (隔离)
│   ├── TransportManager (隔离)
│   ├── PluginManager (隔离)
│   ├── TerminalPanel
│   ├── DynamicPanels
│   ├── PanelManager
│   ├── PortSendState
│   ├── 速率统计
│   └── status_message
│
└── 主 Workspace (无端口时的默认界面)
    ├── DataBus (设备面板、回放、设置等)
    ├── PluginManager (全局插件)
    └── ReplayPanel, PluginsPanel
```

## UI 布局

```
┌──────────────────────────────────────────────┐
│ 顶栏: 录制按钮 + 串口快捷控制                   │ ← 全局
├──────────────────────────────────────────────┤
│ [COM2] [COM3] [主界面]                        │ ← workspace tabs
├─────┬────────────────────────────────────────┤
│     │                                        │
│ 📟  │  当前 workspace 的完整内容:              │
│ ⏪  │  - 发送区                               │
│ 🧩  │  - 终端/接收区                          │
│ ⚙   │  - 图表面板                             │
│     │  - 插件面板                             │
│     │  - 日志面板                             │
│     │                                        │
│     │  (所有内容来自 workspace 的 DataBus)      │
│     │                                        │
├─────┴────────────────────────────────────────┤
│ 状态栏: 事件速率 | 状态消息                     │ ← 全局
└──────────────────────────────────────────────┘
```

## Workspace 数据结构

```rust
struct Workspace {
    id: String,              // "COM2" 或 "main"
    bus: DataBus,
    transport: TransportManager,
    plugin_manager: PluginManager,
    terminal: TerminalPanel,
    chart: ChartPanel,
    form: FormPanel,
    dynamic_panels: DynamicPanels,
    panels: PanelManager,
    send: PortSendState,
    inspector_visible: bool,
    status_message: String,
    event_rate: f64,
    // 串口配置
    port_name: String,
    baud_rate: String,
    data_bits: String, stop_bits: String, parity: String, timeout_ms: String,
    // 插件管理
    enabled_plugins: Vec<String>,
}
```

## 切换 Workspace 时发生什么

1. `active_workspace` 字段改变
2. UI 重新渲染，所有面板读取新 workspace 的数据
3. 旧的 workspace 保持运行（DataBus 继续接收事件）
4. 定时器和插件回调在后台继续执行

## 全局状态 vs Workspace 状态

| 组件 | 全局 | Workspace |
|------|:----:|:---------:|
| 串口列表 (ports) | ✓ | |
| 录制 (recorder) | ✓ | |
| 主题/字体 | ✓ | |
| 活动栏顺序 | ✓ | |
| DataBus | | ✓ |
| TransportManager | | ✓ |
| PluginManager | | ✓ |
| TerminalPanel | | ✓ |
| ChartPanel | | ✓ |
| DynamicPanels | | ✓ |
| 发送状态 | | ✓ |

## 实施步骤

### Phase 1: 抽离 Workspace 结构体
- 创建 `Workspace` struct
- 将现有字段迁移进去
- `WorkbenchApp` 持有 `workspaces: BTreeMap<String, Workspace>`
- `active_workspace: String`
- 所有 `self.xxx` 改为 `self.ws().xxx`

### Phase 2: 创建默认主 Workspace
- 启动时自动创建 `"main"` workspace
- 无端口连接时使用主 workspace

### Phase 3: 端口打开时创建新 Workspace
- `open_selected_port()` 时创建 `Workspace::new(port_name)`
- 切换 `active_workspace` 到新端口
- 自动启用该端口的插件

### Phase 4: 端口关闭时销毁 Workspace
- `close_port()` 或断开连接时
- 停止插件、销毁 DataBus
- 清理动态面板

### Phase 5: 录制跨 Workspace
- 录制器订阅所有 workspace 的 DataBus
- 或者在 TransportManager 层录制所有串口数据

## 风险

1. **内存：** 每个 workspace 持有自己的 DataBus + PluginManager，内存消耗随端口数线性增长
2. **插件隔离：** 插件在 workspace A 创建的面板在 workspace B 不可见，需要用户理解
3. **串口 list：** 所有 workspace 共享串口枚举，打开/关闭操作需要协调
4. **录制：** 录制器需要跨 workspace 工作，可能需要特殊处理
