import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'src/theme/app_theme.dart';
import 'src/panels/shell.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();

  // 设置窗口最小尺寸（桌面端）
  SystemChrome.setPreferredOrientations([
    DeviceOrientation.landscapeLeft,
    DeviceOrientation.landscapeRight,
  ]);

  runApp(const ProviderScope(child: HardwareWorkbenchApp()));
}

class HardwareWorkbenchApp extends StatelessWidget {
  const HardwareWorkbenchApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: '硬件调试工作台',
      debugShowCheckedModeBanner: false,
      theme: AppTheme.darkTheme,
      home: const WorkbenchShell(),
    );
  }
}
