// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.crypto

import android.app.PendingIntent
import androidx.activity.result.ActivityResult
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import java.util.Optional
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Bridges OpenKeychain's async `PendingIntent`-driven user-interaction
 * flow onto the synchronous `Crypto` foreign-trait surface UniFFI
 * generates.
 *
 * The flow is roughly:
 *
 * 1. [OpenKeychainCrypto.encrypt] (or `decrypt`) runs on
 *    `Dispatchers.IO` and calls `OpenPgpApi.executeApi`.
 * 2. OpenKeychain returns `RESULT_CODE_USER_INTERACTION_REQUIRED`
 *    with a [PendingIntent] in `RESULT_INTENT`.
 * 3. [OpenKeychainCrypto] calls [requestUserInteraction], which
 *    [tryEmit][MutableSharedFlow.tryEmit]s a [CryptoUiRequest] onto
 *    [requests] and BLOCKS the IO thread on a 1-slot
 *    [LinkedBlockingQueue].
 * 4. `MainActivity` (collecting on the main thread) sees the
 *    request, launches the [PendingIntent] via an
 *    `ActivityResultLauncher`, and when the user finishes the
 *    OpenKeychain UI it calls [CryptoUiRequest.deliver] with the
 *    [ActivityResult].
 * 5. The IO thread's `poll` unblocks; [OpenKeychainCrypto]
 *    re-executes the original `executeApi` call, which now returns
 *    `RESULT_CODE_SUCCESS` because OpenKeychain remembers the
 *    user's confirmation.
 *
 * Concurrency: the [SharedFlow] is FIFO-ordered with a buffer of 16
 * (more than enough — encrypt/decrypt are called one at a time per
 * UI action). The `MainActivity` collector uses a [java.util.Queue]
 * to pair each result back to its originating request.
 */
class CryptoUiBridge {

    private val _requests = MutableSharedFlow<CryptoUiRequest>(extraBufferCapacity = 16)
    val requests: SharedFlow<CryptoUiRequest> = _requests.asSharedFlow()

    /**
     * Block the calling thread until the UI dance for [pendingIntent]
     * completes, or [TIMEOUT_SECONDS] elapses.
     *
     * Returns `null` if the user cancelled or the timeout fired.
     * Returns an [ActivityResult] (which may itself carry
     * `RESULT_CANCELED`) if the launcher's callback fired in time.
     */
    fun requestUserInteraction(pendingIntent: PendingIntent): ActivityResult? {
        val box = LinkedBlockingQueue<Optional<ActivityResult>>(1)
        val request = CryptoUiRequest(pendingIntent, box)
        if (!_requests.tryEmit(request)) {
            // SharedFlow buffer is sized for 16 outstanding requests;
            // hitting this means we have a runaway loop. Surface as a
            // cancel rather than blocking forever.
            return null
        }
        val slot = box.poll(TIMEOUT_SECONDS, TimeUnit.SECONDS)
        return slot?.orElse(null)
    }

    private companion object {
        const val TIMEOUT_SECONDS = 90L
    }
}

/**
 * One outstanding user-interaction round. The Activity collects
 * [pendingIntent], launches it via `ActivityResultLauncher`, and
 * calls [deliver] with the result.
 */
class CryptoUiRequest internal constructor(
    val pendingIntent: PendingIntent,
    private val box: LinkedBlockingQueue<Optional<ActivityResult>>,
) {
    /** Hand the [ActivityResult] (or `null` for cancellation) back
     *  to the blocked IO thread. Non-blocking; safe to call from the
     *  main thread. Idempotent: only the first call wins. */
    fun deliver(result: ActivityResult?) {
        box.offer(Optional.ofNullable(result))
    }
}
