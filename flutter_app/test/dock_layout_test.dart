import 'package:flutter_test/flutter_test.dart';
import 'package:hardware_workbench/src/panels/dock_layout.dart';

void main() {
  group('DockLayout persistence', () {
    test('round-trips panels, visibility, and dimensions', () {
      final layout = DockLayout.defaultLayout().copyWith(
        bottomSize: 412,
        rightSize: 268,
        bottomVisible: true,
        rightVisible: true,
        activityBarVisible: false,
      );

      final restored = DockLayout.fromJson(layout.toJson());

      expect(restored.center.active, PanelKind.terminal);
      expect(restored.bottom.tabs, [PanelKind.sender, PanelKind.logs]);
      expect(restored.right.active, PanelKind.plugins);
      expect(restored.bottomSize, 412);
      expect(restored.rightSize, 268);
      expect(restored.bottomVisible, isTrue);
      expect(restored.rightVisible, isTrue);
      expect(restored.activityBarVisible, isFalse);
    });

    test('invalid persisted active index falls back safely', () {
      final stack = DockStack.fromJson({
        'tabs': ['terminal', 'logs'],
        'activeIndex': -1,
      });
      expect(stack.active, PanelKind.terminal);

      final oversized = DockStack.fromJson({
        'tabs': ['terminal'],
        'activeIndex': 99,
      });
      expect(oversized.active, PanelKind.terminal);
    });

    test('moving a panel keeps it unique and activates destination', () {
      final moved = DockLayout.defaultLayout().moveTo(
        DockArea.center,
        PanelKind.logs,
      );
      expect(moved.center.active, PanelKind.logs);
      expect(moved.center.tabs.where((panel) => panel == PanelKind.logs), hasLength(1));
      expect(moved.bottom.tabs, [PanelKind.sender]);
    });
  });
}
