import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/models.dart';
import '../providers/backend_provider.dart';
import '../theme/app_theme.dart';

/// 系统日志面板，日志数据由后端事件流提供。
class LogPanel extends ConsumerStatefulWidget {
  const LogPanel({super.key});

  @override
  ConsumerState<LogPanel> createState() => _LogPanelState();
}

class _LogPanelState extends ConsumerState<LogPanel> {
  final _searchController = TextEditingController();
  final _enabled = <LogLevel>{...LogLevel.values};
  String _query = '';

  @override
  void dispose() {
    _searchController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final entries = ref.watch(logEntriesProvider);
    final visible = entries.where(_matches).toList(growable: false);
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          _buildToolbar(visible.length),
          Expanded(
            child: visible.isEmpty
                ? const Center(
                    child: Text(
                      '暂无日志',
                      style: TextStyle(
                        color: AppTheme.textSecondary,
                        fontSize: 12,
                      ),
                    ),
                  )
                : ListView.builder(
                    itemCount: visible.length,
                    itemBuilder: (_, index) => _buildRow(visible[index]),
                  ),
          ),
        ],
      ),
    );
  }

  bool _matches(LogEntry entry) {
    if (!_enabled.contains(entry.level)) return false;
    if (_query.isEmpty) return true;
    final haystack = '${entry.source} ${entry.message}'.toLowerCase();
    return haystack.contains(_query.toLowerCase());
  }

  Widget _buildToolbar(int visibleCount) {
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
            Text(
              '日志 ($visibleCount)',
              style: const TextStyle(
                fontSize: 11,
                color: AppTheme.textSecondary,
              ),
            ),
            const SizedBox(width: 8),
            SizedBox(
              width: 180,
              child: TextField(
                controller: _searchController,
                onChanged: (value) => setState(() => _query = value),
                style: const TextStyle(fontSize: 11),
                decoration: const InputDecoration(
                  hintText: '筛选日志',
                  prefixIcon: Icon(Icons.search, size: 14),
                  contentPadding: EdgeInsets.symmetric(vertical: 4),
                ),
              ),
            ),
            const SizedBox(width: 16),
            for (final level in [LogLevel.info, LogLevel.warn, LogLevel.error])
              Padding(
                padding: const EdgeInsets.only(left: 4),
                child: FilterChip(
                  label: Text(
                    level.name.toUpperCase(),
                    style: const TextStyle(fontSize: 10),
                  ),
                  selected: _enabled.contains(level),
                  onSelected: (selected) => setState(() {
                    if (selected) {
                      _enabled.add(level);
                    } else {
                      _enabled.remove(level);
                    }
                  }),
                  visualDensity: VisualDensity.compact,
                ),
              ),
            IconButton(
              icon: const Icon(Icons.delete_outline, size: 14),
              color: AppTheme.textSecondary,
              onPressed: () => ref.read(logEntriesProvider.notifier).clear(),
              tooltip: '清空',
              style: IconButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: const Size(28, 28),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildRow(LogEntry entry) {
    final color = switch (entry.level) {
      LogLevel.error => AppTheme.error,
      LogLevel.warn => AppTheme.warning,
      LogLevel.debug || LogLevel.trace => AppTheme.textSecondary,
      LogLevel.info => AppTheme.textPrimary,
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 52,
            child: Text(
              entry.level.name.toUpperCase(),
              style: TextStyle(
                color: color,
                fontSize: 10,
                fontFamily: 'monospace',
                fontWeight: FontWeight.bold,
              ),
            ),
          ),
          SizedBox(
            width: 90,
            child: Text(
              entry.source,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: AppTheme.textSecondary,
                fontSize: 11,
                fontFamily: 'monospace',
              ),
            ),
          ),
          Expanded(
            child: Text(
              entry.message,
              style: TextStyle(
                color: color,
                fontSize: 11,
                fontFamily: 'monospace',
              ),
            ),
          ),
        ],
      ),
    );
  }
}
