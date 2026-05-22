// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.crypto

import uniffi.bypass.Crypto
import uniffi.bypass.BypassException

/**
 * Phase 8.2.a placeholder. Throws on every call so the FFI's error
 * round-trip is observable in the UI (Snackbar surfacing) without
 * needing OpenKeychain installed.
 *
 * Phase 8.2.b replaces this with an OpenKeychain AIDL client that
 * binds to `org.sufficientlysecure.keychain.intent.OPEN_PGP_SERVICE`
 * via the `openpgp-api` library and bridges OpenKeychain's async
 * PendingIntent flow onto the synchronous UniFFI foreign-trait
 * surface. See [ADR-0025](../../../../../../../doc/adr/0025-android-ui-architecture.md).
 */
class StubCrypto : Crypto {
    override fun encrypt(plaintext: ByteArray, recipients: List<String>): ByteArray {
        throw BypassException.Crypto(
            "OpenKeychain integration lands in 8.2.b — encrypt() is stubbed in this build."
        )
    }

    override fun decrypt(ciphertext: ByteArray): ByteArray {
        throw BypassException.Crypto(
            "OpenKeychain integration lands in 8.2.b — decrypt() is stubbed in this build."
        )
    }
}
