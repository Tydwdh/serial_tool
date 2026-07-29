import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../providers/backend_provider.dart';
import '../providers/dock_layout_provider.dart';
import '../theme/app_theme.dart';

class SettingsPanel extends ConsumerStatefulWidget {
  const SettingsPanel({super.key});

  @override
  ConsumerState<SettingsPanel> createState() => _SettingsPanelState();
}

class _SettingsPanelState extends ConsumerState<SettingsPanel> {
  String _baudRate = '115200';
  String _dataBits = '8';
  String _stopBits = '1';
  String _parity = 'none';
  bool _autoReconnect = false;
  String? _message;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  void _load() {
    try {
      final config = ref.read(backendServiceProvider).getConfig();
      final serial = Map<String, dynamic>.from(config['serial'] as Map? ?? {});
      if (!mounted) return;
      setState(() {
        _baudRate = serial['baud_rate'] as String? ?? _baudRate;
        _dataBits = serial['data_bits'] as String? ?? _dataBits;
        _stopBits = serial['stop_bits'] as String? ?? _stopBits;
        _parity = serial['parity'] as String? ?? _parity;
        _autoReconnect = serial['auto_reconnect'] as bool? ?? _autoReconnect;
        _message = null;
      });
    } catch (error) {
      if (mounted) setState(() => _message = error.toString());
    }
  }

  void _save() {
    try {
      final backend = ref.read(backendServiceProvider);
      backend.setSerialConfig({
        'baud_rate': _baudRate,
        'data_bits': _dataBits,
        'stop_bits': _stopBits,
        'parity': _parity,
        'auto_reconnect': _autoReconnect,
      });
      backend.saveConfig();
      setState(() => _message = '配置已保存；重新打开串口后生效。');
    } catch (error) {
      setState(() => _message = error.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppTheme.bgDark,
      child: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Text('串口默认配置', style: TextStyle(fontSize: 14)),
          const SizedBox(height: 12),
          _dropdown('波特率', _baudRate, const [
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
          ], (value) => setState(() => _baudRate = value)),
          _dropdown('数据位', _dataBits, const [
            '5',
            '6',
            '7',
            '8',
          ], (value) => setState(() => _dataBits = value)),
          _dropdown('停止位', _stopBits, const [
            '1',
            '2',
          ], (value) => setState(() => _stopBits = value)),
          _dropdown('校验位', _parity, const [
            'none',
            'odd',
            'even',
          ], (value) => setState(() => _parity = value)),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('自动重连', style: TextStyle(fontSize: 12)),
            value: _autoReconnect,
            onChanged: (value) => setState(() => _autoReconnect = value),
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 4,
            children: [
              FilledButton.icon(
                onPressed: _save,
                icon: const Icon(Icons.save, size: 16),
                label: const Text('保存配置'),
              ),
              TextButton.icon(
                onPressed: _load,
                icon: const Icon(Icons.refresh, size: 16),
                label: const Text('重新加载'),
              ),
            ],
          ),
          if (_message != null) ...[
            const SizedBox(height: 12),
            Text(
              _message!,
              style: const TextStyle(color: AppTheme.textSecondary),
            ),
          ],
          const SizedBox(height: 28),
          const Divider(),
          const SizedBox(height: 16),
          const Text('工作区', style: TextStyle(fontSize: 14)),
          const SizedBox(height: 6),
          const Text(
            '面板位置、尺寸和打开状态会自动保存到当前用户的应用配置中。',
            style: TextStyle(fontSize: 12, color: AppTheme.textSecondary),
          ),
          const SizedBox(height: 10),
          OutlinedButton.icon(
            onPressed: () {
              ref.read(dockLayoutProvider.notifier).resetLayout();
              setState(() => _message = '工作区布局已恢复为默认值。');
            },
            icon: const Icon(Icons.restart_alt, size: 16),
            label: const Text('恢复默认布局'),
          ),
          const SizedBox(height: 28),
          const Divider(),
          const SizedBox(height: 16),
          const Text('发送器', style: TextStyle(fontSize: 14)),
          const SizedBox(height: 6),
          const Text(
            '发送历史、HEX 模式、行尾和周期发送间隔会在发送器面板中自动保存。',
            style: TextStyle(fontSize: 12, color: AppTheme.textSecondary),
          ),
          const SizedBox(height: 28),
          const Divider(),
          const SizedBox(height: 16),
          const Text('配置文件', style: TextStyle(fontSize: 14)),
          const SizedBox(height: 6),
          const SelectableText(
            r'%APPDATA%\hardware_workbench\config.json',
            style: TextStyle(
              fontSize: 12,
              color: AppTheme.textSecondary,
              fontFamily: 'monospace',
            ),
          ),
        ],
      ),
    );
  }

  Widget _dropdown(
    String label,
    String value,
    List<String> values,
    ValueChanged<String> onChanged,
  ) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final compact = constraints.maxWidth < 320;
          final dropdown = DropdownButtonFormField<String>(
            key: ValueKey(value),
            initialValue: value,
            isDense: true,
            items: values
                .map((item) => DropdownMenuItem(value: item, child: Text(item)))
                .toList(),
            onChanged: (next) {
              if (next != null) onChanged(next);
            },
          );
          if (compact) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label, style: const TextStyle(fontSize: 12)),
                const SizedBox(height: 4),
                SizedBox(width: double.infinity, child: dropdown),
              ],
            );
          }
          return Row(
            children: [
              SizedBox(
                width: 100,
                child: Text(label, style: const TextStyle(fontSize: 12)),
              ),
              Expanded(child: dropdown),
            ],
          );
        },
      ),
    );
  }
}
