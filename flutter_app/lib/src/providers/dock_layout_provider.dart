import 'dart:async';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../panels/dock_layout.dart';
import 'backend_provider.dart';

/// Dock layout state. Changes are saved after a short debounce so resizing a
/// panel does not send hundreds of FFI calls.
final dockLayoutProvider =
    StateNotifierProvider<DockLayoutNotifier, DockLayout>((ref) {
      return DockLayoutNotifier(ref);
    });

class DockLayoutNotifier extends StateNotifier<DockLayout> {
  DockLayoutNotifier(this._ref) : super(DockLayout.defaultLayout()) {
    _restore();
    _ref.onDispose(() => _saveTimer?.cancel());
  }

  final Ref _ref;
  Timer? _saveTimer;
  bool _restored = false;

  Future<void> _restore() async {
    try {
      await _ref.read(backendInitializedProvider.future);
      final config = _ref.read(backendServiceProvider).getConfig();
      final layout = config['layout'];
      if (layout is Map) {
        state = DockLayout.fromJson(Map<String, dynamic>.from(layout));
      }
    } catch (_) {
      // The default layout is a safe fallback for a missing or old config.
    } finally {
      _restored = true;
    }
  }

  void _set(DockLayout next) {
    state = next;
    if (!_restored) return;
    _saveTimer?.cancel();
    _saveTimer = Timer(const Duration(milliseconds: 400), () {
      try {
        final backend = _ref.read(backendServiceProvider);
        backend.setLayout(state.toJson());
        backend.saveConfig();
      } catch (_) {
        // Persistence failure must not make a dock interaction unusable.
      }
    });
  }

  void toggleActivityBar() =>
      _set(state.copyWith(activityBarVisible: !state.activityBarVisible));

  void toggleBottom() =>
      _set(state.copyWith(bottomVisible: !state.bottomVisible));

  void toggleRight() => _set(state.copyWith(rightVisible: !state.rightVisible));

  void setBottomSize(double size) =>
      _set(state.copyWith(bottomSize: size.clamp(150, 600)));

  void setRightSize(double size) =>
      _set(state.copyWith(rightSize: size.clamp(200, 600)));

  void setBottomTab(int index) => _set(
    state.copyWith(bottom: state.bottom.copyWith(activeIndex: () => index)),
  );

  void setRightTab(int index) => _set(
    state.copyWith(right: state.right.copyWith(activeIndex: () => index)),
  );

  void setCenterTab(PanelKind kind) =>
      _set(state.copyWith(center: state.center.setActive(kind)));

  void movePanelTo(DockArea area, PanelKind kind) =>
      _set(state.moveTo(area, kind));

  void reorderTab(DockArea area, int oldIndex, int newIndex) {
    final stack = state.stack(area).move(oldIndex, newIndex);
    switch (area) {
      case DockArea.center:
        _set(state.copyWith(center: stack));
      case DockArea.bottom:
        _set(state.copyWith(bottom: stack));
      case DockArea.right:
        _set(state.copyWith(right: stack));
    }
  }

  void resetLayout() => _set(DockLayout.defaultLayout());

  void fromJson(Map<String, dynamic> json) => _set(DockLayout.fromJson(json));

  Map<String, dynamic> toJson() => state.toJson();
}
