// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.repository

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.bypass.BypassStore
import java.io.File

/**
 * Thin coroutine wrapper around the UniFFI [BypassStore]. Every
 * call dispatches to [Dispatchers.IO] so the Compose main thread
 * never blocks on the FFI (which itself blocks on the OpenKeychain
 * AIDL round-trip once 8.2.b lands).
 *
 * The repository deliberately does NOT cache results — `Store` is
 * fast enough on local I/O that re-reading on every screen entry
 * is the simpler-and-correct choice. If a real performance issue
 * surfaces, add a [kotlinx.coroutines.flow.MutableStateFlow] cache
 * here without changing the screen API.
 */
class BypassRepository(
    private val store: BypassStore,
) {

    /** True once the store has a `.gpg-id` (i.e. has been
     *  initialised by the user). Probed by reading the store root
     *  through the [BypassStore.ls] call's behaviour — `ls` fails
     *  with `NotInitialized` until then. */
    suspend fun isInitialised(rootDir: String): Boolean = withContext(Dispatchers.IO) {
        File(rootDir, ".gpg-id").exists()
    }

    suspend fun init(recipients: List<String>) = withContext(Dispatchers.IO) {
        store.init(recipients)
    }

    suspend fun ls(subpath: String? = null): List<String> = withContext(Dispatchers.IO) {
        store.ls(subpath)
    }

    suspend fun find(pattern: String): List<String> = withContext(Dispatchers.IO) {
        store.find(pattern)
    }

    suspend fun show(path: String): ByteArray = withContext(Dispatchers.IO) {
        store.show(path)
    }

    suspend fun showField(path: String, field: String): String = withContext(Dispatchers.IO) {
        store.showField(path, field)
    }

    suspend fun insert(
        path: String,
        plaintext: ByteArray,
        overwrite: Boolean = false,
    ) = withContext(Dispatchers.IO) {
        store.insert(path, plaintext, overwrite)
    }

    suspend fun generate(
        path: String,
        length: UInt? = null,
        symbols: Boolean? = null,
        inPlace: Boolean = false,
        force: Boolean = false,
    ): String = withContext(Dispatchers.IO) {
        store.generate(path, length, symbols, inPlace, force)
    }

    suspend fun otp(path: String): String = withContext(Dispatchers.IO) {
        store.otp(path)
    }

    suspend fun rm(path: String, recursive: Boolean = false) = withContext(Dispatchers.IO) {
        store.rm(path, recursive)
    }
}
