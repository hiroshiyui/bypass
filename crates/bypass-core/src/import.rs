// SPDX-License-Identifier: GPL-3.0-or-later

//! Foreign-format importers (Milestone 4.5 / [ADR-0027](../../../doc/adr/0027-foreign-format-importers.md)).
//!
//! The CLI's `bypass import --format=<name>` verb funnels every
//! foreign vault — Bitwarden, KeePass, generic CSV — through one
//! write path. This module owns the **format-agnostic** pieces:
//!
//! - [`ImportedEntry`]: a parser's output shape. Each first-party
//!   parser (`bitwarden`, `keepass`, `csv`) produces a `Vec<ImportedEntry>`;
//!   extensions (via `bypass import --from-ext`) instead emit an
//!   ADR-0026 bundle.
//! - [`prepare`]: turns a parser's batch into [`PreparedEntry`] values
//!   ready to hand to [`crate::store::Store::insert_no_commit`].
//!   Applies the canonical mapping rules (slugging, key conventions,
//!   in-batch collision suffixing, atomic-fail on store collisions).
//! - [`LossinessReport`]: the mandatory stderr summary surface.
//!
//! Submodules:
//!
//! - [`bitwarden`]: plain-JSON export (`bitwarden_export.json`).
//! - [`csv`]: RFC-4180 with an explicit `--csv-schema` column mapping.
//! - (KeePass KDBX-XML lands in a follow-up commit; the extension
//!   path covers anything else.)

use crate::crypto::SecretBytes;
use crate::path::RelPath;

pub mod bitwarden;
pub mod csv;
pub mod keepass;

/// One entry as it comes out of a foreign-format parser, before
/// slugging, collision resolution, or serialisation to the bypass
/// entry body.
#[derive(Debug)]
pub struct ImportedEntry {
    /// Folder / group path components, in source-vault order. May
    /// be empty (item at the source's root).
    pub folder: Vec<String>,
    /// Display name from the source vault.
    pub name: String,
    /// The password itself.
    pub password: SecretBytes,
    pub username: Option<String>,
    /// Extra `key: value` lines. Order is preserved.
    pub fields: Vec<(String, String)>,
    /// Already-formatted `otpauth://totp/...` URI, if any.
    pub totp: Option<String>,
    /// Free-form notes; rendered last in the entry body.
    pub notes: Option<String>,
    /// Associated URLs / URIs. The first one becomes `url:`; the
    /// rest become `url-2:`, `url-3:`, …
    pub uris: Vec<String>,
}

/// A single line in the [`LossinessReport`] — one fact the importer
/// transformed or dropped, scoped to a specific entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossinessNote {
    /// Friendly identifier (the eventual entry path) so the user can
    /// find the entry in the imported store. Empty string is fine
    /// when the note applies before path resolution.
    pub entry: String,
    pub message: String,
}

/// The stderr summary the CLI prints at the end of every import.
/// Importers append to this as they work; [`prepare`] folds in the
/// notes it generates itself (collision suffixing, newline
/// flattening).
#[derive(Debug, Default)]
pub struct LossinessReport {
    pub notes: Vec<LossinessNote>,
}

impl LossinessReport {
    pub fn push(&mut self, entry: impl Into<String>, message: impl Into<String>) {
        self.notes.push(LossinessNote {
            entry: entry.into(),
            message: message.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }
}

/// An [`ImportedEntry`] after canonical mapping: a [`RelPath`] the
/// store can index by, and the UTF-8 entry body the CLI will encrypt
/// and write.
#[derive(Debug)]
pub struct PreparedEntry {
    pub path: RelPath,
    pub body: SecretBytes,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// At least one slugged path collides with an existing entry in
    /// the destination store. The import is refused atomically — no
    /// entries are written. Carries every collision so the user
    /// fixes them in one pass.
    #[error(
        "{} entr{} collide with paths already in the destination store: {}",
        .0.len(),
        if .0.len() == 1 { "y" } else { "ies" },
        .0.join(", "),
    )]
    StoreCollision(Vec<String>),

    /// A source-vault entry has no name *and* no folder, so it
    /// would slug to the empty string. The parser should have
    /// caught this, but defend in depth.
    #[error("entry #{0} from the source has no usable name (folder + name both empty/unprintable)")]
    UnnamedEntry(usize),
}

/// Canonical mapping: turn parser output into bypass-ready
/// [`PreparedEntry`] values. Atomic: returns [`ImportError::StoreCollision`]
/// if any slugged path already exists in `existing_entries`, without
/// preparing anything.
///
/// `existing_entries` is the destination store's current entry list
/// (output of [`crate::store::Store::list`]). Pass an empty slice when
/// importing into a fresh store.
pub fn prepare(
    entries: Vec<ImportedEntry>,
    existing_entries: &[RelPath],
) -> Result<(Vec<PreparedEntry>, LossinessReport), ImportError> {
    let mut report = LossinessReport::default();

    // First pass: build each entry's desired path by joining its
    // folder components and name with `/`, then slugging the whole
    // string. Per ADR-0027, `/` is preserved by the slugger — a
    // CSV row with `name = "Email/Work"` produces a two-segment
    // path `email/work`.
    let mut desired: Vec<RelPath> = Vec::with_capacity(entries.len());
    for (idx, e) in entries.iter().enumerate() {
        let raw = e
            .folder
            .iter()
            .chain(std::iter::once(&e.name))
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let slugged = slug_path(&raw);
        if slugged.is_empty() {
            return Err(ImportError::UnnamedEntry(idx));
        }
        // Slug output is lowercase ASCII + `._-/`, with no leading
        // or trailing `/` and no `//` runs — that's a valid RelPath
        // by construction.
        let path = RelPath::new(slugged).expect("slug output is always a valid RelPath");
        desired.push(path);
    }

    // Second pass: resolve in-batch collisions (suffix `-2`, `-3`,
    // ...). For each slugged path, count how many earlier entries
    // already claimed it; suffix accordingly.
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut final_paths: Vec<RelPath> = Vec::with_capacity(entries.len());
    for (idx, base) in desired.iter().enumerate() {
        let n = counts.entry(base.as_str().to_owned()).or_insert(0);
        *n += 1;
        let resolved = if *n == 1 {
            base.clone()
        } else {
            let suffixed = format!("{}-{}", base.as_str(), n);
            let resolved = RelPath::new(suffixed.clone())
                .expect("slug + dash + digits is always a valid RelPath");
            report.push(
                resolved.as_str().to_owned(),
                format!(
                    "in-batch path collision with {}: suffixed to disambiguate",
                    base.as_str(),
                ),
            );
            let _ = idx; // silence the unused-binding lint without changing arity
            resolved
        };
        final_paths.push(resolved);
    }

    // Third pass: detect *store-side* collisions. Atomic-fail per
    // ADR-0027 — never partial-apply.
    let existing: std::collections::HashSet<&str> =
        existing_entries.iter().map(|p| p.as_str()).collect();
    let mut collisions: Vec<String> = final_paths
        .iter()
        .filter(|p| existing.contains(p.as_str()))
        .map(|p| p.as_str().to_owned())
        .collect();
    if !collisions.is_empty() {
        collisions.sort();
        collisions.dedup();
        return Err(ImportError::StoreCollision(collisions));
    }

    // Fourth pass: serialise the entry body. This is where most
    // lossiness happens (embedded newlines in field values get
    // flattened to `\n`).
    let mut prepared: Vec<PreparedEntry> = Vec::with_capacity(entries.len());
    for (entry, path) in entries.into_iter().zip(final_paths.into_iter()) {
        let body = serialise_body(&entry, path.as_str(), &mut report);
        prepared.push(PreparedEntry { path, body });
    }

    Ok((prepared, report))
}

/// Slug a path (or a single segment) per ADR-0027 mapping rules:
///
/// 1. Lowercase.
/// 2. Whitespace runs collapse to a single `-`.
/// 3. Strip characters outside `[a-z0-9._-/]`.
/// 4. Collapse repeated `-` and repeated `/`.
/// 5. Trim leading/trailing `-` and `/`.
/// 6. Drop any segment that became empty (so `Foo//Bar` → `foo/bar`).
///
/// `/` is preserved — a CSV name like `"Email/Work"` (with no folder
/// column) still produces a two-segment path. Slug per-segment by
/// passing a string with no `/`.
pub fn slug_path(s: &str) -> String {
    // First pass: character-class strip + lowercase + whitespace→`-`.
    let mut buf = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            buf.push(c.to_ascii_lowercase());
        } else if c == '.' || c == '_' || c == '-' || c == '/' {
            buf.push(c);
        } else if c.is_whitespace() {
            buf.push('-');
        }
        // else: silently strip.
    }
    // Second pass: per-segment normalisation. Collapse `-` runs and
    // trim leading/trailing `-` within each segment; drop segments
    // that became empty.
    let mut segments: Vec<String> = Vec::new();
    for seg in buf.split('/') {
        let mut normed = String::with_capacity(seg.len());
        let mut last_dash = false;
        for c in seg.chars() {
            if c == '-' {
                if last_dash {
                    continue;
                }
                last_dash = true;
            } else {
                last_dash = false;
            }
            normed.push(c);
        }
        let trimmed = normed.trim_matches('-').to_owned();
        if !trimmed.is_empty() {
            segments.push(trimmed);
        }
    }
    segments.join("/")
}

/// Render an [`ImportedEntry`] into the bypass entry body
/// (`password\nkey: value\n...`).
///
/// Newlines inside `username` and field values are flattened to the
/// literal two characters `\n`; the lossiness is noted in `report`
/// against `path` so the user sees which entry was affected.
fn serialise_body(entry: &ImportedEntry, path: &str, report: &mut LossinessReport) -> SecretBytes {
    let mut body = String::new();
    // First line: the password as-is. Bytes flow through `SecretBytes`
    // so we don't UTF-8-validate the raw plaintext — but we do need a
    // `String` here so we can append `key: value` lines. Lossy decode
    // is acceptable: foreign vaults' password fields are UTF-8 in
    // every format we ship for; a non-UTF-8 byte sequence indicates
    // a malformed source vault, which the CLI surfaces via the
    // lossiness report.
    let password_str = String::from_utf8_lossy(entry.password.as_slice());
    if password_str.contains('\u{FFFD}') {
        report.push(
            path,
            "password contained non-UTF-8 bytes; replaced with U+FFFD",
        );
    }
    // Flatten newlines in the password line too — if the source
    // smuggled a `\n` into the password field, our parser would
    // already need to have split the entry. Defend anyway.
    if password_str.contains('\n') {
        report.push(
            path,
            "password contained newlines; flattened to `\\n` to keep the entry parseable",
        );
    }
    body.push_str(&flatten_newlines(&password_str));
    body.push('\n');

    if let Some(username) = &entry.username {
        push_kv(&mut body, "login", username, path, report);
    }

    for (k, v) in &entry.fields {
        // Skip empty keys defensively; the user can't address a `: value` line.
        if k.trim().is_empty() {
            report.push(path, "field with empty key dropped");
            continue;
        }
        push_kv(&mut body, k.trim(), v, path, report);
    }

    if let Some(totp) = &entry.totp {
        push_kv(&mut body, "otpauth", totp, path, report);
    }

    for (i, uri) in entry.uris.iter().enumerate() {
        let key = if i == 0 {
            "url".to_owned()
        } else {
            format!("url-{}", i + 1)
        };
        push_kv(&mut body, &key, uri, path, report);
    }

    if let Some(notes) = &entry.notes
        && !notes.is_empty()
    {
        // Notes go last after a blank line, free-form (newlines OK).
        body.push('\n');
        body.push_str(notes);
        if !notes.ends_with('\n') {
            body.push('\n');
        }
    }

    SecretBytes::new(body.into_bytes())
}

fn push_kv(body: &mut String, key: &str, value: &str, path: &str, report: &mut LossinessReport) {
    if value.contains('\n') {
        report.push(
            path,
            format!("field {key:?} contained newlines; flattened to `\\n`"),
        );
    }
    body.push_str(key);
    body.push_str(": ");
    body.push_str(&flatten_newlines(value));
    body.push('\n');
}

fn flatten_newlines(s: &str) -> String {
    s.replace('\n', "\\n").replace('\r', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sb(s: &str) -> SecretBytes {
        SecretBytes::new(s.as_bytes().to_vec())
    }

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn entry(folder: &[&str], name: &str, password: &str) -> ImportedEntry {
        ImportedEntry {
            folder: folder.iter().map(|s| s.to_string()).collect(),
            name: name.to_string(),
            password: sb(password),
            username: None,
            fields: Vec::new(),
            totp: None,
            notes: None,
            uris: Vec::new(),
        }
    }

    #[test]
    fn slug_path_basic_cases() {
        assert_eq!(slug_path("Email"), "email");
        assert_eq!(slug_path("My Bank"), "my-bank");
        assert_eq!(slug_path("Étoile  café"), "toile-caf");
        assert_eq!(slug_path("---weird---"), "weird");
        assert_eq!(slug_path(""), "");
        assert_eq!(slug_path("   "), "");
        assert_eq!(slug_path("Foo.Bar_Baz-Qux"), "foo.bar_baz-qux");
    }

    #[test]
    fn slug_path_preserves_segment_boundaries() {
        // Per ADR-0027: `/` is part of the allowed alphabet; a name
        // like "Email/Work" stays a two-segment path. Repeated `/`
        // and empty segments collapse.
        assert_eq!(slug_path("Email/Work"), "email/work");
        assert_eq!(slug_path("Personal/Email/Gmail"), "personal/email/gmail");
        assert_eq!(slug_path("//foo//bar//"), "foo/bar");
        assert_eq!(slug_path("/leading/and/trailing/"), "leading/and/trailing");
        assert_eq!(slug_path("a / b"), "a/b");
    }

    #[test]
    fn prepare_simple_entry_produces_password_line() {
        let entries = vec![entry(&["Personal"], "Email", "hunter2")];
        let (prepared, report) = prepare(entries, &[]).unwrap();
        assert!(report.is_empty());
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].path, rp("personal/email"));
        assert_eq!(prepared[0].body.as_slice(), b"hunter2\n");
    }

    #[test]
    fn prepare_full_entry_emits_canonical_layout() {
        let entries = vec![ImportedEntry {
            folder: vec!["Banking".into()],
            name: "Chase".into(),
            password: sb("p4ss"),
            username: Some("alice".into()),
            fields: vec![
                ("recovery".into(), "kitten".into()),
                ("pin".into(), "1234".into()),
            ],
            totp: Some("otpauth://totp/Chase:alice?secret=ABC".into()),
            notes: Some("Used for the joint account.".into()),
            uris: vec![
                "https://chase.example".into(),
                "https://chase.example/mobile".into(),
            ],
        }];
        let (prepared, report) = prepare(entries, &[]).unwrap();
        assert!(report.is_empty(), "no lossiness expected");
        assert_eq!(prepared[0].path, rp("banking/chase"));
        let body = std::str::from_utf8(prepared[0].body.as_slice()).unwrap();
        assert_eq!(
            body,
            concat!(
                "p4ss\n",
                "login: alice\n",
                "recovery: kitten\n",
                "pin: 1234\n",
                "otpauth: otpauth://totp/Chase:alice?secret=ABC\n",
                "url: https://chase.example\n",
                "url-2: https://chase.example/mobile\n",
                "\n",
                "Used for the joint account.\n",
            ),
        );
    }

    #[test]
    fn prepare_flattens_newlines_in_field_values_and_reports() {
        let entries = vec![ImportedEntry {
            folder: vec!["work".into()],
            name: "vpn".into(),
            password: sb("p"),
            username: None,
            fields: vec![("config".into(), "line1\nline2".into())],
            totp: None,
            notes: None,
            uris: Vec::new(),
        }];
        let (prepared, report) = prepare(entries, &[]).unwrap();
        let body = std::str::from_utf8(prepared[0].body.as_slice()).unwrap();
        assert!(body.contains("config: line1\\nline2\n"));
        assert_eq!(report.len(), 1);
        assert!(report.notes[0].message.contains("flattened"));
        assert_eq!(report.notes[0].entry, "work/vpn");
    }

    #[test]
    fn in_batch_collision_is_suffixed_and_reported() {
        let entries = vec![
            entry(&["e"], "work", "v1"),
            entry(&["e"], "work", "v2"),
            entry(&["e"], "work", "v3"),
        ];
        let (prepared, report) = prepare(entries, &[]).unwrap();
        assert_eq!(prepared[0].path, rp("e/work"));
        assert_eq!(prepared[1].path, rp("e/work-2"));
        assert_eq!(prepared[2].path, rp("e/work-3"));
        // Two collision-suffix notes (the second and third entries).
        assert_eq!(report.len(), 2);
    }

    #[test]
    fn store_collision_fails_atomically() {
        let existing = vec![rp("email/work")];
        let entries = vec![
            entry(&["email"], "work", "v"),
            entry(&["email"], "personal", "v"),
        ];
        let err = prepare(entries, &existing).unwrap_err();
        match err {
            ImportError::StoreCollision(paths) => {
                assert_eq!(paths, vec!["email/work".to_owned()]);
            }
            other => panic!("expected StoreCollision, got {other:?}"),
        }
    }

    #[test]
    fn unnamed_entry_is_rejected() {
        let entries = vec![entry(&[], "", "p")];
        let err = prepare(entries, &[]).unwrap_err();
        assert!(matches!(err, ImportError::UnnamedEntry(0)));
    }

    #[test]
    fn empty_folder_segments_are_dropped() {
        // A vault that emits `["", "Email", ""]` as the folder path
        // should yield just `email/...` not `//email//`.
        let entries = vec![entry(&["", "Email", ""], "Work", "p")];
        let (prepared, _) = prepare(entries, &[]).unwrap();
        assert_eq!(prepared[0].path, rp("email/work"));
    }
}
