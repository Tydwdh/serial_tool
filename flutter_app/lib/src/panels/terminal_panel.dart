import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/models.dart';
import '../providers/backend_provider.dart';
import '../theme/app_theme.dart';

class _TerminalLine {
  final int id;
  final String port;
  final bool isRx;
  final String text;
  final String hex;
  final int timestamp;
  bool bookmarked = false;

  _TerminalLine({
    required this.id,
    required this.port,
    required this.isRx,
    required this.text,
    required this.hex,
    required this.timestamp,
  });
}

class TerminalPanel extends ConsumerStatefulWidget {
  final String? port;
  const TerminalPanel({super.key, this.port});

  @override
  ConsumerState<TerminalPanel> createState() => _TerminalPanelState();
}

class _TerminalPanelState extends ConsumerState<TerminalPanel> {
  final _entries = <_TerminalLine>[];
  int _nextId = 0;
  final int _maxEntries = 50000;

  bool _showHex = false;
  bool _showRaw = false;
  bool _showTimestamp = true;
  bool _showPort = true;
  final bool _showDirection = true;
  bool _showRx = true;
  bool _showTx = true;
  final bool _autoScroll = true;
  bool _paused = false;
  int _pausedDropped = 0;

  bool _searchVisible = false;
  final _searchController = TextEditingController();
  String _searchText = '';
  bool _searchCaseSensitive = false;

  final _scrollController = ScrollController();
  bool _userScrolled = false;

  Timer? _batchTimer;
  final _batch = <_TerminalLine>[];
  final _pendingPackets = <String, List<int>>{};
  final _fragmentTimers = <String, Timer>{};

  @override
  void initState() {
    super.initState();
    _scrollController.addListener(_onScroll);
    ref.listen(backendEventStreamProvider, (prev, next) {
      next.whenData((event) => _onBackendEvent(event));
    });
  }

  @override
  void dispose() {
    _batchTimer?.cancel();
    for (final timer in _fragmentTimers.values) {
      timer.cancel();
    }
    _searchController.dispose();
    _scrollController.removeListener(_onScroll);
    _scrollController.dispose();
    super.dispose();
  }

  void _onBackendEvent(BackendEvent event) {
    if (event.type == 'serial_data') {
      final port = event.data['port'] as String? ?? '';
      if (widget.port != null && port != widget.port) return;
      final dir = event.data['direction'] as String?;
      final isRx = dir == 'rx';
      final rawData = event.data['data'];
      List<int>? bytes;
      if (rawData is List) {
        bytes = rawData.cast<int>();
      }
      if (bytes == null) return;
      _ingestPacket(
        port: port,
        isRx: isRx,
        bytes: bytes,
        timestamp: (event.data['timestamp'] as num?)?.toInt() ?? 0,
      );
    }
  }

  void _onScroll() {
    if (!_scrollController.hasClients) return;
    final atBottom =
        _scrollController.position.pixels >=
        _scrollController.position.maxScrollExtent - 5;
    if (_userScrolled && atBottom) {
      setState(() => _userScrolled = false);
    }
    if (!atBottom && !_userScrolled) {
      _userScrolled = true;
    }
  }

  void _scrollToBottom() {
    if (!_scrollController.hasClients) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scrollController.hasClients) {
        _scrollController.jumpTo(_scrollController.position.maxScrollExtent);
      }
    });
  }

  List<_TerminalLine> get _filteredEntries {
    var result = _entries;
    if (widget.port != null) {
      result = result.where((e) => e.port == widget.port).toList();
    }
    if (!_showRx || !_showTx) {
      result = result
          .where((e) => _showRx && e.isRx || _showTx && !e.isRx)
          .toList();
    }
    if (_searchText.isNotEmpty) {
      result = result.where((e) {
        final text = _searchCaseSensitive ? e.text : e.text.toLowerCase();
        final query = _searchCaseSensitive
            ? _searchText
            : _searchText.toLowerCase();
        return text.contains(query);
      }).toList();
    }
    return result;
  }

  void _ingestPacket({
    required String port,
    required bool isRx,
    required List<int> bytes,
    required int timestamp,
  }) {
    // TX is a complete user operation. RX is line-buffered so that a serial
    // frame split across multiple reads remains one terminal row.
    if (!isRx) {
      _addBytes(port: port, isRx: false, bytes: bytes, timestamp: timestamp);
      return;
    }
    final key = '$port\u0000$isRx';
    final pending = _pendingPackets.putIfAbsent(key, () => <int>[]);
    pending.addAll(bytes);
    _fragmentTimers.remove(key)?.cancel();

    var lineStart = 0;
    for (var index = 0; index < pending.length; index++) {
      if (pending[index] == 0x0a) {
        _addBytes(
          port: port,
          isRx: true,
          bytes: pending.sublist(lineStart, index + 1),
          timestamp: timestamp,
        );
        lineStart = index + 1;
      }
    }
    if (lineStart > 0) pending.removeRange(0, lineStart);
    if (pending.isEmpty) {
      _pendingPackets.remove(key);
      return;
    }

    // Binary protocols and devices without line endings must remain visible.
    // A short quiet period is enough to merge normal split packets first.
    _fragmentTimers[key] = Timer(const Duration(milliseconds: 180), () {
      final fragment = _pendingPackets.remove(key);
      _fragmentTimers.remove(key);
      if (fragment != null && fragment.isNotEmpty && mounted) {
        _addBytes(
          port: port,
          isRx: true,
          bytes: fragment,
          timestamp: timestamp,
        );
      }
    });
  }

  void _addBytes({
    required String port,
    required bool isRx,
    required List<int> bytes,
    required int timestamp,
  }) {
    _addEntry(
      port: port,
      isRx: isRx,
      text: utf8.decode(bytes, allowMalformed: true),
      hex: bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join(' '),
      timestamp: timestamp,
    );
  }

  void _addEntry({
    required String port,
    required bool isRx,
    required String text,
    required String hex,
    required int timestamp,
  }) {
    if (_paused) {
      _pausedDropped++;
      return;
    }
    _batch.add(
      _TerminalLine(
        id: _nextId++,
        port: port,
        isRx: isRx,
        text: text,
        hex: hex,
        timestamp: timestamp,
      ),
    );
    _batchTimer?.cancel();
    _batchTimer = Timer(const Duration(milliseconds: 16), _flushBatch);
  }

  void _flushBatch() {
    if (_batch.isEmpty) return;
    _batchTimer?.cancel();
    setState(() {
      _entries.addAll(_batch);
      _batch.clear();
      if (_entries.length > _maxEntries) {
        final remove = _entries.length - _maxEntries;
        _entries.removeRange(0, remove);
      }
    });
    if (_autoScroll && !_userScrolled) {
      _scrollToBottom();
    }
  }

  void _clear() {
    setState(() {
      _entries.clear();
      _batch.clear();
      _batchTimer?.cancel();
      _nextId = 0;
      _pendingPackets.clear();
      for (final timer in _fragmentTimers.values) {
        timer.cancel();
      }
      _fragmentTimers.clear();
    });
  }

  Future<void> _export() async {
    if (_entries.isEmpty) return;
    final lines = _filteredEntries.map((entry) {
      final time = DateTime.fromMillisecondsSinceEpoch(
        entry.timestamp,
      ).toIso8601String();
      final direction = entry.isRx ? 'RX' : 'TX';
      return '[$time] [${entry.port}] [$direction] ${entry.text.replaceAll('\r', '\\r').replaceAll('\n', '\\n')}';
    });
    try {
      final filename =
          'terminal-${DateTime.now().toIso8601String().replaceAll(':', '-')}.log';
      final selectedPath = ref
          .read(backendServiceProvider)
          .pickTerminalExportPath(filename);
      if (selectedPath == null || !mounted) return;
      final output = File(selectedPath);
      await output.writeAsString('${lines.join('\n')}\n', flush: true);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('已导出 ${_filteredEntries.length} 条记录：${output.path}'),
          ),
        );
      }
    } on FileSystemException catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('导出失败：${error.message}')));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          _buildToolbar(),
          if (_searchVisible) _buildSearchBar(),
          Expanded(child: _buildTerminalList()),
          _buildStatusBar(),
        ],
      ),
    );
  }

  Widget _buildToolbar() {
    return Container(
      height: 28,
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
            _toolBtn(
              Icons.search,
              _searchVisible,
              () => setState(() => _searchVisible = !_searchVisible),
              '搜索',
            ),
            _toolBtn(Icons.pause_circle_outline, _paused, _togglePaused, '暂停'),
            _toolBtn(Icons.file_download_outlined, false, _export, '导出当前筛选结果'),
            _toolBtn(Icons.delete_outline, false, _clear, '清空'),
            if (_paused && _pausedDropped > 0)
              Padding(
                padding: const EdgeInsets.only(left: 8),
                child: Text(
                  '丢包: $_pausedDropped',
                  style: const TextStyle(fontSize: 10, color: AppTheme.warning),
                ),
              ),
            const SizedBox(width: 12),
            if (_entries.isNotEmpty && widget.port == null)
              _filterBtn('端口', _showPort, (v) => setState(() => _showPort = v)),
            _filterBtn('HEX', _showHex, (v) => setState(() => _showHex = v)),
            const SizedBox(width: 4),
            _filterBtn('RAW', _showRaw, (v) => setState(() => _showRaw = v)),
            const SizedBox(width: 4),
            _filterBtn(
              'TS',
              _showTimestamp,
              (v) => setState(() => _showTimestamp = v),
            ),
            const SizedBox(width: 4),
            _filterBtn('RX', _showRx, (v) => setState(() => _showRx = v)),
            const SizedBox(width: 4),
            _filterBtn('TX', _showTx, (v) => setState(() => _showTx = v)),
          ],
        ),
      ),
    );
  }

  Widget _toolBtn(
    IconData icon,
    bool active,
    VoidCallback onPressed,
    String tooltip,
  ) {
    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 2),
      child: IconButton(
        icon: Icon(icon, size: 14),
        color: active ? AppTheme.accent : AppTheme.textSecondary,
        onPressed: onPressed,
        tooltip: tooltip,
        style: IconButton.styleFrom(
          padding: EdgeInsets.zero,
          minimumSize: const Size(24, 24),
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          backgroundColor: active ? AppTheme.bgActive : null,
        ),
      ),
    );
  }

  void _togglePaused() {
    final next = !_paused;
    try {
      ref.read(backendServiceProvider).setTerminalPaused(next);
      setState(() => _paused = next);
    } catch (_) {
      // Preserve the current visual state when the backend rejects the change.
    }
  }

  Widget _filterBtn(String label, bool active, ValueChanged<bool> onChanged) {
    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 1),
      child: TextButton(
        onPressed: () => onChanged(!active),
        style: TextButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 5),
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

  Widget _buildSearchBar() {
    return Container(
      height: 32,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: const BoxDecoration(
        color: AppTheme.bgInput,
        border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
      ),
      child: Row(
        children: [
          const Icon(Icons.search, size: 14, color: AppTheme.textSecondary),
          const SizedBox(width: 6),
          Expanded(
            child: TextField(
              controller: _searchController,
              style: const TextStyle(
                fontSize: 12,
                color: AppTheme.textPrimary,
                fontFamily: 'monospace',
              ),
              decoration: const InputDecoration(
                hintText: '搜索…',
                border: InputBorder.none,
                contentPadding: EdgeInsets.zero,
                isDense: true,
                hintStyle: TextStyle(
                  fontSize: 12,
                  color: AppTheme.textSecondary,
                ),
              ),
              onChanged: (v) => setState(() => _searchText = v),
            ),
          ),
          _filterBtn(
            'Aa',
            _searchCaseSensitive,
            (v) => setState(() => _searchCaseSensitive = v),
          ),
          IconButton(
            icon: const Icon(Icons.close, size: 14),
            color: AppTheme.textSecondary,
            onPressed: () {
              setState(() {
                _searchVisible = false;
                _searchText = '';
                _searchController.clear();
              });
            },
            style: IconButton.styleFrom(
              padding: EdgeInsets.zero,
              minimumSize: const Size(24, 24),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildTerminalList() {
    final filtered = _filteredEntries;
    if (filtered.isEmpty) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Text(
              _searchText.isNotEmpty ? '无匹配结果' : '等待串口数据…',
              style: const TextStyle(
                fontSize: 13,
                color: AppTheme.textSecondary,
                fontFamily: 'monospace',
              ),
            ),
            const SizedBox(height: 8),
          ],
        ),
      );
    }

    return ListView.builder(
      controller: _scrollController,
      itemCount: filtered.length,
      itemExtent: 22,
      itemBuilder: (_, i) {
        final entry = filtered[i];
        final totalSec = entry.timestamp ~/ 1000;
        final h = totalSec ~/ 3600;
        final m = (totalSec ~/ 60) % 60;
        final s = totalSec % 60;
        final ts = _showTimestamp
            ? '${h.toString().padLeft(2, '0')}:${m.toString().padLeft(2, '0')}:${s.toString().padLeft(2, '0')}'
            : '';
        final dir = entry.isRx ? 'RX' : 'TX';
        final dirColor = entry.isRx ? AppTheme.rxColor : AppTheme.txColor;
        String content;
        if (_showHex) {
          content = entry.hex;
        } else if (_showRaw) {
          content = entry.text
              .replaceAll('\\', '\\\\')
              .replaceAll('\n', '\\n')
              .replaceAll('\r', '\\r')
              .replaceAll('\t', '\\t');
        } else {
          content = entry.text.replaceFirst(RegExp(r'[\r\n]+$'), '');
        }

        return Container(
          height: 22,
          padding: const EdgeInsets.symmetric(horizontal: 4),
          decoration: BoxDecoration(
            color: i.isEven ? Colors.transparent : AppTheme.bgHover,
          ),
          child: Row(
            children: [
              if (_showTimestamp)
                SizedBox(
                  width: 80,
                  child: Text(
                    ts,
                    style: const TextStyle(
                      fontSize: 11,
                      color: AppTheme.textSecondary,
                      fontFamily: 'monospace',
                    ),
                  ),
                ),
              if (_showPort)
                SizedBox(
                  width: 50,
                  child: Text(
                    entry.port,
                    style: const TextStyle(
                      fontSize: 11,
                      color: AppTheme.textSecondary,
                      fontFamily: 'monospace',
                    ),
                  ),
                ),
              if (_showDirection)
                SizedBox(
                  width: 24,
                  child: Text(
                    dir,
                    style: TextStyle(
                      fontSize: 10,
                      color: dirColor,
                      fontFamily: 'monospace',
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                ),
              Expanded(
                child: Text(
                  content,
                  style: TextStyle(
                    fontSize: 12,
                    color: _showHex
                        ? AppTheme.textSecondary
                        : AppTheme.textPrimary,
                    fontFamily: 'monospace',
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildStatusBar() {
    return Container(
      height: 22,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: AppTheme.bgPanel,
        border: Border(top: BorderSide(color: AppTheme.borderColor)),
      ),
      child: Row(
        children: [
          if (widget.port != null)
            Text(
              '端口: ${widget.port}',
              style: const TextStyle(
                fontSize: 11,
                color: AppTheme.textSecondary,
              ),
            )
          else
            const Text(
              '全部端口',
              style: TextStyle(fontSize: 11, color: AppTheme.textSecondary),
            ),
          const Spacer(),
          Text(
            '${_entries.length} 条',
            style: const TextStyle(fontSize: 11, color: AppTheme.textSecondary),
          ),
          if (_paused) ...[
            const SizedBox(width: 8),
            const Text(
              '已暂停',
              style: TextStyle(fontSize: 11, color: AppTheme.warning),
            ),
          ],
        ],
      ),
    );
  }
}
