import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../providers/backend_provider.dart';
import '../providers/dynamic_panels_provider.dart';
import '../theme/app_theme.dart';

class DynamicPanelsView extends ConsumerWidget {
  const DynamicPanelsView({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final panels = ref.watch(dynamicPanelsProvider);
    if (panels.isEmpty) {
      return const Center(
        child: Text(
          '等待插件创建面板…',
          style: TextStyle(color: AppTheme.textSecondary),
        ),
      );
    }
    return GridView.builder(
      padding: const EdgeInsets.all(12),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 460,
        mainAxisExtent: 250,
        crossAxisSpacing: 12,
        mainAxisSpacing: 12,
      ),
      itemCount: panels.length,
      itemBuilder: (_, index) => _DynamicCard(panel: panels[index]),
    );
  }
}

class _DynamicCard extends ConsumerWidget {
  const _DynamicCard({required this.panel});
  final DynamicPanelSpec panel;

  @override
  Widget build(BuildContext context, WidgetRef ref) => Card(
    clipBehavior: Clip.antiAlias,
    child: Padding(
      padding: const EdgeInsets.all(14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(_iconFor(panel.kind), size: 17, color: AppTheme.accentLight),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  panel.title,
                  style: const TextStyle(
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (panel.values['_topic'] != null)
                const Tooltip(
                  message: '正在接收协议数据',
                  child: Icon(Icons.sensors, size: 15, color: AppTheme.success),
                ),
            ],
          ),
          const SizedBox(height: 12),
          Expanded(
            child: switch (panel.kind.toLowerCase()) {
              'gauge' => _gauge(),
              'form' => _form(context, ref),
              'attitude' || 'attitude3d' => _attitude(),
              _ => _chart(),
            },
          ),
        ],
      ),
    ),
  );

  IconData _iconFor(String kind) => switch (kind.toLowerCase()) {
    'gauge' => Icons.speed,
    'form' => Icons.tune,
    'attitude' || 'attitude3d' => Icons.explore,
    _ => Icons.query_stats,
  };

  num _number(dynamic value, [num fallback = 0]) =>
      value is num ? value : fallback;

  Widget _gauge() {
    final value = _number(panel.values['value'] ?? panel.config['value']);
    final min = _number(panel.config['min']);
    final max = _number(panel.config['max'], 100);
    final fraction = ((value - min) / (max - min == 0 ? 1 : max - min))
        .clamp(0, 1)
        .toDouble();
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          SizedBox(
            width: 116,
            height: 116,
            child: Stack(
              alignment: Alignment.center,
              children: [
                CircularProgressIndicator(
                  value: fraction,
                  strokeWidth: 10,
                  color: fraction > .85 ? AppTheme.warning : AppTheme.accent,
                  backgroundColor: AppTheme.bgInput,
                ),
                Text(
                  _format(value),
                  style: const TextStyle(fontSize: 22, fontFamily: 'monospace'),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
          Text(
            '${panel.config['unit'] ?? ''}  ${_format(min)} – ${_format(max)}',
            style: const TextStyle(color: AppTheme.textSecondary),
          ),
        ],
      ),
    );
  }

  Widget _chart() {
    final field = panel.config['field']?.toString() ?? _firstNumericField();
    if (field == null || panel.samples.isEmpty) {
      return const Center(
        child: Text(
          '等待匹配 topic 的数值数据',
          style: TextStyle(color: AppTheme.textSecondary),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '$field: ${_format(panel.values[field])}',
          style: const TextStyle(
            fontFamily: 'monospace',
            color: AppTheme.textBright,
          ),
        ),
        const SizedBox(height: 6),
        Expanded(
          child: CustomPaint(
            painter: _LineChartPainter(panel.samples, field),
            child: const SizedBox.expand(),
          ),
        ),
      ],
    );
  }

  String? _firstNumericField() {
    for (final sample in panel.samples.reversed) {
      if (sample.values.isNotEmpty) return sample.values.keys.first;
    }
    return null;
  }

  Widget _attitude() {
    final roll = _number(
      panel.values['roll'] ?? panel.config['roll'],
    ).toDouble();
    final pitch = _number(
      panel.values['pitch'] ?? panel.config['pitch'],
    ).toDouble();
    final yaw = _number(panel.values['yaw'] ?? panel.config['yaw']).toDouble();
    return Row(
      children: [
        Expanded(
          child: CustomPaint(
            painter: _AttitudePainter(roll: roll, pitch: pitch),
            child: const SizedBox.expand(),
          ),
        ),
        SizedBox(
          width: 82,
          child: DefaultTextStyle(
            style: const TextStyle(
              fontFamily: 'monospace',
              color: AppTheme.textSecondary,
            ),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('R ${_format(roll)}°'),
                const SizedBox(height: 6),
                Text('P ${_format(pitch)}°'),
                const SizedBox(height: 6),
                Text('Y ${_format(yaw)}°'),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Widget _form(BuildContext context, WidgetRef ref) {
    final fields = (panel.config['fields'] as List? ?? const [])
        .whereType<Map>()
        .map(Map<String, dynamic>.from)
        .where((field) => field['visible'] != false)
        .toList(growable: false);
    if (fields.isEmpty) {
      return const Center(child: Text('此表单没有字段'));
    }
    final autoApply = panel.config['auto_apply'] == true;
    return Column(
      children: [
        Expanded(
          child: ListView.separated(
            itemCount: fields.length,
            separatorBuilder: (_, _) => const SizedBox(height: 8),
            itemBuilder: (_, index) =>
                _formField(context, ref, fields[index], autoApply),
          ),
        ),
        if (!autoApply) ...[
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerRight,
            child: FilledButton.icon(
              onPressed: () => _submit(context, ref),
              icon: const Icon(Icons.check, size: 16),
              label: const Text('应用'),
            ),
          ),
        ],
      ],
    );
  }

  Widget _formField(
    BuildContext context,
    WidgetRef ref,
    Map<String, dynamic> field,
    bool autoApply,
  ) {
    final id = field['id']?.toString() ?? '';
    final label = field['label']?.toString() ?? id;
    final value = panel.values[id] ?? field['value'] ?? field['default'];
    final enabled = field['enabled'] != false;
    void changed(dynamic next) {
      ref.read(dynamicPanelsProvider.notifier).setFormValue(panel.id, id, next);
      if (autoApply) _submit(context, ref, override: {id: next});
    }

    switch (field['kind']?.toString().toLowerCase()) {
      case 'separator':
        return const Divider();
      case 'label':
        return Text(field['text']?.toString() ?? label);
      case 'progress':
        final progress = value is num
            ? value.toDouble()
            : ((value as Map?)?['current'] as num?)?.toDouble() ?? 0;
        final total = value is Map
            ? ((value['total'] as num?)?.toDouble() ?? 100)
            : 100.0;
        final fraction = (progress / (total == 0 ? 1 : total))
            .clamp(0, 1)
            .toDouble();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(label, style: const TextStyle(color: AppTheme.textSecondary)),
            const SizedBox(height: 5),
            LinearProgressIndicator(value: fraction, minHeight: 8),
            const SizedBox(height: 3),
            Text('${_format(progress)} / ${_format(total)}'),
          ],
        );
      case 'status':
        final status = value is Map ? value : const <String, dynamic>{};
        final level = status['level']?.toString() ?? 'idle';
        final color = switch (level) {
          'success' => AppTheme.success,
          'warn' || 'warning' => AppTheme.warning,
          'error' => AppTheme.error,
          'running' => AppTheme.accentLight,
          _ => AppTheme.textSecondary,
        };
        final text = value is String
            ? value
            : status['text']?.toString() ?? label;
        return Row(
          children: [
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(text, style: TextStyle(color: color)),
            ),
          ],
        );
      case 'button':
        return Align(
          alignment: Alignment.centerLeft,
          child: FilledButton(
            onPressed: !enabled
                ? null
                : () => _triggerAction(
                    context,
                    ref,
                    id,
                    field['action']?.toString(),
                  ),
            child: Text(field['text']?.toString() ?? label),
          ),
        );
      case 'file':
        final path = value?.toString() ?? '';
        return Row(
          children: [
            SizedBox(
              width: 92,
              child: Text(label, overflow: TextOverflow.ellipsis),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                path.isEmpty ? '未选择文件' : path,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(color: AppTheme.textSecondary),
              ),
            ),
            const SizedBox(width: 8),
            OutlinedButton.icon(
              onPressed: !enabled
                  ? null
                  : () => _pickFile(context, ref, id, label, field['filters']),
              icon: const Icon(Icons.folder_open_outlined, size: 16),
              label: const Text('浏览'),
            ),
          ],
        );
      case 'textarea':
        return TextFormField(
          key: ValueKey('${panel.id}-$id-$value'),
          initialValue: '$value',
          enabled: enabled,
          minLines: (field['rows'] as num?)?.toInt() ?? 3,
          maxLines: (field['rows'] as num?)?.toInt() ?? 6,
          decoration: InputDecoration(labelText: label),
          onChanged: changed,
          onFieldSubmitted: (_) => _submit(context, ref),
        );
      case 'slider':
      case 'range':
        final min = (field['min'] as num?)?.toDouble() ?? 0;
        final max = (field['max'] as num?)?.toDouble() ?? 100;
        final current = (value is num ? value.toDouble() : min)
            .clamp(min, max)
            .toDouble();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('$label  ${_format(current)}'),
            Slider(
              value: current,
              min: min,
              max: max == min ? min + 1 : max,
              divisions: _sliderDivisions(min, max, field['step']),
              onChanged: enabled ? changed : null,
            ),
          ],
        );
      case 'boolean':
      case 'bool':
      case 'checkbox':
        return SwitchListTile(
          dense: true,
          contentPadding: EdgeInsets.zero,
          title: Text(label),
          value: value == true,
          onChanged: enabled ? changed : null,
        );
      case 'select':
      case 'choice':
      case 'enum':
      case 'dropdown':
        final options = (field['options'] as List? ?? const [])
            .map(
              (option) => option is Map
                  ? DropdownMenuItem<dynamic>(
                      value: option['value'],
                      child: Text('${option['label'] ?? option['value']}'),
                    )
                  : DropdownMenuItem<dynamic>(
                      value: option,
                      child: Text('$option'),
                    ),
            )
            .toList(growable: false);
        return DropdownButtonFormField<dynamic>(
          key: ValueKey('${panel.id}-$id-$value'),
          initialValue: options.any((option) => option.value == value)
              ? value
              : null,
          isExpanded: true,
          decoration: InputDecoration(labelText: label),
          items: options,
          onChanged: enabled
              ? (next) {
                  if (next != null) changed(next);
                }
              : null,
        );
      case 'serial':
      case 'serial_port':
      case 'comport':
        final ports = ref.watch(portListProvider).valueOrNull ?? const [];
        final selectedPort = value?.toString();
        return DropdownButtonFormField<String>(
          key: ValueKey('${panel.id}-$id-$selectedPort'),
          initialValue: ports.any((port) => port.portName == selectedPort)
              ? selectedPort
              : null,
          isExpanded: true,
          decoration: InputDecoration(labelText: label),
          hint: const Text('选择串口'),
          items: ports
              .map(
                (port) => DropdownMenuItem(
                  value: port.portName,
                  child: Text('${port.portName}  ${port.portType}'),
                ),
              )
              .toList(growable: false),
          onChanged: enabled
              ? (next) {
                  if (next != null) changed(next);
                }
              : null,
        );
      default:
        final numeric = field['kind']?.toString().toLowerCase() == 'number';
        return TextFormField(
          key: ValueKey('${panel.id}-$id-$value'),
          initialValue: '$value',
          enabled: enabled,
          keyboardType: numeric
              ? const TextInputType.numberWithOptions(
                  decimal: true,
                  signed: true,
                )
              : null,
          decoration: InputDecoration(labelText: label),
          onChanged: numeric
              ? (text) => changed(num.tryParse(text) ?? text)
              : changed,
          onFieldSubmitted: (_) => _submit(context, ref),
        );
    }
  }

  int? _sliderDivisions(double min, double max, dynamic rawStep) {
    final step = rawStep is num ? rawStep.toDouble() : 0;
    if (step <= 0 || max <= min) return null;
    final divisions = ((max - min) / step).round();
    return divisions >= 1 && divisions <= 1000 ? divisions : null;
  }

  void _triggerAction(
    BuildContext context,
    WidgetRef ref,
    String fieldId,
    String? action,
  ) {
    try {
      ref
          .read(backendServiceProvider)
          .triggerDynamicFormAction(
            panelId: panel.id,
            fieldId: fieldId,
            action: action,
            values: panel.values,
          );
    } catch (error) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('$error')));
    }
  }

  void _pickFile(
    BuildContext context,
    WidgetRef ref,
    String fieldId,
    String title,
    dynamic rawFilters,
  ) {
    try {
      final path = ref
          .read(backendServiceProvider)
          .pickDynamicFormFile(
            pluginId: panel.pluginId,
            title: title,
            filters: rawFilters is List ? rawFilters : const [],
          );
      if (path == null) return;
      ref
          .read(dynamicPanelsProvider.notifier)
          .setFormValue(panel.id, fieldId, path);
      if (panel.config['auto_apply'] == true) {
        _submit(context, ref, override: {fieldId: path});
      }
    } catch (error) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('$error')));
    }
  }

  void _submit(
    BuildContext context,
    WidgetRef ref, {
    Map<String, dynamic>? override,
  }) {
    final values = {...panel.values, ...?override};
    try {
      ref.read(backendServiceProvider).submitDynamicForm(panel.id, values);
    } catch (error) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('$error')));
    }
  }

  String _format(dynamic value) {
    if (value is num) {
      return value % 1 == 0
          ? value.toStringAsFixed(0)
          : value.toStringAsFixed(2);
    }
    return value?.toString() ?? '0';
  }
}

class _LineChartPainter extends CustomPainter {
  const _LineChartPainter(this.samples, this.field);
  final List<DynamicSample> samples;
  final String field;

  @override
  void paint(Canvas canvas, Size size) {
    const padding = 8.0;
    final points = samples
        .where((sample) => sample.values[field] != null)
        .map((sample) => sample.values[field]!.toDouble())
        .toList(growable: false);
    if (points.isEmpty || size.isEmpty) return;
    final min = points.reduce(math.min);
    final max = points.reduce(math.max);
    final span = max - min == 0 ? 1.0 : max - min;
    final grid = Paint()
      ..color = AppTheme.borderColor
      ..strokeWidth = 1;
    for (var index = 1; index < 4; index++) {
      final y = padding + (size.height - 2 * padding) * index / 4;
      canvas.drawLine(
        Offset(padding, y),
        Offset(size.width - padding, y),
        grid,
      );
    }
    final path = Path();
    for (var index = 0; index < points.length; index++) {
      final x =
          padding +
          (size.width - 2 * padding) * index / math.max(1, points.length - 1);
      final y =
          size.height -
          padding -
          (points[index] - min) / span * (size.height - 2 * padding);
      index == 0 ? path.moveTo(x, y) : path.lineTo(x, y);
    }
    canvas.drawPath(
      path,
      Paint()
        ..color = AppTheme.accentLight
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(covariant _LineChartPainter oldDelegate) =>
      !identical(samples, oldDelegate.samples) || field != oldDelegate.field;
}

class _AttitudePainter extends CustomPainter {
  const _AttitudePainter({required this.roll, required this.pitch});
  final double roll;
  final double pitch;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = math.min(size.width, size.height) / 2 - 10;
    if (radius <= 0) return;
    canvas.save();
    canvas.clipPath(
      Path()..addOval(Rect.fromCircle(center: center, radius: radius)),
    );
    canvas.translate(center.dx, center.dy + pitch.clamp(-45, 45) * radius / 45);
    canvas.rotate(roll * math.pi / 180);
    canvas.drawRect(
      Rect.fromCenter(
        center: Offset.zero,
        width: radius * 3,
        height: radius * 3,
      ),
      Paint()..color = const Color(0xFF315E92),
    );
    canvas.drawRect(
      Rect.fromLTWH(-radius * 1.5, 0, radius * 3, radius * 1.5),
      Paint()..color = const Color(0xFF57422A),
    );
    canvas.drawLine(
      Offset(-radius * 1.5, 0),
      Offset(radius * 1.5, 0),
      Paint()
        ..color = Colors.white
        ..strokeWidth = 2,
    );
    canvas.restore();
    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..color = AppTheme.borderColor,
    );
    final reticle = Paint()
      ..color = Colors.white
      ..strokeWidth = 2;
    canvas.drawLine(center + Offset(-24, 0), center + Offset(-7, 0), reticle);
    canvas.drawLine(center + Offset(7, 0), center + Offset(24, 0), reticle);
    canvas.drawLine(
      center + const Offset(0, -7),
      center + const Offset(0, 7),
      reticle,
    );
  }

  @override
  bool shouldRepaint(covariant _AttitudePainter oldDelegate) =>
      roll != oldDelegate.roll || pitch != oldDelegate.pitch;
}
