// SPDX-License-Identifier: GPL-3.0-or-later

//! Generic RFC-4180 CSV importer. The user states their column
//! layout via `--csv-schema=<role1>,<role2>,...` — there is no
//! automagic header sniffing, per ADR-0027 (CSV exports come in
//! wildly different shapes; the wrong guess silently mis-maps the
//! whole vault).
//!
//! Role names:
//!
//! - `name`: entry name (becomes the final path segment).
//! - `folder`: forward-slash-separated subtree path; segments are
//!   slugged like any other folder.
//! - `password`: the password line.
//! - `username`: emitted as `login: <value>`.
//! - `url`: emitted as `url: <value>` (or `url-2`, `url-3`, … if
//!   multiple `url` columns are declared).
//! - `totp`: emitted as `otpauth: <value>` (already-formatted URI
//!   expected).
//! - `notes`: free-form notes body.
//! - `-` or empty: ignored column.
//! - Anything else: treated as a custom field with that header.
//!
//! Pass `--csv-has-header` to skip the file's first row.

use std::io::Read;

use thiserror::Error;

use super::{ImportedEntry, LossinessReport};
use crate::crypto::SecretBytes;

#[derive(Debug, Error)]
pub enum CsvError {
    #[error("--csv-schema is empty (need at least one role)")]
    EmptySchema,
    #[error("CSV parse: {0}")]
    Parse(#[from] ::csv::Error),
    #[error("CSV row {row} has {got} columns but the schema declares {expected}")]
    ColumnCount {
        row: usize,
        got: usize,
        expected: usize,
    },
    #[error(
        "CSV row {row} has no `name` value and the schema declares no `folder` either — entry would be unnamed"
    )]
    UnnamedRow { row: usize },
    #[error(
        "CSV row {row} has no `password` value (the schema must include `password` and rows must populate it)"
    )]
    MissingPassword { row: usize },
    #[error("--csv-schema is missing a `password` role")]
    SchemaMissingPassword,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    Name,
    Folder,
    Password,
    Username,
    Url,
    Totp,
    Notes,
    Skip,
    Field(String),
}

#[derive(Debug, Clone)]
pub struct CsvSchema {
    roles: Vec<Role>,
}

impl CsvSchema {
    /// Parse the `--csv-schema` argument value. Role names are
    /// comma-separated; whitespace around each name is trimmed.
    pub fn parse(spec: &str) -> Result<Self, CsvError> {
        let roles: Vec<Role> = spec
            .split(',')
            .map(|s| s.trim())
            .map(|s| match s.to_ascii_lowercase().as_str() {
                "name" => Role::Name,
                "folder" => Role::Folder,
                "password" => Role::Password,
                "username" | "login" => Role::Username,
                "url" | "uri" => Role::Url,
                "totp" | "otpauth" => Role::Totp,
                "notes" | "note" => Role::Notes,
                "" | "-" | "_" | "skip" => Role::Skip,
                _ => Role::Field(s.to_owned()),
            })
            .collect();
        if roles.is_empty() {
            return Err(CsvError::EmptySchema);
        }
        if !roles.iter().any(|r| matches!(r, Role::Password)) {
            return Err(CsvError::SchemaMissingPassword);
        }
        Ok(Self { roles })
    }

    pub fn column_count(&self) -> usize {
        self.roles.len()
    }
}

/// Parse a CSV stream using the supplied schema.
///
/// `has_header == true` skips the first row entirely. The schema's
/// column count must match every data row's column count exactly.
pub fn parse<R: Read>(
    reader: R,
    schema: &CsvSchema,
    has_header: bool,
) -> Result<(Vec<ImportedEntry>, LossinessReport), CsvError> {
    let mut rdr = ::csv::ReaderBuilder::new()
        .has_headers(has_header)
        // Accept whatever shape the file has — we surface the
        // column-count mismatch ourselves so the error names the
        // schema-declared column count, not just the previous row's.
        .flexible(true)
        .from_reader(reader);

    let mut entries = Vec::new();
    let mut report = LossinessReport::default();
    let expected = schema.column_count();

    // `records()` does not include the header row when `has_headers`
    // is true, so positional indexing matches the schema either way.
    for (row_idx, record) in rdr.records().enumerate() {
        // CSV row numbers are user-facing; 1-based, *including* the
        // header row when present, so the message lines up with what
        // the user sees in a spreadsheet.
        let row = if has_header { row_idx + 2 } else { row_idx + 1 };
        let record = record.map_err(CsvError::Parse)?;
        if record.len() != expected {
            return Err(CsvError::ColumnCount {
                row,
                got: record.len(),
                expected,
            });
        }
        let entry = build_entry(row, schema, &record, &mut report)?;
        entries.push(entry);
    }
    Ok((entries, report))
}

fn build_entry(
    row: usize,
    schema: &CsvSchema,
    record: &::csv::StringRecord,
    report: &mut LossinessReport,
) -> Result<ImportedEntry, CsvError> {
    let mut name = String::new();
    let mut folder: Vec<String> = Vec::new();
    let mut password: Option<String> = None;
    let mut username: Option<String> = None;
    let mut uris: Vec<String> = Vec::new();
    let mut totp: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut fields: Vec<(String, String)> = Vec::new();

    for (role, raw_value) in schema.roles.iter().zip(record.iter()) {
        let value = raw_value.trim();
        if value.is_empty() && !matches!(role, Role::Skip) {
            continue;
        }
        match role {
            Role::Name => name = value.to_owned(),
            Role::Folder => {
                folder = value
                    .split('/')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            Role::Password => password = Some(value.to_owned()),
            Role::Username => username = Some(value.to_owned()),
            Role::Url => uris.push(value.to_owned()),
            Role::Totp => totp = Some(value.to_owned()),
            Role::Notes => notes = Some(value.to_owned()),
            Role::Field(key) => fields.push((key.clone(), value.to_owned())),
            Role::Skip => {}
        }
    }

    if name.is_empty() && folder.is_empty() {
        return Err(CsvError::UnnamedRow { row });
    }
    if name.is_empty() {
        // The folder's last segment becomes the entry name. This is
        // what users expect when their CSV has only a "Service"
        // column and nothing else identifying.
        if let Some(last) = folder.pop() {
            name = last;
            report.push(
                if folder.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", folder.join("/"), name)
                },
                format!("row {row}: no `name` column populated; used last folder segment as the entry name"),
            );
        }
    }
    let password = password.ok_or(CsvError::MissingPassword { row })?;

    Ok(ImportedEntry {
        folder,
        name,
        password: SecretBytes::new(password.into_bytes()),
        username,
        fields,
        totp,
        notes,
        uris,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(entry: &ImportedEntry) -> String {
        String::from_utf8(entry.password.as_slice().to_vec()).unwrap()
    }

    #[test]
    fn schema_parses_known_roles() {
        let s = CsvSchema::parse("name,folder,password,username,url,totp,notes,-").unwrap();
        assert_eq!(s.column_count(), 8);
    }

    #[test]
    fn schema_requires_password() {
        let err = CsvSchema::parse("name,username").unwrap_err();
        assert!(matches!(err, CsvError::SchemaMissingPassword));
    }

    #[test]
    fn schema_unknown_role_becomes_custom_field() {
        let s = CsvSchema::parse("password,Recovery").unwrap();
        match &s.roles[1] {
            Role::Field(k) => assert_eq!(k, "Recovery"),
            other => panic!("expected Field, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_simple_csv() {
        let schema = CsvSchema::parse("name,username,password,url").unwrap();
        let csv = "GitHub,alice,p4ss,https://github.com\nTwitter,alice,p4ss2,https://twitter.com\n";
        let (entries, report) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert!(report.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "GitHub");
        assert_eq!(entries[0].username.as_deref(), Some("alice"));
        assert_eq!(body(&entries[0]), "p4ss");
        assert_eq!(entries[0].uris, vec!["https://github.com"]);
        assert_eq!(entries[1].name, "Twitter");
    }

    #[test]
    fn header_row_is_skipped_when_requested() {
        let schema = CsvSchema::parse("name,password").unwrap();
        let csv = "Service,Secret\nGitHub,p4ss\n";
        let (entries, _) = parse(csv.as_bytes(), &schema, true).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "GitHub");
    }

    #[test]
    fn folder_column_splits_on_slash() {
        let schema = CsvSchema::parse("folder,name,password").unwrap();
        let csv = "Personal/Email,Gmail,p\n";
        let (entries, _) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert_eq!(entries[0].folder, vec!["Personal", "Email"]);
        assert_eq!(entries[0].name, "Gmail");
    }

    #[test]
    fn multiple_url_columns_become_multiple_uris() {
        let schema = CsvSchema::parse("name,password,url,url").unwrap();
        let csv = "x,p,https://a,https://b\n";
        let (entries, _) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert_eq!(entries[0].uris, vec!["https://a", "https://b"]);
    }

    #[test]
    fn custom_field_columns_become_kv_pairs() {
        let schema = CsvSchema::parse("name,password,Recovery,Pin").unwrap();
        let csv = "x,p,kitten,1234\n";
        let (entries, _) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert_eq!(
            entries[0].fields,
            vec![
                ("Recovery".into(), "kitten".into()),
                ("Pin".into(), "1234".into()),
            ]
        );
    }

    #[test]
    fn column_count_mismatch_is_reported_with_row_number() {
        let schema = CsvSchema::parse("name,password").unwrap();
        let csv = "ok,p\ntoo,many,fields\n";
        let err = parse(csv.as_bytes(), &schema, false).unwrap_err();
        match err {
            CsvError::ColumnCount { row, got, expected } => {
                assert_eq!(row, 2);
                assert_eq!(got, 3);
                assert_eq!(expected, 2);
            }
            other => panic!("expected ColumnCount, got {other:?}"),
        }
    }

    #[test]
    fn missing_password_value_is_rejected_with_row_number() {
        let schema = CsvSchema::parse("name,password").unwrap();
        let csv = "x,\n";
        let err = parse(csv.as_bytes(), &schema, false).unwrap_err();
        assert!(matches!(err, CsvError::MissingPassword { row: 1 }));
    }

    #[test]
    fn name_falls_back_to_last_folder_segment_with_lossiness() {
        let schema = CsvSchema::parse("folder,password").unwrap();
        let csv = "Personal/Email,p\n";
        let (entries, report) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert_eq!(entries[0].folder, vec!["Personal"]);
        assert_eq!(entries[0].name, "Email");
        assert_eq!(report.notes.len(), 1);
    }

    #[test]
    fn skip_role_drops_the_column_silently() {
        let schema = CsvSchema::parse("name,-,password").unwrap();
        let csv = "x,ignored,p\n";
        let (entries, report) = parse(csv.as_bytes(), &schema, false).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(report.is_empty());
        assert!(entries[0].fields.is_empty());
    }
}
