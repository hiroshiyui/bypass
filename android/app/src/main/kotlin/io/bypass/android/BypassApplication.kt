// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android

import android.app.Application
import io.bypass.android.crypto.StubCrypto
import io.bypass.android.repository.BypassRepository
import uniffi.bypass.BypassStore
import java.io.File

/**
 * Application subclass that owns the [BypassRepository] singleton for
 * the lifetime of the process. ViewModels reach it through
 * [androidx.compose.ui.platform.LocalContext]'s applicationContext —
 * the manual-DI pattern locked in by ADR-0025.
 *
 * Construction order:
 *   1. Android instantiates [BypassApplication] on cold start.
 *   2. We resolve the store root under `filesDir`, ensure it exists.
 *   3. We construct a [StubCrypto] (8.2.a) — 8.2.b will swap this
 *      for the OpenKeychain client.
 *   4. We open the [BypassStore] from the UniFFI surface.
 *   5. The [BypassRepository] wrapping it becomes the application-
 *      wide singleton.
 */
class BypassApplication : Application() {

    val repository: BypassRepository by lazy {
        val storeDir = File(filesDir, "store").apply { mkdirs() }
        val crypto = StubCrypto()
        val store = BypassStore.open(storeDir.absolutePath, crypto)
        BypassRepository(store)
    }
}
