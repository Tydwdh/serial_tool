import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../theme/app_theme.dart';
import '../providers/backend_provider.dart';
import '../providers/dock_layout_provider.dart';
import '../backend/models.dart';
import 'dock_layout.dart';
import 'dock_tab_bar.dart';
import 'dynamic_panels.dart';
import 'terminal_panel.dart';
import 'sender_panel.dart';
import 'log_panel.dart';
import 'plugins_panel.dart';
import 'replay_panel.dart';
import 'settings_panel.dart';
import 'command_palette.dart';

class WorkbenchShell extends ConsumerStatefulWidget {
  const WorkbenchShell({super.key});
  @override
  ConsumerState<WorkbenchShell> createState() => _WorkbenchShellState();
}

class _WorkbenchShellState extends ConsumerState<WorkbenchShell> {
  bool _draggingBottom = false;
  bool _commandPaletteOpen = false;
  String? _selectedPort;
  String _baudRate = '115200';
  bool _dtr = false;
  bool _rts = false;

  void _runSerialAction(VoidCallback action) {
    try {
      action();
    } catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(error.toString())));
    }
  }

  @override
  void initState() {
    super.initState();
    ref.listen(backendEventStreamProvider, (_, next) {
      next.whenData((event) {
        if (event.type != 'notification' && event.type != 'error') return;
        final message = event.data['message']?.toString();
        if (message == null || message.isEmpty || !mounted) return;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!mounted) return;
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text(message),
              backgroundColor: event.data['level'] == 'warning'
                  ? AppTheme.warning
                  : null,
            ),
          );
        });
      });
    });
    Future.delayed(const Duration(milliseconds: 100), () {
      if (mounted) ref.read(backendServiceProvider).refreshPorts();
    });
    _restoreSerialSelection();
  }

  Future<void> _restoreSerialSelection() async {
    try {
      await ref.read(backendInitializedProvider.future);
      final config = ref.read(backendServiceProvider).getConfig();
      final serial = Map<String, dynamic>.from(config['serial'] as Map? ?? {});
      if (!mounted) return;
      setState(() {
        _selectedPort = serial['selected_port'] as String?;
        _baudRate = serial['baud_rate'] as String? ?? _baudRate;
      });
    } catch (_) {
      // Default controls remain usable when no previous config exists.
    }
  }

  @override
  Widget build(BuildContext context) {
    final initialized = ref.watch(backendInitializedProvider);
    return initialized.when(
      loading: () => const Scaffold(
        body: Center(
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              SizedBox(
                width: 40,
                height: 40,
                child: CircularProgressIndicator(strokeWidth: 3),
              ),
              SizedBox(height: 16),
              Text(
                '初始化后端...',
                style: TextStyle(color: AppTheme.textSecondary, fontSize: 13),
              ),
            ],
          ),
        ),
      ),
      error: (err, _) => Scaffold(
        body: Center(
          child: Text(
            '初始化失败: $err',
            style: const TextStyle(color: AppTheme.error),
          ),
        ),
      ),
      data: (_) => _buildLayout(),
    );
  }

  Widget _buildLayout() {
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyK, control: true): () {
          setState(() => _commandPaletteOpen = !_commandPaletteOpen);
        },
      },
      child: Focus(
        autofocus: true,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final layout = ref.watch(dockLayoutProvider);
            // Keep the center workspace usable even after a layout saved on a
            // larger display is restored on a smaller one.
            final availableHeight = constraints.maxHeight - 64;
            final maxBottomHeight = (availableHeight - 220).clamp(0.0, 600.0);
            final showBottom = layout.bottomVisible && maxBottomHeight >= 150;
            final bottomHeight = showBottom
                ? layout.bottomSize.clamp(150.0, maxBottomHeight).toDouble()
                : 0.0;
            return Stack(
              children: [
                Column(
                  children: [
                    _buildTopBar(),
                    Expanded(child: _buildMainArea()),
                    if (showBottom) _buildBottomDock(bottomHeight),
                    _buildStatusBar(),
                  ],
                ),
                if (_commandPaletteOpen)
                  Positioned.fill(
                    child: CommandPalette(
                      backend: ref.read(backendServiceProvider),
                      onClose: () =>
                          setState(() => _commandPaletteOpen = false),
                      onOpenPort: _selectedPort == null
                          ? null
                          : () => _runSerialAction(
                              () => ref
                                  .read(backendServiceProvider)
                                  .openPort(_selectedPort!),
                            ),
                      onClosePort: _selectedPort == null
                          ? null
                          : () => _runSerialAction(
                              () => ref
                                  .read(backendServiceProvider)
                                  .closePort(_selectedPort!),
                            ),
                      onToggleBottom: () =>
                          ref.read(dockLayoutProvider.notifier).toggleBottom(),
                      onToggleRight: () =>
                          ref.read(dockLayoutProvider.notifier).toggleRight(),
                      onResetLayout: () =>
                          ref.read(dockLayoutProvider.notifier).resetLayout(),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  // ═══════════════════════ 顶部栏 ═══════════════════════

  Widget _buildTopBar() {
    final ports = ref.watch(portListProvider).valueOrNull ?? [];
    final layout = ref.watch(dockLayoutProvider);
    final status = ref.watch(backendStatusProvider).valueOrNull;
    final selectedOpen =
        _selectedPort != null &&
        (status?.openPorts.contains(_selectedPort) ?? false);
    return Container(
      height: 40,
      padding: const EdgeInsets.symmetric(horizontal: 6),
      decoration: const BoxDecoration(
        color: AppTheme.bgPanel,
        border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
      ),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final compact = constraints.maxWidth < 980;
          final narrow = constraints.maxWidth < 620;
          return Row(
            children: [
              Padding(
                padding: EdgeInsets.only(left: 6, right: 14),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    DecoratedBox(
                      decoration: BoxDecoration(
                        color: AppTheme.accent,
                        borderRadius: BorderRadius.all(Radius.circular(8)),
                      ),
                      child: SizedBox(
                        width: 26,
                        height: 26,
                        child: Icon(
                          Icons.memory_rounded,
                          size: 16,
                          color: AppTheme.bgDark,
                        ),
                      ),
                    ),
                    if (!compact) ...[
                      const SizedBox(width: 8),
                      const Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            'Hardware Workbench',
                            style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w700,
                              color: AppTheme.textBright,
                            ),
                          ),
                          Text(
                            '硬件调试工作台',
                            style: TextStyle(
                              fontSize: 9,
                              color: AppTheme.textSecondary,
                            ),
                          ),
                        ],
                      ),
                    ],
                  ],
                ),
              ),
              if (!compact)
                Container(width: 1, height: 22, color: AppTheme.borderColor),
              if (!compact) const SizedBox(width: 8),
              if (!narrow)
                _iconBtn(Icons.menu, layout.activityBarVisible, () {
                  ref.read(dockLayoutProvider.notifier).toggleActivityBar();
                }, '活动栏'),
              const SizedBox(width: 4),
              _buildPortCombo(ports, width: narrow ? 104 : 130),
              const SizedBox(width: 4),
              _buildBaudCombo(),
              const SizedBox(width: 4),
              _iconBtn(
                selectedOpen ? Icons.power_off : Icons.power_settings_new,
                selectedOpen,
                _selectedPort == null
                    ? null
                    : () => _runSerialAction(() {
                        final backend = ref.read(backendServiceProvider);
                        if (selectedOpen) {
                          backend.closePort(_selectedPort!);
                        } else {
                          backend.openPort(_selectedPort!);
                        }
                      }),
                selectedOpen ? '关闭串口' : '打开串口',
                color: _selectedPort == null
                    ? null
                    : selectedOpen
                    ? AppTheme.error
                    : AppTheme.success,
              ),
              _iconBtn(
                Icons.refresh,
                false,
                () => _runSerialAction(
                  () => ref.read(backendServiceProvider).refreshPorts(),
                ),
                '刷新',
              ),
              const Spacer(),
              if (!compact)
                _iconBtn(
                  Icons.fiber_manual_record,
                  false,
                  () => ref.read(backendServiceProvider).toggleRecording(),
                  '录制',
                ),
              if (!compact) const SizedBox(width: 4),
              if (!compact)
                _iconBtn(
                  layout.bottomVisible
                      ? Icons.keyboard_arrow_down
                      : Icons.keyboard_arrow_up,
                  layout.bottomVisible,
                  () => ref.read(dockLayoutProvider.notifier).toggleBottom(),
                  '底部面板',
                ),
              if (!compact)
                _iconBtn(
                  Icons.chevron_left,
                  layout.rightVisible,
                  () => ref.read(dockLayoutProvider.notifier).toggleRight(),
                  '右侧面板',
                ),
              if (!compact)
                _iconBtn(
                  Icons.search,
                  false,
                  () => setState(() => _commandPaletteOpen = true),
                  '命令面板 (Ctrl+K)',
                ),
              if (compact) _buildTopOverflowMenu(layout),
            ],
          );
        },
      ),
    );
  }

  Widget _buildTopOverflowMenu(DockLayout layout) {
    return PopupMenuButton<String>(
      tooltip: '更多操作',
      icon: const Icon(Icons.more_horiz, size: 18),
      onSelected: (action) {
        switch (action) {
          case 'activity':
            ref.read(dockLayoutProvider.notifier).toggleActivityBar();
          case 'record':
            ref.read(backendServiceProvider).toggleRecording();
          case 'bottom':
            ref.read(dockLayoutProvider.notifier).toggleBottom();
          case 'right':
            ref.read(dockLayoutProvider.notifier).toggleRight();
          case 'palette':
            setState(() => _commandPaletteOpen = true);
        }
      },
      itemBuilder: (_) => [
        PopupMenuItem(
          value: 'activity',
          child: Text(layout.activityBarVisible ? '隐藏活动栏' : '显示活动栏'),
        ),
        const PopupMenuItem(value: 'record', child: Text('切换录制')),
        PopupMenuItem(
          value: 'bottom',
          child: Text(layout.bottomVisible ? '隐藏底部面板' : '显示底部面板'),
        ),
        PopupMenuItem(
          value: 'right',
          child: Text(layout.rightVisible ? '隐藏右侧面板' : '显示右侧面板'),
        ),
        const PopupMenuItem(value: 'palette', child: Text('命令面板')),
      ],
    );
  }

  Widget _buildPortCombo(List<PortDescriptor> ports, {double width = 130}) {
    final selected = ports.any((port) => port.portName == _selectedPort)
        ? _selectedPort
        : null;
    return SizedBox(
      width: width,
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: selected,
          isExpanded: true,
          hint: const Text(
            '选择串口',
            style: TextStyle(fontSize: 12, color: AppTheme.textSecondary),
          ),
          dropdownColor: AppTheme.bgInput,
          style: const TextStyle(fontSize: 12, color: AppTheme.textPrimary),
          icon: const Icon(
            Icons.arrow_drop_down,
            size: 16,
            color: AppTheme.textSecondary,
          ),
          items: ports.isEmpty
              ? [
                  const DropdownMenuItem(
                    value: null,
                    child: Text(
                      '无可用串口',
                      style: TextStyle(
                        fontSize: 12,
                        color: AppTheme.textSecondary,
                      ),
                    ),
                  ),
                ]
              : ports
                    .map(
                      (p) => DropdownMenuItem(
                        value: p.portName,
                        child: Row(
                          children: [
                            Container(
                              width: 8,
                              height: 8,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: AppTheme.textSecondary,
                              ),
                            ),
                            const SizedBox(width: 6),
                            Text(
                              p.portName,
                              style: const TextStyle(fontSize: 12),
                            ),
                          ],
                        ),
                      ),
                    )
                    .toList(),
          onChanged: (v) {
            setState(() => _selectedPort = v);
            if (v != null) ref.read(backendServiceProvider).setSelectedPort(v);
          },
        ),
      ),
    );
  }

  Widget _buildBaudCombo() {
    final rates = [
      '9600',
      '19200',
      '38400',
      '57600',
      '115200',
      '230400',
      '460800',
      '921600',
      '1000000',
      '2000000',
      '3000000',
    ];
    return SizedBox(
      width: 80,
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: _baudRate,
          isExpanded: true,
          dropdownColor: AppTheme.bgInput,
          style: const TextStyle(fontSize: 12, color: AppTheme.textPrimary),
          icon: const Icon(
            Icons.arrow_drop_down,
            size: 16,
            color: AppTheme.textSecondary,
          ),
          items: rates
              .map(
                (r) => DropdownMenuItem(
                  value: r,
                  child: Text(r, style: const TextStyle(fontSize: 12)),
                ),
              )
              .toList(),
          onChanged: (v) {
            if (v != null) {
              setState(() => _baudRate = v);
              ref.read(backendServiceProvider).setBaudRate(v);
            }
          },
        ),
      ),
    );
  }

  Widget _iconBtn(
    IconData icon,
    bool active,
    VoidCallback? onPressed,
    String tooltip, {
    Color? color,
  }) {
    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 1),
      child: IconButton(
        icon: Icon(icon, size: 16),
        color: color ?? (active ? AppTheme.textBright : AppTheme.textSecondary),
        onPressed: onPressed,
        tooltip: tooltip,
        style: IconButton.styleFrom(
          padding: EdgeInsets.zero,
          minimumSize: const Size(28, 28),
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          backgroundColor: active ? AppTheme.bgActive : null,
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.all(Radius.circular(4)),
          ),
        ),
      ),
    );
  }

  // ═══════════════════════ 主区域 ═══════════════════════

  Widget _buildMainArea() {
    final layout = ref.watch(dockLayoutProvider);
    return LayoutBuilder(
      builder: (context, constraints) {
        const activityBarWidth = 48.0;
        const minCenterWidth = 380.0;
        final activityWidth = layout.activityBarVisible ? activityBarWidth : 0;
        final maxRightWidth =
            constraints.maxWidth - activityWidth - minCenterWidth - 4;
        final showRight = layout.rightVisible && maxRightWidth >= 200;
        final rightWidth = showRight
            ? layout.rightSize.clamp(200.0, maxRightWidth).toDouble()
            : 0.0;
        return Row(
          children: [
            if (layout.activityBarVisible) _buildActivityBar(),
            Expanded(child: _buildCenterPanel()),
            if (showRight) _buildRightDock(rightWidth),
          ],
        );
      },
    );
  }

  Widget _buildActivityBar() {
    final layout = ref.watch(dockLayoutProvider);
    const allPanels = [
      PanelKind.terminal,
      PanelKind.sender,
      PanelKind.logs,
      PanelKind.chart,
      PanelKind.plugins,
      PanelKind.replay,
      PanelKind.devices,
      PanelKind.dynamic,
    ];

    return Container(
      width: 48,
      color: AppTheme.bgDark,
      child: Column(
        children: [
          const SizedBox(height: 8),
          ...List.generate(allPanels.length, (i) {
            final kind = allPanels[i];
            final inCenter = layout.center.tabs.contains(kind);
            final isActive = layout.center.active == kind;
            return Container(
              margin: const EdgeInsets.symmetric(vertical: 1, horizontal: 4),
              child: DragTarget<PanelKind>(
                onAcceptWithDetails: (details) {
                  // 如果拖来的面板不在中心，移到中心
                  if (!layout.center.tabs.contains(details.data)) {
                    ref
                        .read(dockLayoutProvider.notifier)
                        .movePanelTo(DockArea.center, details.data);
                  }
                },
                builder: (context, candidateData, rejectedData) {
                  return Draggable<PanelKind>(
                    data: kind,
                    feedback: Material(
                      color: Colors.transparent,
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 8,
                          vertical: 4,
                        ),
                        decoration: BoxDecoration(
                          color: AppTheme.bgActive,
                          border: Border.all(color: AppTheme.accent),
                          borderRadius: BorderRadius.circular(4),
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(
                              kind.icon,
                              size: 14,
                              color: AppTheme.textBright,
                            ),
                            const SizedBox(width: 4),
                            Text(
                              kind.title,
                              style: const TextStyle(
                                fontSize: 11,
                                color: AppTheme.textBright,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                    childWhenDragging: Opacity(
                      opacity: 0.3,
                      child: _activityIcon(kind, false, false, () {}),
                    ),
                    child: _activityIcon(kind, isActive, inCenter, () {
                      if (inCenter) {
                        ref
                            .read(dockLayoutProvider.notifier)
                            .setCenterTab(kind);
                      } else {
                        ref
                            .read(dockLayoutProvider.notifier)
                            .movePanelTo(DockArea.center, kind);
                      }
                    }),
                  );
                },
              ),
            );
          }),
          const Spacer(),
          _activityIcon(PanelKind.settings, false, false, () {
            ref
                .read(dockLayoutProvider.notifier)
                .movePanelTo(DockArea.center, PanelKind.settings);
          }),
          const SizedBox(height: 8),
        ],
      ),
    );
  }

  Widget _activityIcon(
    PanelKind kind,
    bool isActive,
    bool inCenter,
    VoidCallback onPressed,
  ) {
    return IconButton(
      icon: Icon(kind.icon, size: 20),
      color: isActive
          ? AppTheme.textBright
          : inCenter
          ? AppTheme.textPrimary
          : AppTheme.textSecondary,
      onPressed: onPressed,
      tooltip: kind.title,
      style: IconButton.styleFrom(
        backgroundColor: isActive ? AppTheme.bgActive : null,
        padding: EdgeInsets.zero,
        minimumSize: const Size(36, 36),
        shape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(6)),
        ),
      ),
    );
  }

  Widget _buildCenterPanel() {
    final layout = ref.watch(dockLayoutProvider);
    final active = layout.center.active;
    if (active == null) return _buildEmptyDock();
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          DockTabBar(
            area: DockArea.center,
            stack: layout.center,
            onTap: (index) => ref
                .read(dockLayoutProvider.notifier)
                .setCenterTab(layout.center.tabs[index]),
            onClose: () {},
            onReorder: (oldIndex, newIndex) => ref
                .read(dockLayoutProvider.notifier)
                .reorderTab(DockArea.center, oldIndex, newIndex),
          ),
          Expanded(child: _buildPanelContent(active)),
        ],
      ),
    );
  }

  Widget _buildEmptyDock() {
    return DragTarget<PanelKind>(
      onAcceptWithDetails: (details) {
        ref
            .read(dockLayoutProvider.notifier)
            .movePanelTo(DockArea.center, details.data);
      },
      builder: (context, candidateData, rejectedData) {
        return Container(
          color: AppTheme.bgDark,
          child: Center(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(
                  Icons.arrow_back,
                  size: 32,
                  color: candidateData.isNotEmpty
                      ? AppTheme.accent
                      : AppTheme.textSecondary.withValues(alpha: 0.3),
                ),
                const SizedBox(height: 8),
                Text(
                  candidateData.isNotEmpty ? '拖放到此处' : '从左侧拖入面板',
                  style: TextStyle(
                    fontSize: 13,
                    color: candidateData.isNotEmpty
                        ? AppTheme.accent
                        : AppTheme.textSecondary,
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  // ═══════════════════════ 右侧面板 ═══════════════════════

  Widget _buildRightDock(double width) {
    final layout = ref.watch(dockLayoutProvider);
    return Row(
      children: [
        GestureDetector(
          onHorizontalDragUpdate: (e) {
            ref
                .read(dockLayoutProvider.notifier)
                .setRightSize(width - e.delta.dx);
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.resizeColumn,
            child: Container(
              width: 4,
              color: AppTheme.borderColor,
              child: Container(
                width: 1,
                margin: const EdgeInsets.symmetric(vertical: 4),
                color: AppTheme.borderColor,
              ),
            ),
          ),
        ),
        SizedBox(
          width: width,
          child: Container(
            color: AppTheme.bgPanel,
            child: Column(
              children: [
                DockTabBar(
                  area: DockArea.right,
                  stack: layout.right,
                  onTap: (i) =>
                      ref.read(dockLayoutProvider.notifier).setRightTab(i),
                  onClose: () =>
                      ref.read(dockLayoutProvider.notifier).toggleRight(),
                  onReorder: (oldI, newI) => ref
                      .read(dockLayoutProvider.notifier)
                      .reorderTab(DockArea.right, oldI, newI),
                ),
                Expanded(child: _buildPanelContent(layout.right.active)),
              ],
            ),
          ),
        ),
      ],
    );
  }

  // ═══════════════════════ 底部面板 ═══════════════════════

  Widget _buildBottomDock(double height) {
    final layout = ref.watch(dockLayoutProvider);
    return Listener(
      onPointerMove: (e) {
        if (_draggingBottom) {
          ref
              .read(dockLayoutProvider.notifier)
              .setBottomSize(height - e.delta.dy);
        }
      },
      onPointerUp: (_) => _draggingBottom = false,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          MouseRegion(
            cursor: SystemMouseCursors.resizeRow,
            child: Listener(
              onPointerDown: (_) => _draggingBottom = true,
              child: Container(
                height: 4,
                color: AppTheme.borderColor,
                child: Container(
                  height: 1,
                  margin: const EdgeInsets.symmetric(horizontal: 4),
                  color: AppTheme.borderColor,
                ),
              ),
            ),
          ),
          SizedBox(
            height: height,
            child: Container(
              color: AppTheme.bgPanel,
              child: Column(
                children: [
                  DockTabBar(
                    area: DockArea.bottom,
                    stack: layout.bottom,
                    onTap: (i) =>
                        ref.read(dockLayoutProvider.notifier).setBottomTab(i),
                    onClose: () =>
                        ref.read(dockLayoutProvider.notifier).toggleBottom(),
                    onReorder: (oldI, newI) => ref
                        .read(dockLayoutProvider.notifier)
                        .reorderTab(DockArea.bottom, oldI, newI),
                  ),
                  Expanded(child: _buildPanelContent(layout.bottom.active)),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  // ═══════════════════════ 面板内容 ═══════════════════════

  Widget _buildPanelContent(PanelKind? kind) {
    if (kind == null) return const SizedBox.shrink();
    switch (kind) {
      case PanelKind.terminal:
        return TerminalPanel(port: _selectedPort);
      case PanelKind.sender:
        return SenderPanel(port: _selectedPort);
      case PanelKind.logs:
        return const LogPanel();
      case PanelKind.chart:
        return const DynamicPanelsView();
      case PanelKind.plugins:
        return const PluginsPanel();
      case PanelKind.dynamic:
        return const DynamicPanelsView();
      case PanelKind.replay:
        return const ReplayPanel();
      case PanelKind.settings:
        return const SettingsPanel();
      case PanelKind.devices:
        return _buildDevicesPanel();
    }
  }

  Widget _buildDevicesPanel() {
    final port = _selectedPort;
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 8),
            decoration: const BoxDecoration(
              color: AppTheme.bgPanel,
              border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
            ),
            child: Row(
              children: [
                const Text(
                  '设备信息',
                  style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
                ),
              ],
            ),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(8),
              children: [
                _signalSwitch(
                  'DTR',
                  _dtr,
                  port == null
                      ? null
                      : (value) => _runSerialAction(() {
                          ref.read(backendServiceProvider).setDtr(port, value);
                          setState(() => _dtr = value);
                        }),
                ),
                _signalSwitch(
                  'RTS',
                  _rts,
                  port == null
                      ? null
                      : (value) => _runSerialAction(() {
                          ref.read(backendServiceProvider).setRts(port, value);
                          setState(() => _rts = value);
                        }),
                ),
                const SizedBox(height: 8),
                _infoRow('端口', port ?? '未选择'),
                _infoRow('波特率', _baudRate),
                const Text(
                  '其余串口参数可在“设置”面板修改；重新打开端口后生效。',
                  style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _signalSwitch(
    String label,
    bool value,
    ValueChanged<bool>? onChanged,
  ) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      margin: const EdgeInsets.only(bottom: 2),
      decoration: BoxDecoration(
        color: AppTheme.bgInput,
        borderRadius: BorderRadius.circular(4),
      ),
      child: Row(
        children: [
          Text(
            label,
            style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
          ),
          const Spacer(),
          Switch(
            value: value,
            onChanged: onChanged,
            materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
          ),
        ],
      ),
    );
  }

  Widget _infoRow(String label, String value) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
      margin: const EdgeInsets.only(bottom: 2),
      decoration: BoxDecoration(
        color: AppTheme.bgInput,
        borderRadius: BorderRadius.circular(4),
      ),
      child: Row(
        children: [
          Text(
            label,
            style: const TextStyle(
              fontSize: 12,
              color: AppTheme.textPrimary,
              fontFamily: 'monospace',
            ),
          ),
          const Spacer(),
          Text(
            value,
            style: const TextStyle(
              fontSize: 12,
              color: AppTheme.textSecondary,
              fontFamily: 'monospace',
            ),
          ),
        ],
      ),
    );
  }

  // ═══════════════════════ 状态栏 ═══════════════════════

  Widget _buildStatusBar() {
    final status = ref.watch(backendStatusProvider).valueOrNull;
    final selectedOpen =
        _selectedPort != null &&
        (status?.openPorts.contains(_selectedPort) ?? false);
    return Container(
      height: 24,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: AppTheme.bgPanel,
        border: Border(top: BorderSide(color: AppTheme.borderColor)),
      ),
      child: LayoutBuilder(
        builder: (context, constraints) => Row(
          children: [
            Container(
              width: 8,
              height: 8,
              margin: const EdgeInsets.only(right: 4),
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: selectedOpen ? AppTheme.success : AppTheme.textSecondary,
                boxShadow: [
                  BoxShadow(
                    color:
                        (selectedOpen
                                ? AppTheme.success
                                : AppTheme.textSecondary)
                            .withValues(alpha: 0.4),
                    blurRadius: 4,
                    spreadRadius: 1,
                  ),
                ],
              ),
            ),
            Expanded(
              child: Text(
                selectedOpen ? '$_selectedPort @ $_baudRate' : '串口已关闭',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 11,
                  color: AppTheme.textSecondary,
                ),
              ),
            ),
            if (constraints.maxWidth >= 360) const SizedBox(width: 16),
            Container(
              width: 8,
              height: 8,
              margin: const EdgeInsets.only(right: 4),
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: status?.recording == true
                    ? AppTheme.error
                    : AppTheme.textSecondary,
              ),
            ),
            if (constraints.maxWidth >= 300)
              Text(
                status?.recording == true ? '录制中' : '未录制',
                style: TextStyle(
                  fontSize: 11,
                  color: status?.recording == true
                      ? AppTheme.error
                      : AppTheme.textSecondary,
                ),
              ),
            if (constraints.maxWidth >= 520) ...[
              const SizedBox(width: 12),
              Text(
                '端口: ${status?.portsCount ?? 0}',
                style: const TextStyle(
                  fontSize: 11,
                  color: AppTheme.textSecondary,
                ),
              ),
              const SizedBox(width: 12),
              Text(
                '插件: ${status?.pluginsCount ?? 0}',
                style: const TextStyle(
                  fontSize: 11,
                  color: AppTheme.textSecondary,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
