/// Dart 数据模型，与 Rust 后端交换的数据类型。
library;

import 'dart:convert';

/// 串口描述符
class PortDescriptor {
  final String portName;
  final String portType;
  final String? vid;
  final String? pid;

  PortDescriptor({
    required this.portName,
    this.portType = 'unknown',
    this.vid,
    this.pid,
  });

  factory PortDescriptor.fromJson(Map<String, dynamic> json) {
    return PortDescriptor(
      portName: json['port_name'] as String? ?? '',
      portType: json['port_type'] as String? ?? 'unknown',
      vid: json['vid'] as String?,
      pid: json['pid'] as String?,
    );
  }

  Map<String, dynamic> toJson() => {
    'port_name': portName,
    'port_type': portType,
    'vid': vid,
    'pid': pid,
  };
}

/// 串口配置
class SerialConfig {
  final String portName;
  final int baudRate;
  final String dataBits;
  final String stopBits;
  final String parity;

  SerialConfig({
    required this.portName,
    this.baudRate = 115200,
    this.dataBits = '8',
    this.stopBits = '1',
    this.parity = 'none',
  });

  Map<String, dynamic> toJson() => {
    'port_name': portName,
    'baud_rate': baudRate,
    'data_bits': dataBits,
    'stop_bits': stopBits,
    'parity': parity,
  };
}

/// 串口方向
enum Direction { rx, tx, internal }

/// 日志级别
enum LogLevel { error, warn, info, debug, trace }

/// 终端条目
class TerminalEntry {
  final int id;
  final String port;
  final Direction direction;
  final String text;
  final List<int> rawBytes;
  final int timestamp;
  final bool isHex;

  TerminalEntry({
    required this.id,
    required this.port,
    required this.direction,
    required this.text,
    required this.rawBytes,
    required this.timestamp,
    this.isHex = false,
  });
}

/// 日志条目
class LogEntry {
  final LogLevel level;
  final String source;
  final String message;
  final int timestamp;

  LogEntry({
    required this.level,
    required this.source,
    required this.message,
    required this.timestamp,
  });
}

/// 插件摘要
class PluginSummary {
  final String id;
  final String name;
  final String version;
  final String? description;
  final bool enabled;

  PluginSummary({
    required this.id,
    required this.name,
    required this.version,
    this.description,
    this.enabled = false,
  });

  factory PluginSummary.fromJson(Map<String, dynamic> json) {
    return PluginSummary(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      version: json['version'] as String? ?? '',
      description: json['description'] as String?,
      enabled: const {
        'enabled',
        'running',
      }.contains((json['state'] ?? '').toString().toLowerCase()),
    );
  }
}

/// 后端状态摘要
class BackendStatus {
  final int portsCount;
  final List<String> openPorts;
  final String? selectedPort;
  final bool recording;
  final int pluginsCount;

  BackendStatus({
    this.portsCount = 0,
    this.openPorts = const [],
    this.selectedPort,
    this.recording = false,
    this.pluginsCount = 0,
  });

  factory BackendStatus.fromJson(Map<String, dynamic> json) {
    return BackendStatus(
      portsCount: json['ports_count'] as int? ?? 0,
      openPorts: (json['open_ports'] as List? ?? const [])
          .whereType<String>()
          .toList(growable: false),
      selectedPort: json['selected_port'] as String?,
      recording: json['recording'] as bool? ?? false,
      pluginsCount: json['plugins_count'] as int? ?? 0,
    );
  }
}

/// 后端事件（与 Rust BackendEvent 对应）
class BackendEvent {
  final String type;
  final Map<String, dynamic> data;

  BackendEvent({required this.type, required this.data});

  factory BackendEvent.fromJson(String jsonStr) {
    final map = jsonDecode(jsonStr) as Map<String, dynamic>;
    final type = map['type'] as String? ?? 'unknown';
    // Remove the type field and keep the rest as data
    final data = Map<String, dynamic>.from(map);
    data.remove('type');
    return BackendEvent(type: type, data: data);
  }
}
