// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android

import android.app.Application
import io.bypass.android.crypto.OpenKeychainCrypto
import io.bypass.android.repository.BypassRepository
import uniffi.bypass.BypassStore
import java.io.File

/**
 * Application subclass that owns the [BypassRepository] singleton
 * for the lifetime of the process. ViewModels reach it through
 * `LocalContext.current.applicationContext` — the manual-DI pattern
 * locked in by ADR-0025.
 *
 * Construction order on first repository access:
 *   1. Android instantiates [BypassApplication] on cold start.
 *   2. [OpenKeychainCrypto.init] kicks off an async AIDL bind to
 *      `org.sufficientlysecure.keychain`. The first encrypt /
 *      decrypt call blocks on a [java.util.concurrent.CountDownLatch]
 *      until the bind completes (up to ~10 s).
 *   3. We resolve the store root under `filesDir` and call
 *      `BypassStore.open(rootDir, crypto)` to get a working store.
 *   4. The [BypassRepository] wrapping it becomes the
 *      application-wide singleton; ViewModels grab it via the
 *      Application Context.
 *
 * The OpenKeychain service connection is held for the life of the
 * process; Android tears it down at process death automatically.
 */
class BypassApplication : Application() {

    val crypto: OpenKeychainCrypto by lazy { OpenKeychainCrypto(this) }

    val repository: BypassRepository by lazy {
        val storeDir = File(filesDir, "store").apply { mkdirs() }
        val store = BypassStore.open(storeDir.absolutePath, crypto)
        BypassRepository(store)
    }
}
