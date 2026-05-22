// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.crypto

import android.content.Context
import android.content.Intent
import android.util.Log
import org.openintents.openpgp.IOpenPgpService2
import org.openintents.openpgp.OpenPgpError
import org.openintents.openpgp.util.OpenPgpApi
import org.openintents.openpgp.util.OpenPgpServiceConnection
import uniffi.bypass.BypassException
import uniffi.bypass.Crypto
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Real `Crypto` implementation for Phase 8.2.b (replaces
 * `StubCrypto`). Binds to OpenKeychain's `OPEN_PGP_SERVICE` AIDL
 * endpoint via the `org.sufficientlysecure:openpgp-api` library and
 * synchronously dispatches encrypt / decrypt calls through it.
 *
 * **Happy-path scope** per ADR-0025:
 * - `RESULT_CODE_SUCCESS` → return the bytes.
 * - `RESULT_CODE_ERROR` → throw [BypassException.Crypto] with
 *   OpenKeychain's error message.
 * - `RESULT_CODE_USER_INTERACTION_REQUIRED` → throw
 *   [BypassException.Crypto] with a "passphrase cache expired,
 *   unlock in OpenKeychain first" message. Phase 8.2.b.ii (if /
 *   when it lands) is where the async PendingIntent bridge fits.
 *
 * Lifetime: one instance per `BypassApplication`. `bindToService`
 * is called in `init`, and the bound `OpenPgpApi` is held for the
 * life of the process. The service is unbound in [close], but
 * `Application` has no clean shutdown hook so in practice the
 * process death tears the binding down.
 */
class OpenKeychainCrypto(context: Context) : Crypto, AutoCloseable {

    private val appContext: Context = context.applicationContext
    private val serviceConnection: OpenPgpServiceConnection
    private val boundLatch = CountDownLatch(1)

    @Volatile
    private var api: OpenPgpApi? = null

    @Volatile
    private var bindError: String? = null

    init {
        serviceConnection = OpenPgpServiceConnection(
            appContext,
            OPENKEYCHAIN_PACKAGE,
            object : OpenPgpServiceConnection.OnBound {
                override fun onBound(service: IOpenPgpService2) {
                    Log.i(TAG, "OpenKeychain AIDL service bound")
                    api = OpenPgpApi(appContext, service)
                    boundLatch.countDown()
                }

                override fun onError(e: Exception) {
                    Log.w(TAG, "OpenKeychain bind failed", e)
                    bindError = e.message ?: e.javaClass.simpleName
                    boundLatch.countDown()
                }
            },
        )
        try {
            serviceConnection.bindToService()
        } catch (e: Exception) {
            Log.w(TAG, "bindToService threw synchronously", e)
            bindError = e.message ?: e.javaClass.simpleName
            boundLatch.countDown()
        }
    }

    override fun encrypt(plaintext: ByteArray, recipients: List<String>): ByteArray {
        val api = awaitApi("encrypt")
        val intent = Intent(OpenPgpApi.ACTION_ENCRYPT).apply {
            putExtra(OpenPgpApi.EXTRA_USER_IDS, recipients.toTypedArray())
            putExtra(OpenPgpApi.EXTRA_REQUEST_ASCII_ARMOR, false)
        }
        val output = ByteArrayOutputStream()
        val result = api.executeApi(intent, ByteArrayInputStream(plaintext), output)
        return resolveResult(result, output, "encrypt")
    }

    override fun decrypt(ciphertext: ByteArray): ByteArray {
        val api = awaitApi("decrypt")
        val intent = Intent(OpenPgpApi.ACTION_DECRYPT_VERIFY)
        val output = ByteArrayOutputStream()
        val result = api.executeApi(intent, ByteArrayInputStream(ciphertext), output)
        return resolveResult(result, output, "decrypt")
    }

    override fun close() {
        try {
            serviceConnection.unbindFromService()
        } catch (e: Exception) {
            Log.w(TAG, "unbindFromService threw", e)
        }
    }

    // ----- internals -----------------------------------------------

    private fun awaitApi(op: String): OpenPgpApi {
        if (!boundLatch.await(BIND_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
            throw BypassException.Crypto(
                "OpenKeychain AIDL service did not bind within ${BIND_TIMEOUT_SECONDS}s. " +
                    "Install OpenKeychain from F-Droid or Play, then retry $op."
            )
        }
        bindError?.let { msg ->
            throw BypassException.Crypto(
                "OpenKeychain bind failed: $msg. Install / update OpenKeychain, then retry $op."
            )
        }
        return api ?: throw BypassException.Crypto(
            "OpenKeychain bind reported success but the API handle is null; " +
                "this is a bug. Retry $op."
        )
    }

    private fun resolveResult(
        result: Intent,
        output: ByteArrayOutputStream,
        op: String,
    ): ByteArray {
        return when (
            result.getIntExtra(OpenPgpApi.RESULT_CODE, OpenPgpApi.RESULT_CODE_ERROR)
        ) {
            OpenPgpApi.RESULT_CODE_SUCCESS -> output.toByteArray()

            OpenPgpApi.RESULT_CODE_USER_INTERACTION_REQUIRED -> {
                // Phase 8.2.b happy-path only: surface the requirement
                // as an actionable error. The full PendingIntent
                // bridge is 8.2.b.ii territory.
                throw BypassException.Crypto(
                    "OpenKeychain needs user interaction for $op " +
                        "(most likely the passphrase cache expired). " +
                        "Open OpenKeychain, unlock the key, then retry. " +
                        "Tip: set OpenKeychain's passphrase cache to a long " +
                        "TTL under Settings → Password cache."
                )
            }

            OpenPgpApi.RESULT_CODE_ERROR -> {
                @Suppress("DEPRECATION")
                val error = result.getParcelableExtra<OpenPgpError>(OpenPgpApi.RESULT_ERROR)
                val msg = error?.message?.takeIf { it.isNotBlank() } ?: "unknown error"
                throw BypassException.Crypto("OpenKeychain $op failed: $msg")
            }

            else -> throw BypassException.Crypto(
                "OpenKeychain $op returned an unknown result code"
            )
        }
    }

    private companion object {
        const val TAG = "bypass-keychain"
        const val OPENKEYCHAIN_PACKAGE = "org.sufficientlysecure.keychain"
        const val BIND_TIMEOUT_SECONDS = 10L
    }
}
