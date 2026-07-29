import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../providers/backend_provider.dart';
import '../theme/app_theme.dart';

/// 发送器面板 — 文本/HEX 发送、历史、周期发送
class SenderPanel extends ConsumerStatefulWidget {
  final String? port;
  const SenderPanel({super.key, this.port});

  @override
  ConsumerState<SenderPanel> createState() => _SenderPanelState();
}

class _SenderPanelState extends ConsumerState<SenderPanel> {
  final _controller = TextEditingController();
  final _intervalController = TextEditingController(text: '1000');
  Timer? _periodicTimer;
  bool _hexMode = false;
  bool _hexStrict = false;
  String _lineEnding = 'none';
  bool _periodicSend = false;
  final _history = <String>[];
  String? _error;

  final _lineEndings = ['none', 'LF', 'CR', 'CRLF'];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadSettings());
  }

  void _loadSettings() {
    try {
      final config = ref.read(backendServiceProvider).getConfig();
      final send = Map<String, dynamic>.from(config['send'] as Map? ?? {});
      final history = (send['history'] as List? ?? const [])
          .whereType<String>()
          .toList(growable: false);
      if (!mounted) return;
      setState(() {
        _history
          ..clear()
          ..addAll(history);
        _hexMode = send['hex_mode'] as bool? ?? _hexMode;
        _hexStrict = send['strict_hex'] as bool? ?? _hexStrict;
        _lineEnding = _lineEndingFromValue(send['line_ending']?.toString());
        _intervalController.text =
            (send['periodic_interval_ms'] as num?)?.toInt().toString() ??
            _intervalController.text;
      });
    } catch (_) {
      // A missing old config simply keeps the default sender settings.
    }
  }

  String _lineEndingFromValue(String? value) => switch (value) {
    '\n' => 'LF',
    '\r' => 'CR',
    '\r\n' => 'CRLF',
    _ => 'none',
  };

  void _persistSettings() {
    try {
      final interval = int.tryParse(_intervalController.text.trim()) ?? 1000;
      final backend = ref.read(backendServiceProvider);
      backend.setSendConfig({
        'history': _history,
        'line_ending': _getLineEndingValue(),
        'hex_mode': _hexMode,
        'strict_hex': _hexStrict,
        'periodic_enabled': _periodicSend,
        'periodic_interval_ms': interval.clamp(10, 3600000),
      });
      backend.saveConfig();
    } catch (_) {
      // Sending should remain usable even if persistence is temporarily unavailable.
    }
  }

  @override
  void dispose() {
    _periodicTimer?.cancel();
    _controller.dispose();
    _intervalController.dispose();
    super.dispose();
  }

  String _getLineEndingValue() {
    switch (_lineEnding) {
      case 'LF':
        return '\n';
      case 'CR':
        return '\r';
      case 'CRLF':
        return '\r\n';
      default:
        return '';
    }
  }

  String _hexPreview() {
    if (!_hexMode || _controller.text.isEmpty) return '—';
    try {
      final bytes = _parseHex(_controller.text);
      return bytes
          .map((b) => b >= 32 && b < 127 ? String.fromCharCode(b) : '.')
          .join();
    } catch (_) {
      return '无效 HEX';
    }
  }

  List<int> _parseHex(String input) {
    final cleaned = input.replaceAll(RegExp(r'[^0-9a-fA-F]'), '');
    if (cleaned.isEmpty) return [];
    if (cleaned.length.isOdd) {
      if (_hexStrict) throw FormatException('奇数 HEX 长度');
      return [int.parse(cleaned, radix: 16)];
    }
    final bytes = <int>[];
    for (int i = 0; i < cleaned.length; i += 2) {
      bytes.add(int.parse(cleaned.substring(i, i + 2), radix: 16));
    }
    return bytes;
  }

  bool _send({bool fromTimer = false}) {
    final port = widget.port;
    if (port == null) {
      setState(() => _error = '未选择端口');
      return false;
    }
    final text = _controller.text;
    if (text.isEmpty) return false;

    if (_hexMode) {
      try {
        _parseHex(text);
      } on FormatException catch (error) {
        setState(() => _error = error.message);
        return false;
      }
    }

    // 追加换行符
    final data = _hexMode ? text : text + _getLineEndingValue();

    // 调用后端发送
    try {
      ref.read(backendServiceProvider).sendData(port, data, hex: _hexMode);
      // 保存历史
      setState(() {
        _history.remove(text);
        _history.add(text);
        if (_history.length > 200) _history.removeAt(0);
        _error = null;
      });
      _persistSettings();
      return true;
    } catch (e) {
      setState(() => _error = e.toString());
      if (fromTimer) _setPeriodic(false);
      return false;
    }
  }

  void _setPeriodic(bool enabled) {
    _periodicTimer?.cancel();
    _periodicTimer = null;
    if (!enabled) {
      setState(() => _periodicSend = false);
      _persistSettings();
      return;
    }

    final interval = int.tryParse(_intervalController.text.trim());
    if (interval == null || interval < 10 || interval > 3600000) {
      setState(() {
        _periodicSend = false;
        _error = '周期必须在 10–3600000 ms 之间';
      });
      return;
    }
    if (widget.port == null || _controller.text.isEmpty) {
      setState(() {
        _periodicSend = false;
        _error = widget.port == null ? '未选择端口' : '发送内容为空';
      });
      return;
    }

    setState(() {
      _periodicSend = true;
      _error = null;
    });
    _periodicTimer = Timer.periodic(
      Duration(milliseconds: interval),
      (_) => _send(fromTimer: true),
    );
    _persistSettings();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          _buildOptionsBar(),
          Expanded(child: _buildInputArea()),
          _buildActionsBar(),
          if (_error != null) _buildErrorBar(),
        ],
      ),
    );
  }

  Widget _buildOptionsBar() {
    return Container(
      height: 32,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: AppTheme.bgPanel,
        border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
      ),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (widget.port != null)
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: AppTheme.bgActive,
                  borderRadius: BorderRadius.circular(3),
                ),
                child: Text(
                  widget.port!,
                  style: const TextStyle(
                    fontSize: 11,
                    color: AppTheme.textPrimary,
                    fontFamily: 'monospace',
                  ),
                ),
              ),
            if (widget.port == null)
              const Text(
                '未选择端口',
                style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
              ),
            const SizedBox(width: 8),
            _toggleBtn('HEX', _hexMode, (v) {
              setState(() => _hexMode = v);
              _persistSettings();
            }),
            if (_hexMode) ...[
              const SizedBox(width: 4),
              _toggleBtn('严格', _hexStrict, (v) {
                setState(() => _hexStrict = v);
                _persistSettings();
              }),
            ],
            const SizedBox(width: 8),
            const Text(
              '换行:',
              style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
            ),
            const SizedBox(width: 4),
            SizedBox(
              width: 56,
              child: DropdownButtonHideUnderline(
                child: DropdownButton<String>(
                  value: _lineEnding,
                  isDense: true,
                  dropdownColor: AppTheme.bgInput,
                  style: const TextStyle(
                    fontSize: 11,
                    color: AppTheme.textPrimary,
                    fontFamily: 'monospace',
                  ),
                  items: _lineEndings
                      .map(
                        (e) => DropdownMenuItem(
                          value: e,
                          child: Text(e, style: const TextStyle(fontSize: 11)),
                        ),
                      )
                      .toList(),
                  onChanged: (v) {
                    setState(() => _lineEnding = v ?? 'none');
                    _persistSettings();
                  },
                ),
              ),
            ),
            const SizedBox(width: 24),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text(
                  '周期:',
                  style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
                ),
                const SizedBox(width: 4),
                SizedBox(
                  width: 50,
                  child: TextField(
                    controller: _intervalController,
                    style: const TextStyle(
                      fontSize: 11,
                      color: AppTheme.textPrimary,
                      fontFamily: 'monospace',
                    ),
                    decoration: const InputDecoration(
                      border: InputBorder.none,
                      isDense: true,
                      contentPadding: EdgeInsets.symmetric(
                        horizontal: 4,
                        vertical: 2,
                      ),
                    ),
                    keyboardType: TextInputType.number,
                    onChanged: (_) {},
                  ),
                ),
                const Text(
                  'ms',
                  style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
                ),
                const SizedBox(width: 4),
                Switch(
                  value: _periodicSend,
                  onChanged: _setPeriodic,
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _toggleBtn(String label, bool active, ValueChanged<bool> onChanged) {
    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 1),
      child: TextButton(
        onPressed: () => onChanged(!active),
        style: TextButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 6),
          minimumSize: Size.zero,
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          foregroundColor: active ? AppTheme.accent : AppTheme.textSecondary,
          backgroundColor: active ? AppTheme.bgActive : null,
          textStyle: const TextStyle(
            fontSize: 10,
            fontFamily: 'monospace',
            fontWeight: FontWeight.w600,
          ),
        ),
        child: Text(label),
      ),
    );
  }

  Widget _buildInputArea() {
    return Container(
      margin: const EdgeInsets.all(4),
      decoration: BoxDecoration(
        color: AppTheme.bgInput,
        borderRadius: BorderRadius.circular(4),
        border: Border.all(color: AppTheme.borderColor),
      ),
      child: Column(
        children: [
          Expanded(
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter, control: true):
                    _send,
              },
              child: TextField(
                controller: _controller,
                maxLines: null,
                expands: true,
                style: const TextStyle(
                  fontSize: 13,
                  color: AppTheme.textPrimary,
                  fontFamily: 'monospace',
                ),
                decoration: InputDecoration(
                  border: InputBorder.none,
                  contentPadding: const EdgeInsets.all(8),
                  hintText: _hexMode
                      ? '输入 HEX (如: 41 42 43)…'
                      : '输入要发送的数据… (Ctrl+Enter 发送)',
                  hintStyle: const TextStyle(
                    fontSize: 13,
                    color: AppTheme.textSecondary,
                    fontFamily: 'monospace',
                  ),
                ),
                keyboardType: _hexMode
                    ? TextInputType.text
                    : TextInputType.multiline,
                onChanged: (_) => setState(() {}),
              ),
            ),
          ),
          if (_hexMode && _controller.text.isNotEmpty)
            Container(
              height: 20,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              decoration: const BoxDecoration(
                border: Border(top: BorderSide(color: AppTheme.borderColor)),
              ),
              child: Row(
                children: [
                  const Text(
                    '预览: ',
                    style: TextStyle(
                      fontSize: 10,
                      color: AppTheme.textSecondary,
                      fontFamily: 'monospace',
                    ),
                  ),
                  Expanded(
                    child: Text(
                      _hexPreview(),
                      style: const TextStyle(
                        fontSize: 10,
                        color: AppTheme.textSecondary,
                        fontFamily: 'monospace',
                      ),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildActionsBar() {
    return Container(
      height: 32,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        border: Border(top: BorderSide(color: AppTheme.borderColor)),
      ),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            FilledButton.icon(
              icon: const Icon(Icons.send, size: 14),
              label: const Text(
                '发送 (Ctrl+Enter)',
                style: TextStyle(fontSize: 11),
              ),
              onPressed: widget.port != null ? _send : null,
              style: FilledButton.styleFrom(
                padding: const EdgeInsets.symmetric(horizontal: 12),
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                backgroundColor: widget.port != null
                    ? AppTheme.accent
                    : AppTheme.bgInput,
              ),
            ),
            const SizedBox(width: 8),
            TextButton.icon(
              icon: const Icon(Icons.delete_outline, size: 14),
              label: const Text('清空', style: TextStyle(fontSize: 11)),
              onPressed: () => setState(() => _controller.clear()),
              style: TextButton.styleFrom(
                padding: const EdgeInsets.symmetric(horizontal: 8),
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                foregroundColor: AppTheme.textSecondary,
              ),
            ),
            const SizedBox(width: 8),
            if (_history.isNotEmpty)
              SizedBox(
                width: 200,
                child: DropdownButtonHideUnderline(
                  child: DropdownButton<String>(
                    value: null,
                    isExpanded: true,
                    hint: Text(
                      '历史 (${_history.length})',
                      style: const TextStyle(
                        fontSize: 11,
                        color: AppTheme.textSecondary,
                      ),
                    ),
                    dropdownColor: AppTheme.bgInput,
                    style: const TextStyle(
                      fontSize: 11,
                      color: AppTheme.textPrimary,
                      fontFamily: 'monospace',
                    ),
                    items: _history.reversed
                        .take(50)
                        .map(
                          (e) => DropdownMenuItem(
                            value: e,
                            child: Text(
                              e.length > 40 ? '${e.substring(0, 40)}…' : e,
                              style: const TextStyle(
                                fontSize: 11,
                                fontFamily: 'monospace',
                              ),
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        )
                        .toList(),
                    onChanged: (v) {
                      if (v != null) {
                        _controller.text = v;
                        _controller.selection = TextSelection.fromPosition(
                          TextPosition(offset: v.length),
                        );
                      }
                    },
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildErrorBar() {
    return Container(
      height: 24,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: AppTheme.error,
        border: Border(top: BorderSide(color: AppTheme.borderColor)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              _error!,
              style: const TextStyle(
                fontSize: 11,
                color: Colors.white,
                fontFamily: 'monospace',
              ),
            ),
          ),
          IconButton(
            icon: const Icon(Icons.close, size: 14, color: Colors.white),
            onPressed: () => setState(() => _error = null),
            style: IconButton.styleFrom(
              padding: EdgeInsets.zero,
              minimumSize: const Size(20, 20),
            ),
          ),
        ],
      ),
    );
  }
}
