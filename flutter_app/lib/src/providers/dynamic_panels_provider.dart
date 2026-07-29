import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/models.dart';
import 'backend_provider.dart';

class DynamicSample {
  const DynamicSample(this.timestamp, this.values);
  final int timestamp;
  final Map<String, num> values;
}

class DynamicPanelSpec {
  final String id;
  final String pluginId;
  final String title;
  final String kind;
  final Map<String, dynamic> config;
  final Map<String, dynamic> values;
  final List<DynamicSample> samples;

  const DynamicPanelSpec({
    required this.id,
    required this.pluginId,
    required this.title,
    required this.kind,
    required this.config,
    this.values = const {},
    this.samples = const [],
  });

  DynamicPanelSpec copyWith({
    Map<String, dynamic>? config,
    Map<String, dynamic>? values,
    List<DynamicSample>? samples,
  }) => DynamicPanelSpec(
    id: id,
    pluginId: pluginId,
    title: title,
    kind: kind,
    config: config ?? this.config,
    values: values ?? this.values,
    samples: samples ?? this.samples,
  );
}

final dynamicPanelsProvider =
    StateNotifierProvider<DynamicPanelsNotifier, List<DynamicPanelSpec>>(
      (ref) => DynamicPanelsNotifier(ref),
    );

class DynamicPanelsNotifier extends StateNotifier<List<DynamicPanelSpec>> {
  DynamicPanelsNotifier(this._ref) : super(const []) {
    _ref.listen(
      backendEventStreamProvider,
      (_, next) => next.whenData(_ingest),
    );
  }
  final Ref _ref;
  static const _maxSamples = 300;

  void _ingest(BackendEvent event) {
    if (event.type == 'protocol_data') {
      _ingestProtocol(event);
      return;
    }
    if (event.type != 'plugin_event') return;
    final kind = event.data['kind'];
    final data = event.data['data'];
    if (kind == 'ui.panel.create' && data is Map) {
      final config = Map<String, dynamic>.from(data);
      final id = config['id']?.toString();
      if (id == null || id.isEmpty) return;
      final spec = DynamicPanelSpec(
        id: id,
        pluginId: event.data['plugin_id']?.toString() ?? '',
        title: config['title']?.toString() ?? id,
        kind: config['kind']?.toString() ?? 'chart',
        config: config,
      );
      state = [...state.where((panel) => panel.id != id), spec];
    } else if (kind == 'ui.panel.remove' && data is Map) {
      final id = data['id']?.toString();
      if (id != null) {
        state = state.where((panel) => panel.id != id).toList(growable: false);
      }
    } else if (kind == 'ui.form.set_value' && data is Map) {
      _updateField(data, (field, value) => value);
    } else if (kind == 'ui.form.set_enabled' && data is Map) {
      _updateField(
        data,
        (field, value) => value..['enabled'] = data['enabled'],
      );
    } else if (kind == 'ui.form.set_visible' && data is Map) {
      _updateField(
        data,
        (field, value) => value..['visible'] = data['visible'],
      );
    }
  }

  void _updateField(
    Map data,
    Map<String, dynamic> Function(
      Map<String, dynamic> field,
      Map<String, dynamic> value,
    )
    transform,
  ) {
    final id = data['panel_id']?.toString();
    final fieldId = data['field_id']?.toString();
    if (id == null || fieldId == null) return;
    state = state
        .map((panel) {
          if (panel.id != id) return panel;
          final fields = (panel.config['fields'] as List? ?? const [])
              .whereType<Map>()
              .map((item) {
                final field = Map<String, dynamic>.from(item);
                if (field['id']?.toString() != fieldId) return field;
                return transform(field, Map<String, dynamic>.from(field));
              })
              .toList(growable: false);
          final values = kindValue(data, panel.values, fieldId);
          return panel.copyWith(
            config: {...panel.config, 'fields': fields},
            values: values,
          );
        })
        .toList(growable: false);
  }

  Map<String, dynamic> kindValue(
    Map data,
    Map<String, dynamic> current,
    String fieldId,
  ) {
    if (!data.containsKey('value')) return current;
    return {...current, fieldId: data['value']};
  }

  void setFormValue(String panelId, String fieldId, dynamic value) {
    state = state
        .map((panel) {
          if (panel.id != panelId) return panel;
          return panel.copyWith(values: {...panel.values, fieldId: value});
        })
        .toList(growable: false);
  }

  void _ingestProtocol(BackendEvent event) {
    final topic = event.data['topic']?.toString();
    final rawData = event.data['data'];
    if (topic == null || rawData is! Map) return;
    final values = Map<String, dynamic>.from(rawData);
    final timestamp =
        (event.data['timestamp'] as num?)?.toInt() ??
        DateTime.now().millisecondsSinceEpoch;
    state = state
        .map((panel) {
          if (!_matchesTopic(panel.config, topic)) return panel;
          final updatedValues = {...panel.values, ...values, '_topic': topic};
          final numeric = <String, num>{};
          for (final entry in values.entries) {
            if (entry.value is num) numeric[entry.key] = entry.value as num;
          }
          if (numeric.isEmpty) return panel.copyWith(values: updatedValues);
          final samples = [...panel.samples, DynamicSample(timestamp, numeric)];
          final bounded = samples.length <= _maxSamples
              ? samples
              : samples.sublist(samples.length - _maxSamples);
          return panel.copyWith(values: updatedValues, samples: bounded);
        })
        .toList(growable: false);
  }

  bool _matchesTopic(Map<String, dynamic> config, String topic) {
    final exact = config['topic']?.toString();
    if (exact != null && exact.isNotEmpty) return exact == topic;
    final prefix = config['topic_prefix']?.toString();
    if (prefix != null && prefix.isNotEmpty) return topic.startsWith(prefix);
    return false;
  }
}
