// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

/**
 * Material 3 baseline palette. On Android 12+ (API 31+) we prefer
 * the system's dynamic-colour scheme so the app blends into the
 * user's wallpaper-derived theme. Pre-12 devices fall back to the
 * static palette below.
 */
private val LightBaseline = lightColorScheme(
    primary = Color(0xFF3F51B5),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFC5CAE9),
    secondary = Color(0xFF5C6BC0),
    background = Color(0xFFF5F5F5),
    surface = Color(0xFFFFFFFF),
)

private val DarkBaseline = darkColorScheme(
    primary = Color(0xFF9FA8DA),
    onPrimary = Color(0xFF1A237E),
    primaryContainer = Color(0xFF303F9F),
    secondary = Color(0xFF7986CB),
    background = Color(0xFF121212),
    surface = Color(0xFF1E1E1E),
)

@Composable
fun BypassTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val colorScheme = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            if (darkTheme) dynamicDarkColorScheme(context)
            else dynamicLightColorScheme(context)
        }
        darkTheme -> DarkBaseline
        else -> LightBaseline
    }
    MaterialTheme(
        colorScheme = colorScheme,
        content = content,
    )
}
