import 'package:flutter/material.dart';

/// A calm, high-contrast desktop workbench palette. Terminal data keeps its
/// monospace styling locally; the surrounding product UI uses a lighter,
/// modern system type scale.
class AppTheme {
  static const Color bgDark = Color(0xFF0A1020);
  static const Color bgPanel = Color(0xFF111A2C);
  static const Color bgInput = Color(0xFF0D1628);
  static const Color bgHover = Color(0xFF18243B);
  static const Color bgActive = Color(0xFF213655);
  static const Color borderColor = Color(0xFF283955);
  static const Color textPrimary = Color(0xFFDCE7F7);
  static const Color textSecondary = Color(0xFF91A4C0);
  static const Color textBright = Color(0xFFF4F8FF);
  static const Color accent = Color(0xFF4F9CFF);
  static const Color accentLight = Color(0xFF7BC4FF);
  static const Color success = Color(0xFF38D39F);
  static const Color warning = Color(0xFFF3C969);
  static const Color error = Color(0xFFFF717D);
  static const Color rxColor = Color(0xFF72B7FF);
  static const Color txColor = Color(0xFFF2B982);
  static const Color scrollbarBg = Color(0xFF111A2C);
  static const Color scrollbarFg = Color(0xFF4A6389);

  static ThemeData get darkTheme {
    const radius = Radius.circular(10);
    const outline = OutlineInputBorder(
      borderRadius: BorderRadius.all(radius),
      borderSide: BorderSide(color: borderColor),
    );
    return ThemeData(
      useMaterial3: true,
      brightness: Brightness.dark,
      fontFamily: 'Segoe UI',
      scaffoldBackgroundColor: bgDark,
      colorScheme: const ColorScheme.dark(
        primary: accent,
        secondary: accentLight,
        surface: bgPanel,
        error: error,
        onPrimary: Color(0xFF061221),
        onSecondary: Color(0xFF061221),
        onSurface: textPrimary,
        onError: Color(0xFF25040A),
      ),
      appBarTheme: const AppBarTheme(
        backgroundColor: bgDark,
        foregroundColor: textPrimary,
        elevation: 0,
        surfaceTintColor: Colors.transparent,
      ),
      cardTheme: const CardThemeData(
        color: bgPanel,
        elevation: 0,
        margin: EdgeInsets.zero,
        clipBehavior: Clip.antiAlias,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(14)),
          side: BorderSide(color: borderColor),
        ),
      ),
      dividerTheme: const DividerThemeData(color: borderColor, thickness: 1),
      inputDecorationTheme: const InputDecorationTheme(
        filled: true,
        fillColor: bgInput,
        border: outline,
        enabledBorder: outline,
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.all(radius),
          borderSide: BorderSide(color: accent, width: 1.5),
        ),
        contentPadding: EdgeInsets.symmetric(horizontal: 11, vertical: 9),
        isDense: true,
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.all(Radius.circular(9)),
          ),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.all(Radius.circular(8)),
          ),
        ),
      ),
      tooltipTheme: const TooltipThemeData(
        decoration: BoxDecoration(
          color: Color(0xFF20314D),
          borderRadius: BorderRadius.all(Radius.circular(7)),
        ),
        textStyle: TextStyle(fontSize: 12, color: textBright),
      ),
      textTheme: const TextTheme(
        bodySmall: TextStyle(fontSize: 12, color: textSecondary),
        bodyMedium: TextStyle(fontSize: 13, color: textPrimary),
        bodyLarge: TextStyle(fontSize: 15, color: textPrimary),
        labelSmall: TextStyle(fontSize: 11, color: textSecondary),
        titleSmall: TextStyle(
          fontSize: 13,
          color: textBright,
          fontWeight: FontWeight.w600,
        ),
      ),
      scrollbarTheme: ScrollbarThemeData(
        trackColor: WidgetStateProperty.all(scrollbarBg),
        thumbColor: WidgetStateProperty.all(scrollbarFg),
        thickness: WidgetStateProperty.all(7),
        radius: const Radius.circular(5),
      ),
    );
  }
}
