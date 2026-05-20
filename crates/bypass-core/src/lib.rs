// SPDX-License-Identifier: GPL-3.0-or-later

//! Platform-agnostic core for the `bypass` password manager.
//!
//! This crate defines the business logic and platform-abstraction traits:
//! [`crypto::Crypto`] for OpenPGP encrypt/decrypt, [`storage::Storage`] for
//! reading and writing blobs, and [`vcs::VersionControl`] for optional git
//! history. Concrete implementations live in the platform crates
//! (`bypass-cli`, future `bypass-ffi`, etc.).

pub mod crypto;
pub mod entry;
pub mod error;
pub mod generate;
pub mod gpg_id;
pub mod otp;
pub mod path;
pub mod storage;
pub mod store;
pub mod vcs;
