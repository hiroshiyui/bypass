// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.crypto

import android.app.Activity
import android.app.PendingIntent
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
 * Real `Crypto` implementation. Binds to OpenKeychain's
 * `OPEN_PGP_SERVICE` AIDL endpoint via the
 * `org.sufficientlysecure:openpgp-api` library and synchronously
 * dispatches encrypt / decrypt calls.
 *
 * **Phase 8.2.b.ii: async PendingIntent bridge.** When OpenKeychain
 * returns `RESULT_CODE_USER_INTERACTION_REQUIRED` (cold passphrase
 * cache, key picker, …), we hand the [PendingIntent] to the
 * [CryptoUiBridge] and BLOCK the calling thread on a 1-slot queue.
 * `MainActivity`, collecting on the main thread, launches the
 * `PendingIntent` via `ActivityResultLauncher`; when the user
 * dismisses OpenKeychain we deliver the [androidx.activity.result.ActivityResult]
 * back and re-execute the original API call. Bounded by
 * [MAX_INTERACTION_ROUNDS] to keep a misbehaving service from
 * looping.
 *
 * Lifetime: one instance per `BypassApplication`. `bindToService`
 * is called in `init`; the bound `OpenPgpApi` is held for the life
 * of the process. The service is unbound in [close], but
 * `Application` has no clean shutdown hook so in practice process
 * death tears the binding down.
 */
class OpenKeychainCrypto(
    context: Context,
    private val bridge: CryptoUiBridge,
) : Crypto, AutoCloseable {

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
        return runWithUiBridge(api, intent, plaintext, "encrypt")
    }

    override fun decrypt(ciphertext: ByteArray): ByteArray {
        val api = awaitApi("decrypt")
        val intent = Intent(OpenPgpApi.ACTION_DECRYPT_VERIFY)
        return runWithUiBridge(api, intent, ciphertext, "decrypt")
    }

    override fun close() {
        try {
            serviceConnection.unbindFromService()
        } catch (e: Exception) {
            Log.w(TAG, "unbindFromService threw", e)
        }
    }

    // ----- internals -----------------------------------------------

    /**
     * Run [intent] against the bound API, transparently handling up to
     * [MAX_INTERACTION_ROUNDS] of user-interaction PendingIntents.
     * Each round blocks the calling thread until the Activity returns
     * the result; the OpenPgpApi service remembers the user's
     * confirmation between rounds, so the eventual re-execute returns
     * `RESULT_CODE_SUCCESS`.
     */
    private fun runWithUiBridge(
        api: OpenPgpApi,
        intent: Intent,
        input: ByteArray,
        op: String,
    ): ByteArray {
        repeat(MAX_INTERACTION_ROUNDS) { round ->
            val output = ByteArrayOutputStream()
            val result = api.executeApi(intent, ByteArrayInputStream(input), output)
            when (
                result.getIntExtra(OpenPgpApi.RESULT_CODE, OpenPgpApi.RESULT_CODE_ERROR)
            ) {
                OpenPgpApi.RESULT_CODE_SUCCESS -> return output.toByteArray()

                OpenPgpApi.RESULT_CODE_USER_INTERACTION_REQUIRED -> {
                    @Suppress("DEPRECATION")
                    val pi = result.getParcelableExtra<PendingIntent>(OpenPgpApi.RESULT_INTENT)
                        ?: throw BypassException.Crypto(
                            "OpenKeychain asked for user interaction but didn't supply " +
                                "a PendingIntent (round $round of $op). Open a bug.",
                        )
                    val response = bridge.requestUserInteraction(pi)
                        ?: throw BypassException.Crypto(
                            "$op cancelled — OpenKeychain UI didn't return within the " +
                                "timeout, or the buffer overflowed. Try again.",
                        )
                    if (response.resultCode != Activity.RESULT_OK) {
                        throw BypassException.Crypto(
                            "$op cancelled by user (resultCode=${response.resultCode}).",
                        )
                    }
                    // Re-run executeApi at the top of the loop. The
                    // service remembers the user's confirmation
                    // internally, so the next call should succeed.
                }

                OpenPgpApi.RESULT_CODE_ERROR -> {
                    @Suppress("DEPRECATION")
                    val error = result.getParcelableExtra<OpenPgpError>(OpenPgpApi.RESULT_ERROR)
                    val msg = error?.message?.takeIf { it.isNotBlank() } ?: "unknown error"
                    throw BypassException.Crypto("OpenKeychain $op failed: $msg")
                }

                else -> throw BypassException.Crypto(
                    "OpenKeychain $op returned an unknown result code",
                )
            }
        }
        throw BypassException.Crypto(
            "$op gave up after $MAX_INTERACTION_ROUNDS user-interaction rounds. " +
                "OpenKeychain is asking for confirmation in a loop; check its state.",
        )
    }

    private fun awaitApi(op: String): OpenPgpApi {
        if (!boundLatch.await(BIND_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
            throw BypassException.Crypto(
                "OpenKeychain AIDL service did not bind within ${BIND_TIMEOUT_SECONDS}s. " +
                    "Install OpenKeychain from F-Droid or Play, then retry $op.",
            )
        }
        bindError?.let { msg ->
            throw BypassException.Crypto(
                "OpenKeychain bind failed: $msg. Install / update OpenKeychain, then retry $op.",
            )
        }
        return api ?: throw BypassException.Crypto(
            "OpenKeychain bind reported success but the API handle is null; " +
                "this is a bug. Retry $op.",
        )
    }

    private companion object {
        const val TAG = "bypass-keychain"
        const val OPENKEYCHAIN_PACKAGE = "org.sufficientlysecure.keychain"
        const val BIND_TIMEOUT_SECONDS = 10L
        const val MAX_INTERACTION_ROUNDS = 5
    }
}
