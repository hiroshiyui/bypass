// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.ActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import androidx.navigation.compose.rememberNavController
import io.bypass.android.crypto.CryptoUiRequest
import io.bypass.android.ui.BypassNavHost
import io.bypass.android.ui.theme.BypassTheme
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentLinkedQueue

/**
 * Single Activity per ADR-0025. Drives the Compose tree below
 * AND brokers OpenKeychain user-interaction `PendingIntent`s from
 * the [io.bypass.android.crypto.CryptoUiBridge] per the 8.2.b.ii
 * async bridge.
 *
 * Pairing requests to results: `ActivityResultLauncher` doesn't
 * tell us which `PendingIntent` triggered each callback, so we
 * maintain a FIFO [ConcurrentLinkedQueue] of [CryptoUiRequest]s.
 * Launches go out in order; callbacks come back in the same order;
 * each callback pops the head of the queue and delivers the
 * result to its waiting IO thread.
 */
class MainActivity : ComponentActivity() {

    private val pendingRequests = ConcurrentLinkedQueue<CryptoUiRequest>()

    private val launcher = registerForActivityResult(
        ActivityResultContracts.StartIntentSenderForResult(),
    ) { result: ActivityResult ->
        val req = pendingRequests.poll()
        if (req == null) {
            // Spurious callback (e.g. process restoration race);
            // nobody to deliver to.
            Log.w(TAG, "ActivityResult arrived with no pending CryptoUiRequest")
            return@registerForActivityResult
        }
        req.deliver(result)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        val app = applicationContext as BypassApplication

        // Collect crypto UI requests for the life of the Activity.
        // Each emission is a PendingIntent to launch + a callback
        // box the OpenKeychainCrypto IO thread is blocked on.
        lifecycleScope.launch {
            app.cryptoBridge.requests.collect { req ->
                pendingRequests.offer(req)
                try {
                    val sender = IntentSenderRequest.Builder(
                        req.pendingIntent.intentSender,
                    ).build()
                    launcher.launch(sender)
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to launch crypto PendingIntent", e)
                    pendingRequests.remove(req)
                    req.deliver(null)
                }
            }
        }

        setContent {
            BypassTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    val navController = rememberNavController()
                    BypassNavHost(navController = navController)
                }
            }
        }
    }

    private companion object {
        const val TAG = "bypass-activity"
    }
}
