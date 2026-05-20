//! Crypto abstraction. Implemented per platform: gpg subprocess on the CLI,
//! OpenKeychain (via callback interface) on Android, native-messaging host
//! relay in the browser extension. Core never speaks OpenPGP itself.

// TODO(0.5): define KeyId, SecretBytes (zeroize), CryptoError, Crypto trait.
