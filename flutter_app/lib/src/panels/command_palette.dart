import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../backend/backend_service.dart';
import '../theme/app_theme.dart';

/// 命令面板 — Ctrl+K 弹出搜索执行命令
class CommandPalette extends StatefulWidget {
  final BackendService backend;
  final VoidCallback onClose;
  final VoidCallback? onOpenPort;
  final VoidCallback? onClosePort;
  final VoidCallback onToggleBottom;
  final VoidCallback onToggleRight;
  final VoidCallback onResetLayout;
  const CommandPalette({
    super.key,
    required this.backend,
    required this.onClose,
    this.onOpenPort,
    this.onClosePort,
    required this.onToggleBottom,
    required this.onToggleRight,
    required this.onResetLayout,
  });

  @override
  State<CommandPalette> createState() => _CommandPaletteState();
}

class _CommandPaletteState extends State<CommandPalette> {
  final _controller = TextEditingController();
  final _focusNode = FocusNode();
  int _selectedIndex = 0;

  late final List<_CmdItem> _commands;

  @override
  void initState() {
    super.initState();
    _commands = [
      _CmdItem('刷新串口列表', Icons.refresh, () => widget.backend.refreshPorts()),
      _CmdItem('打开串口', Icons.power_settings_new, widget.onOpenPort),
      _CmdItem('关闭串口', Icons.power_off, widget.onClosePort),
      _CmdItem(
        '开始/停止录制',
        Icons.fiber_manual_record,
        () => widget.backend.toggleRecording(),
      ),
      _CmdItem('保存配置', Icons.save, () => widget.backend.saveConfig()),
      _CmdItem('加载配置', Icons.download, () => widget.backend.loadConfig()),
      _CmdItem(
        '切换底部面板',
        Icons.keyboard_arrow_up,
        widget.onToggleBottom,
        isLayout: true,
      ),
      _CmdItem(
        '切换右侧面板',
        Icons.chevron_left,
        widget.onToggleRight,
        isLayout: true,
      ),
      _CmdItem('重置布局', Icons.restart_alt, widget.onResetLayout, isLayout: true),
    ];
    _focusNode.requestFocus();
  }

  @override
  void dispose() {
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  List<_CmdItem> get _filteredCommands {
    final q = _controller.text.toLowerCase();
    if (q.isEmpty) return _commands;
    return _commands.where((c) => c.label.toLowerCase().contains(q)).toList();
  }

  void _executeSelected() {
    final filtered = _filteredCommands;
    if (_selectedIndex >= filtered.length) return;
    final action = filtered[_selectedIndex].action;
    if (action == null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('请先在顶部选择串口')));
    } else {
      action();
    }
    widget.onClose();
  }

  @override
  Widget build(BuildContext context) {
    final filtered = _filteredCommands;

    return GestureDetector(
      onTap: widget.onClose,
      child: Container(
        color: Colors.black54,
        child: Center(
          child: GestureDetector(
            onTap: () {},
            child: LayoutBuilder(
              builder: (context, constraints) => Focus(
                autofocus: true,
                onKeyEvent: (node, event) {
                  if (event is KeyDownEvent || event is KeyRepeatEvent) {
                    if (event.logicalKey == LogicalKeyboardKey.escape) {
                      widget.onClose();
                      return KeyEventResult.handled;
                    } else if (event.logicalKey == LogicalKeyboardKey.arrowUp) {
                      setState(
                        () => _selectedIndex = (_selectedIndex - 1).clamp(
                          0,
                          filtered.length - 1,
                        ),
                      );
                      return KeyEventResult.handled;
                    } else if (event.logicalKey ==
                        LogicalKeyboardKey.arrowDown) {
                      setState(
                        () => _selectedIndex = (_selectedIndex + 1).clamp(
                          0,
                          filtered.length - 1,
                        ),
                      );
                      return KeyEventResult.handled;
                    } else if (event.logicalKey == LogicalKeyboardKey.enter) {
                      _executeSelected();
                      return KeyEventResult.handled;
                    }
                  }
                  return KeyEventResult.ignored;
                },
                child: Container(
                  width: constraints.maxWidth.clamp(0.0, 420.0).toDouble(),
                  constraints: const BoxConstraints(maxHeight: 400),
                  decoration: BoxDecoration(
                    color: AppTheme.bgPanel,
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: AppTheme.borderColor),
                    boxShadow: [
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.3),
                        blurRadius: 20,
                        offset: const Offset(0, 8),
                      ),
                    ],
                  ),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Container(
                        padding: const EdgeInsets.all(8),
                        child: TextField(
                          controller: _controller,
                          focusNode: _focusNode,
                          style: const TextStyle(
                            fontSize: 14,
                            color: AppTheme.textPrimary,
                          ),
                          decoration: const InputDecoration(
                            hintText: '搜索命令…',
                            prefixIcon: Icon(
                              Icons.search,
                              size: 18,
                              color: AppTheme.textSecondary,
                            ),
                            border: InputBorder.none,
                            fillColor: AppTheme.bgInput,
                            filled: true,
                            contentPadding: EdgeInsets.symmetric(
                              horizontal: 8,
                              vertical: 8,
                            ),
                          ),
                          onChanged: (_) => setState(() => _selectedIndex = 0),
                          onSubmitted: (_) => _executeSelected(),
                        ),
                      ),
                      if (filtered.isEmpty)
                        const Padding(
                          padding: EdgeInsets.all(24),
                          child: Text(
                            '无匹配命令',
                            style: TextStyle(color: AppTheme.textSecondary),
                          ),
                        )
                      else
                        ConstrainedBox(
                          constraints: const BoxConstraints(maxHeight: 300),
                          child: ListView.builder(
                            shrinkWrap: true,
                            itemCount: filtered.length,
                            itemBuilder: (_, i) {
                              final sel = i == _selectedIndex;
                              final cmd = filtered[i];
                              return InkWell(
                                onTap: () {
                                  _selectedIndex = i;
                                  _executeSelected();
                                },
                                onHover: (v) =>
                                    setState(() => _selectedIndex = i),
                                child: Container(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 12,
                                    vertical: 8,
                                  ),
                                  color: sel
                                      ? AppTheme.bgActive
                                      : cmd.action == null
                                      ? AppTheme.bgHover
                                      : null,
                                  child: Row(
                                    children: [
                                      Icon(
                                        cmd.icon,
                                        size: 16,
                                        color: cmd.action == null
                                            ? AppTheme.textSecondary
                                            : sel
                                            ? AppTheme.textBright
                                            : AppTheme.textSecondary,
                                      ),
                                      const SizedBox(width: 8),
                                      Text(
                                        cmd.label,
                                        style: TextStyle(
                                          fontSize: 13,
                                          color: cmd.action == null
                                              ? AppTheme.textSecondary
                                              : sel
                                              ? AppTheme.textBright
                                              : AppTheme.textPrimary,
                                        ),
                                      ),
                                      if (cmd.isLayout) ...[
                                        const Spacer(),
                                        Text(
                                          '布局',
                                          style: TextStyle(
                                            fontSize: 10,
                                            color: AppTheme.textSecondary,
                                          ),
                                        ),
                                      ],
                                    ],
                                  ),
                                ),
                              );
                            },
                          ),
                        ),
                      Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 6,
                        ),
                        decoration: const BoxDecoration(
                          border: Border(
                            top: BorderSide(color: AppTheme.borderColor),
                          ),
                        ),
                        child: const Text(
                          '↑↓ 选择 · Enter 执行 · Esc 关闭',
                          style: TextStyle(
                            fontSize: 11,
                            color: AppTheme.textSecondary,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _CmdItem {
  final String label;
  final IconData icon;
  final VoidCallback? action;
  final bool isLayout;
  const _CmdItem(this.label, this.icon, this.action, {this.isLayout = false});
}
