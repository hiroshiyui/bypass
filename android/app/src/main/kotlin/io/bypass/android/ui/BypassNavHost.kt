// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import io.bypass.android.BypassApplication
import io.bypass.android.ui.screens.GenerateScreen
import io.bypass.android.ui.screens.InitScreen
import io.bypass.android.ui.screens.InsertScreen
import io.bypass.android.ui.screens.ListScreen
import io.bypass.android.ui.screens.ShowScreen

/**
 * Navigation routes. Centralised so screens don't drift on string
 * keys. Five routes per ADR-0025: init / list / show/{path} /
 * insert / generate. `show` takes the entry path as a path arg
 * (URL-encoded to survive the `/` separators).
 */
object Routes {
    const val INIT = "init"
    const val LIST = "list"
    const val INSERT = "insert"
    const val GENERATE = "generate"
    const val SHOW = "show/{path}"
    fun show(path: String): String = "show/${java.net.URLEncoder.encode(path, "UTF-8")}"
}

@Composable
fun BypassNavHost(navController: NavHostController) {
    val context = LocalContext.current
    val app = context.applicationContext as BypassApplication
    val repository = app.repository

    // Decide the start destination by probing whether the store has
    // been initialised yet. First-launch users see the Init screen;
    // returning users land on the entry list.
    var initialised by remember { mutableStateOf<Boolean?>(null) }
    val rootDir = remember { java.io.File(context.filesDir, "store").absolutePath }
    LaunchedEffect(Unit) {
        initialised = repository.isInitialised(rootDir)
    }
    val startDestination = when (initialised) {
        null -> null
        true -> Routes.LIST
        false -> Routes.INIT
    } ?: return  // Loading; render nothing for one frame.

    NavHost(navController = navController, startDestination = startDestination) {
        composable(Routes.INIT) {
            InitScreen(
                repository = repository,
                onInitialised = {
                    navController.navigate(Routes.LIST) {
                        popUpTo(Routes.INIT) { inclusive = true }
                    }
                },
            )
        }
        composable(Routes.LIST) {
            ListScreen(
                repository = repository,
                onEntry = { path -> navController.navigate(Routes.show(path)) },
                onInsert = { navController.navigate(Routes.INSERT) },
                onGenerate = { navController.navigate(Routes.GENERATE) },
            )
        }
        composable(Routes.SHOW) { backStackEntry ->
            val encoded = backStackEntry.arguments?.getString("path").orEmpty()
            val path = java.net.URLDecoder.decode(encoded, "UTF-8")
            ShowScreen(
                repository = repository,
                path = path,
                onBack = { navController.popBackStack() },
            )
        }
        composable(Routes.INSERT) {
            InsertScreen(
                repository = repository,
                onSaved = { navController.popBackStack() },
                onBack = { navController.popBackStack() },
            )
        }
        composable(Routes.GENERATE) {
            GenerateScreen(
                repository = repository,
                onGenerated = { navController.popBackStack() },
                onBack = { navController.popBackStack() },
            )
        }
    }
}
