import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/backend_service.dart';
import '../backend/models.dart';
import '../providers/backend_provider.dart';
import '../theme/app_theme.dart';

class ReplayPanel extends ConsumerStatefulWidget {
  const ReplayPanel({super.key});

  @override
  ConsumerState<ReplayPanel> createState() => _ReplayPanelState();
}

class _ReplayPanelState extends ConsumerState<ReplayPanel> {
  final _path = TextEditingController();
  Map<String, dynamic> _status = const {};
  List<Map<String, dynamic>> _recentFiles = const [];
  double? _seekPreview;
  String? _error;

  @override
  void initState() {
    super.initState();
    ref.listenManual(backendEventStreamProvider, (_, next) {
      next.whenData((BackendEvent event) {
        if (event.type == 'replay_status') {
          final status = event.data['status'];
          if (status is Map<String, dynamic> && mounted) {
            setState(() => _status = status);
          }
        }
      });
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => _refreshFiles());
  }

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  void _run(void Function(BackendService backend) command) {
    try {
      command(ref.read(backendServiceProvider));
      setState(() => _error = null);
    } catch (error) {
      setState(() => _error = error.toString());
    }
  }

  void _refreshFiles() {
    try {
      final files = ref.read(backendServiceProvider).replayFiles();
      if (mounted) setState(() => _recentFiles = files);
    } catch (_) {
      // The directory is absent before the first recording; that is normal.
    }
  }

  void _loadPath(String path) {
    if (path.trim().isEmpty) {
      setState(() => _error = '请选择或输入录制文件路径');
      return;
    }
    _path.text = path;
    _run((backend) => backend.replayLoad(path));
    _refreshFiles();
  }

  void _pickFile() {
    try {
      final path = ref.read(backendServiceProvider).pickReplayFile();
      if (path != null) _loadPath(path);
    } catch (error) {
      setState(() => _error = error.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = _status['state'] as String? ?? 'empty';
    final playing = state == 'playing';
    final duration = (_status['duration_ms'] as num?)?.toInt() ?? 0;
    final position =
        (_seekPreview?.round() ??
                (_status['position_ms'] as num?)?.toInt() ??
                0)
            .clamp(0, duration);
    final policy = _status['policy'] as String? ?? 'auto';
    final bookmarks = (_status['bookmarks'] as List? ?? const [])
        .whereType<Map>()
        .map((bookmark) => Map<String, dynamic>.from(bookmark))
        .toList(growable: false);
    return Container(
      color: AppTheme.bgDark,
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('录制回放', style: TextStyle(fontSize: 16)),
          const SizedBox(height: 12),
          LayoutBuilder(
            builder: (context, constraints) => Wrap(
              spacing: 8,
              runSpacing: 6,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                SizedBox(
                  width: (constraints.maxWidth - 156).clamp(180.0, 640.0),
                  child: TextField(
                    controller: _path,
                    decoration: const InputDecoration(
                      hintText: '输入 .jsonl 录制文件路径',
                    ),
                  ),
                ),
                FilledButton(
                  onPressed: () => _loadPath(_path.text.trim()),
                  child: const Text('加载'),
                ),
                IconButton(
                  tooltip: '选择录制文件',
                  onPressed: _pickFile,
                  icon: const Icon(Icons.upload_file),
                ),
                if (_recentFiles.isNotEmpty)
                  PopupMenuButton<Map<String, dynamic>>(
                    tooltip: '最近录制',
                    icon: const Icon(Icons.folder_open),
                    onSelected: (file) =>
                        _loadPath(file['path']?.toString() ?? ''),
                    itemBuilder: (_) => _recentFiles
                        .map(
                          (file) => PopupMenuItem(
                            value: file,
                            child: Text(
                              file['name']?.toString() ?? '录制文件',
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        )
                        .toList(),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 20),
          Wrap(
            spacing: 8,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              IconButton(
                onPressed: () =>
                    _run((backend) => backend.replayStepBackward()),
                icon: const Icon(Icons.skip_previous),
              ),
              FilledButton.icon(
                onPressed: () => _run(
                  (backend) =>
                      playing ? backend.replayPause() : backend.replayPlay(),
                ),
                icon: Icon(playing ? Icons.pause : Icons.play_arrow),
                label: Text(playing ? '暂停' : '播放'),
              ),
              IconButton(
                onPressed: () => _run((backend) => backend.replayStepForward()),
                icon: const Icon(Icons.skip_next),
              ),
              IconButton(
                onPressed: () => _run((backend) => backend.replayStop()),
                icon: const Icon(Icons.stop),
              ),
              DropdownButton<double>(
                value: (_status['speed'] as num?)?.toDouble() ?? 1,
                items: const <double>[0.5, 1, 2, 5, 10]
                    .map(
                      (speed) => DropdownMenuItem<double>(
                        value: speed,
                        child: Text('${speed}x'),
                      ),
                    )
                    .toList(),
                onChanged: (speed) {
                  if (speed != null) {
                    _run((backend) => backend.replaySetSpeed(speed));
                  }
                },
              ),
              DropdownButton<String>(
                value: const {'auto', 'exact', 'reparse'}.contains(policy)
                    ? policy
                    : 'auto',
                items: const [
                  DropdownMenuItem(value: 'auto', child: Text('自动策略')),
                  DropdownMenuItem(value: 'exact', child: Text('录制数据')),
                  DropdownMenuItem(value: 'reparse', child: Text('重新解析')),
                ],
                onChanged: (next) {
                  if (next != null) {
                    _run((backend) => backend.replaySetPolicy(next));
                  }
                },
              ),
              IconButton(
                tooltip: '在当前位置添加书签',
                onPressed: duration == 0
                    ? null
                    : () => _run((backend) => backend.replayAddBookmark()),
                icon: const Icon(Icons.bookmark_add_outlined),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Slider(
            value: duration == 0 ? 0 : position.toDouble(),
            max: duration == 0 ? 1 : duration.toDouble(),
            onChangeStart: duration == 0
                ? null
                : (value) => setState(() => _seekPreview = value),
            onChanged: duration == 0
                ? null
                : (value) => setState(() => _seekPreview = value),
            onChangeEnd: duration == 0
                ? null
                : (value) {
                    setState(() => _seekPreview = null);
                    _run((backend) => backend.replaySeek(value.round()));
                  },
          ),
          Text(
            '${_format(position)} / ${_format(duration)}  ·  ${_status['cursor'] ?? 0}/${_status['total_events'] ?? 0} 条事件  ·  ${state.toUpperCase()}',
            style: const TextStyle(
              color: AppTheme.textSecondary,
              fontFamily: 'monospace',
            ),
          ),
          if ((_status['analyzer_error'] as String?) != null ||
              (_status['analyzer_warning'] as String?) != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                (_status['analyzer_error'] ?? _status['analyzer_warning'])
                    .toString(),
                style: const TextStyle(color: AppTheme.warning),
              ),
            ),
          if (bookmarks.isNotEmpty) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: bookmarks.map((bookmark) {
                final positionMs =
                    (bookmark['position_ms'] as num?)?.toInt() ?? 0;
                final label =
                    bookmark['name']?.toString() ?? _format(positionMs);
                return InputChip(
                  label: Text(label),
                  onPressed: () =>
                      _run((backend) => backend.replaySeek(positionMs)),
                  onDeleted: () => _run(
                    (backend) => backend.replayRemoveBookmark(positionMs),
                  ),
                );
              }).toList(),
            ),
          ],
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(
                _error!,
                style: const TextStyle(color: AppTheme.error),
              ),
            ),
        ],
      ),
    );
  }

  String _format(int milliseconds) {
    final seconds = milliseconds ~/ 1000;
    return '${(seconds ~/ 60).toString().padLeft(2, '0')}:${(seconds % 60).toString().padLeft(2, '0')}';
  }
}
