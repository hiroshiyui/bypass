// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android

import android.app.Application
import io.bypass.android.crypto.CryptoUiBridge
import io.bypass.android.crypto.OpenKeychainCrypto
import io.bypass.android.repository.BypassRepository
import uniffi.bypass.BypassStore
import java.io.File

/**
 * Application subclass that owns the [BypassRepository] singleton
 * for the lifetime of the process, plus a [CryptoUiBridge] that
 * the `MainActivity` collects from to surface OpenKeychain
 * `PendingIntent`s. Manual-DI pattern per ADR-0025.
 *
 * Construction order on first repository access:
 *   1. Android instantiates [BypassApplication] on cold start.
 *   2. [cryptoBridge] is constructed (it's a passive container —
 *      no I/O yet).
 *   3. [crypto] kicks off an async AIDL bind to OpenKeychain. The
 *      first encrypt / decrypt call blocks on a
 *      [java.util.concurrent.CountDownLatch] until the bind
 *      completes (~10 s timeout).
 *   4. [repository] is the suspending facade over `BypassStore`.
 *   5. `MainActivity` collects [cryptoBridge].requests on the main
 *      thread and launches each `PendingIntent` via an
 *      `ActivityResultLauncher`.
 *
 * The OpenKeychain service connection is held for the life of the
 * process; Android tears it down at process death automatically.
 */
class BypassApplication : Application() {

    val cryptoBridge: CryptoUiBridge by lazy { CryptoUiBridge() }

    val crypto: OpenKeychainCrypto by lazy { OpenKeychainCrypto(this, cryptoBridge) }

    val repository: BypassRepository by lazy {
        val storeDir = File(filesDir, "store").apply { mkdirs() }
        val store = BypassStore.open(storeDir.absolutePath, crypto)
        BypassRepository(store)
    }
}
