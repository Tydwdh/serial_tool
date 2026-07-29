import 'package:flutter/material.dart';

/// 面板种类 — 与 Rust 端的 PanelKind 对应
enum PanelKind {
  terminal, // 终端/接收
  sender, // 发送器
  logs, // 日志
  chart, // 图表
  plugins, // 插件
  settings, // 设置
  replay, // 回放
  devices, // 设备信息
  dynamic, // 动态面板（插件创建）
}

/// 停靠区
enum DockArea { center, bottom, right }

/// 停靠区标签栈 — 不可变
class DockStack {
  final List<PanelKind> tabs;
  final int? activeIndex;

  const DockStack({this.tabs = const [], this.activeIndex});

  PanelKind? get active => activeIndex != null && activeIndex! < tabs.length
      ? tabs[activeIndex!]
      : null;
  int get length => tabs.length;
  bool get isEmpty => tabs.isEmpty;

  DockStack copyWith({List<PanelKind>? tabs, int? Function()? activeIndex}) {
    return DockStack(
      tabs: tabs ?? this.tabs,
      activeIndex: activeIndex != null ? activeIndex() : this.activeIndex,
    );
  }

  DockStack add(PanelKind kind) {
    final newTabs = [...tabs, kind];
    return DockStack(tabs: newTabs, activeIndex: newTabs.length - 1);
  }

  DockStack remove(PanelKind kind) {
    final i = tabs.indexOf(kind);
    if (i < 0) return this;
    final newTabs = [...tabs]..removeAt(i);
    int? newActive = activeIndex;
    if (newActive != null) {
      if (newActive >= newTabs.length) newActive = newTabs.length - 1;
      if (newActive < 0) newActive = null;
    }
    return DockStack(tabs: newTabs, activeIndex: newActive);
  }

  DockStack move(int oldIndex, int newIndex) {
    if (oldIndex < 0 || oldIndex >= tabs.length) return this;
    if (newIndex < 0 || newIndex >= tabs.length) return this;
    final newTabs = [...tabs];
    final item = newTabs.removeAt(oldIndex);
    newTabs.insert(newIndex, item);
    int? newActive = activeIndex;
    if (newActive == oldIndex) {
      newActive = newIndex;
    } else if (newActive != null) {
      if (oldIndex < newActive && newIndex >= newActive) {
        newActive = newActive - 1;
      } else if (oldIndex > newActive && newIndex <= newActive) {
        newActive = newActive + 1;
      }
    }
    return DockStack(tabs: newTabs, activeIndex: newActive);
  }

  DockStack setActive(PanelKind kind) {
    final i = tabs.indexOf(kind);
    if (i < 0) return this;
    return DockStack(tabs: tabs, activeIndex: i);
  }

  Map<String, dynamic> toJson() => {
    'tabs': tabs.map((t) => t.name).toList(),
    'activeIndex': activeIndex,
  };

  factory DockStack.fromJson(Map<String, dynamic> json) {
    final tabs =
        (json['tabs'] as List?)
            ?.map(
              (e) => PanelKind.values.firstWhere(
                (p) => p.name == e,
                orElse: () => PanelKind.terminal,
              ),
            )
            .toList() ??
        [];
    final requestedIndex = json['activeIndex'] as int?;
    final activeIndex =
        requestedIndex != null &&
            requestedIndex >= 0 &&
            requestedIndex < tabs.length
        ? requestedIndex
        : (tabs.isEmpty ? null : 0);
    return DockStack(tabs: tabs, activeIndex: activeIndex);
  }
}

/// Dock 布局 — 三个停靠区的完整状态，不可变
class DockLayout {
  final DockStack center;
  final DockStack bottom;
  final DockStack right;
  final double bottomSize;
  final double rightSize;
  final bool bottomVisible;
  final bool rightVisible;
  final bool activityBarVisible;

  const DockLayout({
    this.center = const DockStack(),
    this.bottom = const DockStack(),
    this.right = const DockStack(),
    this.bottomSize = 300,
    this.rightSize = 300,
    this.bottomVisible = true,
    this.rightVisible = true,
    this.activityBarVisible = true,
  });

  DockStack stack(DockArea area) {
    switch (area) {
      case DockArea.center:
        return center;
      case DockArea.bottom:
        return bottom;
      case DockArea.right:
        return right;
    }
  }

  DockLayout copyWith({
    DockStack? center,
    DockStack? bottom,
    DockStack? right,
    double? bottomSize,
    double? rightSize,
    bool? bottomVisible,
    bool? rightVisible,
    bool? activityBarVisible,
  }) {
    return DockLayout(
      center: center ?? this.center,
      bottom: bottom ?? this.bottom,
      right: right ?? this.right,
      bottomSize: bottomSize ?? this.bottomSize,
      rightSize: rightSize ?? this.rightSize,
      bottomVisible: bottomVisible ?? this.bottomVisible,
      rightVisible: rightVisible ?? this.rightVisible,
      activityBarVisible: activityBarVisible ?? this.activityBarVisible,
    );
  }

  /// 将面板移至指定停靠区
  DockLayout moveTo(DockArea area, PanelKind kind) {
    var c = center.remove(kind);
    var b = bottom.remove(kind);
    var r = right.remove(kind);
    var bv = bottomVisible;
    var rv = rightVisible;
    switch (area) {
      case DockArea.center:
        c = c.add(kind);
      case DockArea.bottom:
        b = b.add(kind);
        bv = true;
      case DockArea.right:
        r = r.add(kind);
        rv = true;
    }
    return copyWith(
      center: c,
      bottom: b,
      right: r,
      bottomVisible: bv,
      rightVisible: rv,
    );
  }

  /// 默认布局：终端在中心，发送器/日志在底部，插件在右侧
  static DockLayout defaultLayout() {
    return DockLayout(
      center: DockStack(tabs: [PanelKind.terminal], activeIndex: 0),
      bottom: DockStack(
        tabs: [PanelKind.sender, PanelKind.logs],
        activeIndex: 0,
      ),
      right: DockStack(tabs: [PanelKind.plugins], activeIndex: 0),
      bottomVisible: true,
      rightVisible: true,
    );
  }

  Map<String, dynamic> toJson() => {
    'center': center.toJson(),
    'bottom': bottom.toJson(),
    'right': right.toJson(),
    'bottomSize': bottomSize,
    'rightSize': rightSize,
    'bottomVisible': bottomVisible,
    'rightVisible': rightVisible,
    'activityBarVisible': activityBarVisible,
  };

  factory DockLayout.fromJson(Map<String, dynamic> json) {
    return DockLayout(
      center: DockStack.fromJson(json['center'] as Map<String, dynamic>? ?? {}),
      bottom: DockStack.fromJson(json['bottom'] as Map<String, dynamic>? ?? {}),
      right: DockStack.fromJson(json['right'] as Map<String, dynamic>? ?? {}),
      bottomSize: (json['bottomSize'] as num?)?.toDouble() ?? 300,
      rightSize: (json['rightSize'] as num?)?.toDouble() ?? 300,
      bottomVisible: json['bottomVisible'] as bool? ?? false,
      rightVisible: json['rightVisible'] as bool? ?? false,
      activityBarVisible: json['activityBarVisible'] as bool? ?? true,
    );
  }
}

/// 面板标题和图标
extension PanelKindHelper on PanelKind {
  String get title {
    switch (this) {
      case PanelKind.terminal:
        return '终端';
      case PanelKind.sender:
        return '发送器';
      case PanelKind.logs:
        return '日志';
      case PanelKind.chart:
        return '图表';
      case PanelKind.plugins:
        return '插件';
      case PanelKind.settings:
        return '设置';
      case PanelKind.replay:
        return '回放';
      case PanelKind.devices:
        return '设备';
      case PanelKind.dynamic:
        return '动态面板';
    }
  }

  IconData get icon {
    switch (this) {
      case PanelKind.terminal:
        return Icons.terminal;
      case PanelKind.sender:
        return Icons.send;
      case PanelKind.logs:
        return Icons.article;
      case PanelKind.chart:
        return Icons.show_chart;
      case PanelKind.plugins:
        return Icons.extension;
      case PanelKind.settings:
        return Icons.settings;
      case PanelKind.replay:
        return Icons.replay;
      case PanelKind.devices:
        return Icons.memory;
      case PanelKind.dynamic:
        return Icons.widgets;
    }
  }
}

/// 拖拽状态 — 跨停靠区拖拽时的临时数据
class DockDragState {
  final PanelKind? kind;
  final DockArea? sourceArea;
  final DockArea? targetArea;
  final int? insertIndex;
  final InsertIndicator indicator;

  const DockDragState({
    this.kind,
    this.sourceArea,
    this.targetArea,
    this.insertIndex,
    this.indicator = InsertIndicator.none,
  });
}

/// 拖拽插入指示器类型
enum InsertIndicator { none, before, after }
