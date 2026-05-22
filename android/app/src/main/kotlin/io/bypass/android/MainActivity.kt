// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.fillMaxSize
import androidx.navigation.compose.rememberNavController
import io.bypass.android.ui.BypassNavHost
import io.bypass.android.ui.theme.BypassTheme

/**
 * Single Activity per ADR-0025. The Compose tree below wires up the
 * [BypassTheme] and the [BypassNavHost] that owns every screen.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            BypassTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    val navController = rememberNavController()
                    BypassNavHost(navController = navController)
                }
            }
        }
    }
}
