import 'package:flutter/material.dart';
import '../theme/app_theme.dart';
import 'dock_layout.dart';

/// Dock 标签栏 — 支持拖拽重排和跨区域移动
class DockTabBar extends StatelessWidget {
  final DockArea area;
  final DockStack stack;
  final void Function(int index) onTap;
  final void Function() onClose;
  final void Function(int oldIndex, int newIndex) onReorder;

  const DockTabBar({
    super.key,
    required this.area,
    required this.stack,
    required this.onTap,
    required this.onClose,
    required this.onReorder,
  });

  @override
  Widget build(BuildContext context) {
    if (stack.isEmpty) return const SizedBox.shrink();

    return Container(
      height: 40,
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 5),
      decoration: const BoxDecoration(
        color: AppTheme.bgPanel,
        border: Border(bottom: BorderSide(color: AppTheme.borderColor)),
      ),
      child: Row(
        children: [
          Expanded(
            child: ReorderableListView.builder(
              scrollDirection: Axis.horizontal,
              buildDefaultDragHandles: false,
              itemCount: stack.length,
              onReorderItem: (oldIndex, newIndex) =>
                  onReorder(oldIndex, newIndex),
              proxyDecorator: (child, index, animation) {
                return Material(
                  color: Colors.transparent,
                  child: Container(
                    decoration: BoxDecoration(
                      color: AppTheme.bgActive,
                      border: Border.all(color: AppTheme.accent),
                      borderRadius: BorderRadius.circular(9),
                    ),
                    child: child,
                  ),
                );
              },
              itemBuilder: (_, i) {
                final kind = stack.tabs[i];
                final active = i == stack.activeIndex;
                return _TabItem(
                  // The key must identify the tab itself, not its current
                  // position. Position-based keys make Flutter rebuild the
                  // wrong item after a drag reorder.
                  key: ValueKey('$area-${kind.name}'),
                  kind: kind,
                  active: active,
                  onTap: () => onTap(i),
                  onDragStart: (details) {
                    // 拖拽开始
                  },
                );
              },
            ),
          ),
          if (area != DockArea.center)
            IconButton(
              icon: const Icon(Icons.close_rounded, size: 16),
              color: AppTheme.textSecondary,
              onPressed: onClose,
              style: IconButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: const Size(28, 28),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              ),
            ),
        ],
      ),
    );
  }
}

class _TabItem extends StatelessWidget {
  final PanelKind kind;
  final bool active;
  final VoidCallback onTap;
  final void Function(DragStartDetails)? onDragStart;

  const _TabItem({
    super.key,
    required this.kind,
    required this.active,
    required this.onTap,
    this.onDragStart,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 4),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(8),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 160),
            curve: Curves.easeOut,
            width: (kind.title.length * 14 + 38).clamp(72.0, 174.0).toDouble(),
            padding: const EdgeInsets.symmetric(horizontal: 10),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: active ? AppTheme.bgActive : Colors.transparent,
              borderRadius: BorderRadius.circular(8),
              border: Border.all(
                color: active ? AppTheme.borderColor : Colors.transparent,
              ),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  kind.icon,
                  size: 15,
                  color: active ? AppTheme.accentLight : AppTheme.textSecondary,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    kind.title,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 12,
                      fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                      color: active
                          ? AppTheme.textBright
                          : AppTheme.textSecondary,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
