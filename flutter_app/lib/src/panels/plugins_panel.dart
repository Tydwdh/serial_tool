import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/models.dart';
import '../providers/backend_provider.dart';
import '../theme/app_theme.dart';

class PluginsPanel extends ConsumerStatefulWidget {
  const PluginsPanel({super.key});

  @override
  ConsumerState<PluginsPanel> createState() => _PluginsPanelState();
}

class _PluginsPanelState extends ConsumerState<PluginsPanel> {
  List<Map<String, dynamic>> _market = const [];
  bool _marketLoading = false;
  String? _busyId;
  String? _marketError;

  Future<void> _refreshMarket() async {
    setState(() {
      _marketLoading = true;
      _marketError = null;
    });
    await Future<void>.delayed(Duration.zero);
    try {
      final entries = ref.read(backendServiceProvider).fetchMarketplace();
      if (mounted) setState(() => _market = entries);
    } catch (error) {
      if (mounted) setState(() => _marketError = error.toString());
    } finally {
      if (mounted) setState(() => _marketLoading = false);
    }
  }

  Future<void> _install(Map<String, dynamic> plugin) async {
    final id = plugin['id']?.toString() ?? '';
    if (id.isEmpty) return;
    setState(() => _busyId = id);
    await Future<void>.delayed(Duration.zero);
    try {
      ref.read(backendServiceProvider).installMarketplacePlugin(plugin);
      ref.invalidate(pluginListProvider);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('插件 ${plugin['name'] ?? id} 已安装')),
        );
      }
    } catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('$error')));
      }
    } finally {
      if (mounted) setState(() => _busyId = null);
    }
  }

  Future<void> _uninstall(PluginSummary plugin) async {
    final accepted = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('卸载插件？'),
        content: Text('将从本机移除“${plugin.name}”。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            style: FilledButton.styleFrom(backgroundColor: AppTheme.error),
            child: const Text('卸载'),
          ),
        ],
      ),
    );
    if (accepted != true) return;
    setState(() => _busyId = plugin.id);
    await Future<void>.delayed(Duration.zero);
    try {
      ref.read(backendServiceProvider).uninstallMarketplacePlugin(plugin.id);
      ref.invalidate(pluginListProvider);
    } catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('$error')));
      }
    } finally {
      if (mounted) setState(() => _busyId = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final plugins = ref.watch(pluginListProvider);
    return Container(
      color: AppTheme.bgDark,
      child: Column(
        children: [
          _toolbar(),
          Expanded(
            child: plugins.when(
              loading: () => const Center(child: CircularProgressIndicator()),
              error: (error, _) => Center(child: Text('插件加载失败: $error')),
              data: (installed) => ListView(
                padding: const EdgeInsets.all(12),
                children: [
                  if (installed.isEmpty)
                    const _EmptyState(
                      icon: Icons.extension_off_outlined,
                      text: '未发现已安装插件',
                    )
                  else ...[
                    const _SectionTitle('已安装'),
                    const SizedBox(height: 8),
                    ...installed.map(
                      (plugin) => Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: _InstalledCard(
                          plugin: plugin,
                          busy: _busyId == plugin.id,
                          onUninstall: () => _uninstall(plugin),
                        ),
                      ),
                    ),
                  ],
                  if (_marketError != null) ...[
                    const SizedBox(height: 16),
                    Text(
                      _marketError!,
                      style: const TextStyle(color: AppTheme.error),
                    ),
                  ],
                  if (_marketLoading) ...[
                    const SizedBox(height: 20),
                    const Center(child: CircularProgressIndicator()),
                  ],
                  if (_market.isNotEmpty) ...[
                    const SizedBox(height: 18),
                    const _SectionTitle('插件市场'),
                    const SizedBox(height: 8),
                    ..._market.map(
                      (plugin) => Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: _MarketCard(
                          plugin: plugin,
                          installed: installed.any(
                            (item) => item.id == plugin['id'],
                          ),
                          busy: _busyId == plugin['id'],
                          onInstall: () => _install(plugin),
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _toolbar() => Container(
    height: 44,
    padding: const EdgeInsets.symmetric(horizontal: 12),
    decoration: const BoxDecoration(
      color: AppTheme.bgPanel,
      border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
    ),
    child: LayoutBuilder(
      builder: (context, constraints) => Row(
        children: [
          const Icon(
            Icons.extension_rounded,
            color: AppTheme.accentLight,
            size: 19,
          ),
          if (constraints.maxWidth >= 250) ...[
            const SizedBox(width: 8),
            const Text('插件', style: TextStyle(fontWeight: FontWeight.w600)),
          ],
          const Spacer(),
          if (constraints.maxWidth >= 250)
            OutlinedButton.icon(
              onPressed: _marketLoading ? null : _refreshMarket,
              icon: const Icon(Icons.storefront_outlined, size: 16),
              label: Text(_market.isEmpty ? '浏览市场' : '刷新市场'),
            )
          else
            IconButton(
              tooltip: _market.isEmpty ? '浏览市场' : '刷新市场',
              onPressed: _marketLoading ? null : _refreshMarket,
              icon: const Icon(Icons.storefront_outlined, size: 18),
            ),
        ],
      ),
    ),
  );
}

class _SectionTitle extends StatelessWidget {
  const _SectionTitle(this.text);
  final String text;
  @override
  Widget build(BuildContext context) => Text(
    text.toUpperCase(),
    style: const TextStyle(
      fontSize: 11,
      letterSpacing: 0.8,
      color: AppTheme.textSecondary,
      fontWeight: FontWeight.w600,
    ),
  );
}

class _InstalledCard extends ConsumerWidget {
  const _InstalledCard({
    required this.plugin,
    required this.busy,
    required this.onUninstall,
  });
  final PluginSummary plugin;
  final bool busy;
  final VoidCallback onUninstall;

  @override
  Widget build(BuildContext context, WidgetRef ref) => Card(
    child: Padding(
      padding: const EdgeInsets.all(12),
      child: Row(
        children: [
          Icon(
            plugin.enabled ? Icons.extension : Icons.extension_outlined,
            color: plugin.enabled ? AppTheme.success : AppTheme.textSecondary,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  plugin.name,
                  style: const TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                Text(
                  '${plugin.id} · v${plugin.version}',
                  style: const TextStyle(
                    fontSize: 11,
                    color: AppTheme.textSecondary,
                  ),
                ),
                if (plugin.description?.isNotEmpty == true)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      plugin.description!,
                      style: const TextStyle(
                        fontSize: 12,
                        color: AppTheme.textSecondary,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          if (busy)
            const SizedBox(
              width: 26,
              height: 26,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else ...[
            Switch(
              value: plugin.enabled,
              onChanged: (enabled) {
                try {
                  final backend = ref.read(backendServiceProvider);
                  enabled
                      ? backend.enablePlugin(plugin.id)
                      : backend.disablePlugin(plugin.id);
                } catch (error) {
                  ScaffoldMessenger.of(
                    context,
                  ).showSnackBar(SnackBar(content: Text('$error')));
                }
              },
            ),
            IconButton(
              tooltip: '卸载插件',
              onPressed: onUninstall,
              icon: const Icon(Icons.delete_outline, size: 18),
              color: AppTheme.textSecondary,
            ),
          ],
        ],
      ),
    ),
  );
}

class _MarketCard extends StatelessWidget {
  const _MarketCard({
    required this.plugin,
    required this.installed,
    required this.busy,
    required this.onInstall,
  });
  final Map<String, dynamic> plugin;
  final bool installed;
  final bool busy;
  final VoidCallback onInstall;

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(12),
      child: Row(
        children: [
          const Icon(Icons.auto_awesome_outlined, color: AppTheme.accentLight),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  plugin['name']?.toString() ?? '未命名插件',
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                Text(
                  'v${plugin['version'] ?? '?'} · ${plugin['category'] ?? '工具'}',
                  style: const TextStyle(
                    fontSize: 11,
                    color: AppTheme.textSecondary,
                  ),
                ),
                if (plugin['description']?.toString().isNotEmpty == true)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      plugin['description'].toString(),
                      style: const TextStyle(
                        fontSize: 12,
                        color: AppTheme.textSecondary,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          if (busy)
            const SizedBox(
              width: 26,
              height: 26,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else
            FilledButton(
              onPressed: onInstall,
              child: Text(installed ? '更新' : '安装'),
            ),
        ],
      ),
    ),
  );
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({required this.icon, required this.text});
  final IconData icon;
  final String text;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 48),
    child: Center(
      child: Column(
        children: [
          Icon(icon, size: 40, color: AppTheme.textSecondary),
          const SizedBox(height: 10),
          Text(text, style: const TextStyle(color: AppTheme.textSecondary)),
        ],
      ),
    ),
  );
}
