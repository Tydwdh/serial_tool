import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hardware_workbench/src/theme/app_theme.dart';

void main() {
  testWidgets('App theme is dark', (WidgetTester tester) async {
    final theme = AppTheme.darkTheme;
    expect(theme.brightness, Brightness.dark);
  });
}