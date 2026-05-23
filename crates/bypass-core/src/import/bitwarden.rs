// SPDX-License-Identifier: GPL-3.0-or-later

//! Bitwarden plain-JSON export parser (`bitwarden_export.json` /
//! the unencrypted variant produced by `Tools → Export Vault` in
//! the web vault, desktop, or mobile apps with `"encrypted": false`).
//!
//! Supported item types:
//!
//! - `type == 1` (login) — full support: name, folder, username,
//!   password, URIs, TOTP, free-form notes, custom fields.
//! - `type == 2` (secure note) — name → entry name, notes → notes
//!   body, custom fields → `key: value` lines. No `password` field
//!   in Bitwarden's schema, so we synthesise an empty first line so
//!   the resulting entry still parses; the lossiness report flags
//!   it.
//!
//! Out of scope (each surface a lossiness note and are *skipped*):
//!
//! - `type == 3` (card) — no clean mapping to a single-password
//!   entry.
//! - `type == 4` (identity) — same.
//! - Item attachments — not part of the JSON export schema.
//! - Encrypted exports (`"encrypted": true`) — need master-password
//!   decryption; covered by a follow-up sub-milestone.
//!
//! See ADR-0027 for the canonical mapping rules every importer
//! follows.

use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

use super::{ImportedEntry, LossinessReport};
use crate::crypto::SecretBytes;

#[derive(Debug, Error)]
pub enum BitwardenError {
    #[error("not a Bitwarden export: {0}")]
    NotBitwarden(String),

    #[error(
        "Bitwarden encrypted exports are not supported by this version of `bypass` — re-export with `Tools → Export Vault` and set the file password to *empty* to produce a plain-JSON export"
    )]
    Encrypted,

    #[error("Bitwarden JSON parse: {0}")]
    Parse(#[from] serde_json::Error),
}

// ===== wire shape =====================================================

#[derive(Debug, Deserialize)]
struct Export {
    #[serde(default)]
    encrypted: bool,
    #[serde(default)]
    folders: Vec<Folder>,
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
struct Folder {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Item {
    #[serde(rename = "type")]
    item_type: u32,
    name: Option<String>,
    #[serde(rename = "folderId", default)]
    folder_id: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    fields: Vec<Field>,
    #[serde(default)]
    login: Option<Login>,
}

#[derive(Debug, Deserialize)]
struct Login {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    uris: Vec<Uri>,
}

#[derive(Debug, Deserialize)]
struct Uri {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Field {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(rename = "type", default)]
    field_type: u32,
}

// ===== public surface =================================================

/// Parse a Bitwarden plain-JSON export. Returns the parsed entries
/// plus any lossiness notes the parser itself produced (the canonical
/// mapping in [`super::prepare`] adds more on top — slugging
/// collisions, newline flattening — so the CLI's final report is the
/// concatenation).
pub fn parse(bytes: &[u8]) -> Result<(Vec<ImportedEntry>, LossinessReport), BitwardenError> {
    let export: Export = serde_json::from_slice(bytes).map_err(BitwardenError::Parse)?;
    if export.encrypted {
        return Err(BitwardenError::Encrypted);
    }
    if !looks_like_bitwarden(&export) {
        return Err(BitwardenError::NotBitwarden(
            "JSON parsed but has neither `folders` nor `items` — not a Bitwarden export".into(),
        ));
    }

    let folder_names: HashMap<String, String> =
        export.folders.into_iter().map(|f| (f.id, f.name)).collect();

    let mut entries = Vec::with_capacity(export.items.len());
    let mut report = LossinessReport::default();

    for item in export.items.into_iter() {
        let name = item.name.unwrap_or_default();
        let folder = item
            .folder_id
            .as_deref()
            .and_then(|id| folder_names.get(id))
            .map(|s| s.split('/').map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();

        let entry_id = if !name.is_empty() {
            name.clone()
        } else {
            "<unnamed>".to_owned()
        };

        match item.item_type {
            1 => {
                // Login.
                let login = item.login.unwrap_or(Login {
                    username: None,
                    password: None,
                    totp: None,
                    uris: Vec::new(),
                });
                let password_text = login.password.unwrap_or_default();
                let uris: Vec<String> = login
                    .uris
                    .into_iter()
                    .filter_map(|u| u.uri)
                    .filter(|u| !u.is_empty())
                    .collect();
                let (fields, dropped_linked) = collect_fields(&item.fields);
                if dropped_linked > 0 {
                    report.push(
                        entry_id.clone(),
                        format!(
                            "dropped {dropped_linked} \"linked\" custom field{} (Bitwarden type-3 — no static value to import)",
                            if dropped_linked == 1 { "" } else { "s" },
                        ),
                    );
                }
                entries.push(ImportedEntry {
                    folder,
                    name,
                    password: SecretBytes::new(password_text.into_bytes()),
                    username: login.username.filter(|s| !s.is_empty()),
                    fields,
                    totp: login.totp.filter(|s| !s.is_empty()),
                    notes: item.notes.filter(|s| !s.is_empty()),
                    uris,
                });
            }
            2 => {
                // Secure note. No password field; emit an empty one
                // so the resulting bypass entry has a parseable first
                // line, and flag in the lossiness report.
                let (fields, dropped_linked) = collect_fields(&item.fields);
                if dropped_linked > 0 {
                    report.push(
                        entry_id.clone(),
                        format!(
                            "dropped {dropped_linked} \"linked\" custom field{} (Bitwarden type-3)",
                            if dropped_linked == 1 { "" } else { "s" },
                        ),
                    );
                }
                report.push(
                    entry_id.clone(),
                    "secure-note imported with an empty password line (Bitwarden notes have no password field)",
                );
                entries.push(ImportedEntry {
                    folder,
                    name,
                    password: SecretBytes::new(Vec::new()),
                    username: None,
                    fields,
                    totp: None,
                    notes: item.notes.filter(|s| !s.is_empty()),
                    uris: Vec::new(),
                });
            }
            3 | 4 => {
                report.push(
                    entry_id,
                    format!(
                        "skipped Bitwarden type-{} item (cards and identities don't map cleanly to a password entry; export them manually if you need them)",
                        item.item_type,
                    ),
                );
            }
            other => {
                report.push(
                    entry_id,
                    format!("skipped Bitwarden item with unknown type {other}"),
                );
            }
        }
    }

    Ok((entries, report))
}

fn looks_like_bitwarden(export: &Export) -> bool {
    !export.items.is_empty() || !export.folders.is_empty()
}

/// Collect Bitwarden custom fields into our `(name, value)` shape.
/// Returns the field list plus the count of "linked" (type-3) fields
/// dropped — the caller surfaces this in the lossiness report against
/// the entry name.
fn collect_fields(input: &[Field]) -> (Vec<(String, String)>, usize) {
    let mut out = Vec::with_capacity(input.len());
    let mut dropped_linked = 0;
    for f in input {
        if f.field_type == 3 {
            dropped_linked += 1;
            continue;
        }
        let Some(name) = &f.name else { continue };
        if name.trim().is_empty() {
            continue;
        }
        let value = f.value.clone().unwrap_or_default();
        out.push((name.clone(), value));
    }
    (out, dropped_linked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(entry: &ImportedEntry) -> String {
        // Convenience: render only the password slice for asserts.
        String::from_utf8(entry.password.as_slice().to_vec()).unwrap_or_default()
    }

    #[test]
    fn parses_minimal_login_item() {
        let json = br#"{
            "encrypted": false,
            "folders": [],
            "items": [
                {
                    "type": 1,
                    "name": "GitHub",
                    "login": {
                        "username": "alice",
                        "password": "p4ss",
                        "uris": [{"uri": "https://github.com"}]
                    }
                }
            ]
        }"#;
        let (entries, report) = parse(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(report.is_empty());
        let e = &entries[0];
        assert_eq!(e.name, "GitHub");
        assert_eq!(e.folder, Vec::<String>::new());
        assert_eq!(e.username.as_deref(), Some("alice"));
        assert_eq!(body(e), "p4ss");
        assert_eq!(e.uris, vec!["https://github.com"]);
    }

    #[test]
    fn maps_folder_id_to_folder_name_split_on_slash() {
        let json = br#"{
            "encrypted": false,
            "folders": [
                {"id": "f1", "name": "Personal/Email"}
            ],
            "items": [
                {
                    "type": 1,
                    "name": "Gmail",
                    "folderId": "f1",
                    "login": {"username": "u", "password": "p"}
                }
            ]
        }"#;
        let (entries, _) = parse(json).unwrap();
        assert_eq!(entries[0].folder, vec!["Personal", "Email"]);
    }

    #[test]
    fn carries_totp_uri_when_present() {
        let json = br#"{
            "encrypted": false,
            "items": [
                {
                    "type": 1,
                    "name": "x",
                    "login": {"password": "p", "totp": "otpauth://totp/x?secret=ABC"}
                }
            ]
        }"#;
        let (entries, _) = parse(json).unwrap();
        assert_eq!(
            entries[0].totp.as_deref(),
            Some("otpauth://totp/x?secret=ABC")
        );
    }

    #[test]
    fn carries_custom_fields_and_drops_linked() {
        let json = br#"{
            "encrypted": false,
            "items": [
                {
                    "type": 1,
                    "name": "x",
                    "login": {"password": "p"},
                    "fields": [
                        {"name": "recovery", "value": "kitten", "type": 0},
                        {"name": "secret", "value": "hidden-val", "type": 1},
                        {"name": "linked", "value": null, "type": 3}
                    ]
                }
            ]
        }"#;
        let (entries, report) = parse(json).unwrap();
        assert_eq!(
            entries[0].fields,
            vec![
                ("recovery".into(), "kitten".into()),
                ("secret".into(), "hidden-val".into()),
            ]
        );
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].message.contains("linked"));
    }

    #[test]
    fn secure_note_yields_empty_password_with_lossiness() {
        let json = br#"{
            "encrypted": false,
            "items": [
                {
                    "type": 2,
                    "name": "Wifi password",
                    "notes": "ssid: foo\nkey: bar"
                }
            ]
        }"#;
        let (entries, report) = parse(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(body(&entries[0]).is_empty());
        assert_eq!(entries[0].notes.as_deref(), Some("ssid: foo\nkey: bar"),);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.message.contains("empty password line"))
        );
    }

    #[test]
    fn cards_and_identities_are_skipped_with_lossiness() {
        let json = br#"{
            "encrypted": false,
            "items": [
                {"type": 3, "name": "Visa"},
                {"type": 4, "name": "Passport"}
            ]
        }"#;
        let (entries, report) = parse(json).unwrap();
        assert!(entries.is_empty());
        assert_eq!(report.notes.len(), 2);
    }

    #[test]
    fn refuses_encrypted_export() {
        let json = br#"{"encrypted": true, "items": []}"#;
        let err = parse(json).unwrap_err();
        assert!(matches!(err, BitwardenError::Encrypted));
    }

    #[test]
    fn refuses_unrelated_json() {
        let json = br#"{"hello": "world"}"#;
        let err = parse(json).unwrap_err();
        assert!(matches!(err, BitwardenError::NotBitwarden(_)));
    }
}
