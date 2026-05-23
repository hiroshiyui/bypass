// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass import` driver — foreign-vault importers (ADR-0027,
//! amended by ADR-0029 for the extension wire format).
//!
//! Three entry points share one write path:
//!
//! - `--format=bitwarden` → [`bypass_core::import::bitwarden::parse`]
//! - `--format=csv` → [`bypass_core::import::csv::parse`]
//! - `--format=keepass` → [`bypass_core::import::keepass::parse`]
//! - `--from-ext <name>` → spawn `bypass-import-<name>`, read NDJSON
//!   `ImportedEntry` records from its stdout, parse each line. The
//!   wire shape is the [ADR-0029] amendment to ADR-0027.
//!
//! Whichever side produced the entries, they funnel through the
//! same [`bypass_core::import::prepare`] (slugging + collision
//! handling) and then [`bypass_core::store::Store::insert_no_commit`]
//! + a single bulk `commit_changes`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use bypass_core::crypto::SecretBytes;
use bypass_core::import::{self, ImportedEntry, LossinessReport};
use bypass_core::path::RelPath;

/// Recognised values for the `--format` argument.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum Format {
    Bitwarden,
    Csv,
    Keepass,
}

pub fn run(
    format: Option<Format>,
    from_ext: Option<&str>,
    source: &Path,
    csv_schema: Option<&str>,
    csv_has_header: bool,
) -> Result<u8> {
    let (entries, mut report, label) = match (format, from_ext) {
        (Some(_), Some(_)) => {
            // Clap should already enforce this via `conflicts_with`,
            // but defend in depth.
            bail!("--format and --from-ext are mutually exclusive");
        }
        (None, None) => {
            bail!("specify either --format=<bitwarden|csv|keepass> or --from-ext=<name>");
        }
        (Some(fmt), None) => {
            let bytes = fs::read(source)
                .with_context(|| format!("read import source {}", source.display()))?;
            parse_in_tree(&fmt, &bytes, csv_schema, csv_has_header)?
        }
        (None, Some(ext_name)) => {
            if csv_schema.is_some() {
                bail!("--csv-schema is not applicable to --from-ext");
            }
            let (entries, report) = run_extension(ext_name, source)
                .with_context(|| format!("invoke importer extension `{ext_name}`"))?;
            (entries, report, format!("ext:{ext_name}"))
        }
    };

    if entries.is_empty() {
        emit_report(&report);
        bail!("source contained no importable entries");
    }

    let mut store = crate::open_store()?;
    let existing: Vec<RelPath> = store.list(None).map_err(crate::map_store_err)?;
    let (prepared, mapping_report) =
        import::prepare(entries, &existing).context("canonical-mapping import entries")?;
    report.notes.extend(mapping_report.notes);

    let mut blobs: Vec<RelPath> = Vec::with_capacity(prepared.len());
    for entry in prepared {
        let blob = store
            .insert_no_commit(&entry.path, entry.body.as_slice())
            .map_err(crate::map_store_err)?;
        blobs.push(blob);
    }
    let n = blobs.len();
    if !blobs.is_empty() {
        store
            .commit_changes(&blobs, &format!("bypass: Import {n} entries from {label}"))
            .map_err(crate::map_store_err)?;
    }

    let suffix = if n == 1 { "entry" } else { "entries" };
    eprintln!("imported {n} {suffix} from {label}");
    emit_report(&report);
    Ok(0)
}

fn parse_in_tree(
    format: &Format,
    bytes: &[u8],
    csv_schema: Option<&str>,
    csv_has_header: bool,
) -> Result<(Vec<ImportedEntry>, LossinessReport, String)> {
    Ok(match format {
        Format::Bitwarden => {
            if csv_schema.is_some() {
                bail!("--csv-schema is not applicable to --format=bitwarden");
            }
            let (e, r) =
                bypass_core::import::bitwarden::parse(bytes).context("parse Bitwarden export")?;
            (e, r, "Bitwarden".to_owned())
        }
        Format::Csv => {
            let schema_str =
                csv_schema.ok_or_else(|| anyhow!("--format=csv requires --csv-schema"))?;
            let schema = bypass_core::import::csv::CsvSchema::parse(schema_str)
                .context("parse --csv-schema")?;
            let (e, r) = bypass_core::import::csv::parse(bytes, &schema, csv_has_header)
                .context("parse CSV import")?;
            (e, r, "CSV".to_owned())
        }
        Format::Keepass => {
            if csv_schema.is_some() {
                bail!("--csv-schema is not applicable to --format=keepass");
            }
            let (e, r) =
                bypass_core::import::keepass::parse(bytes).context("parse KeePass XML export")?;
            (e, r, "KeePass".to_owned())
        }
    })
}

// ===== --from-ext path (ADR-0029) =====================================

/// Spawn `bypass-import-<name>` per ADR-0029 and stream its
/// newline-delimited JSON stdout into `ImportedEntry` records.
fn run_extension(name: &str, source: &Path) -> Result<(Vec<ImportedEntry>, LossinessReport)> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == ".." {
        bail!("invalid extension name: {name:?}");
    }

    let exe = locate_extension(name)?;
    let store_root =
        crate::storage_fs::StorageFs::resolve_default_root().context("resolve store root")?;
    let bypass_bin = std::env::current_exe().context("locate self exe for PASSWORD_STORE_BIN")?;

    let mut child = Command::new(&exe)
        .arg(source)
        .env("PASSWORD_STORE_DIR", &store_root)
        .env("PASSWORD_STORE_BIN", &bypass_bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {}", exe.display()))?;

    let stdout = child
        .stdout
        .take()
        .expect("stdout requested via Stdio::piped");
    let (entries, report) = read_ndjson_stream(stdout)
        .with_context(|| format!("decode NDJSON from {}", exe.display()))?;

    let status = child.wait().context("wait on extension")?;
    if !status.success() {
        bail!(
            "extension `{name}` exited with status {} — bundle abandoned",
            status.code().unwrap_or(-1),
        );
    }

    Ok((entries, report))
}

/// Walk the same candidate paths the existing `bypass ext`
/// dispatch uses (store-local, `$PASSWORD_STORE_EXTENSIONS_DIR`,
/// `~/.password-store-extensions`), but with the import-specific
/// prefix `bypass-import-`.
fn locate_extension(name: &str) -> Result<std::path::PathBuf> {
    let store_root =
        crate::storage_fs::StorageFs::resolve_default_root().context("resolve store root")?;
    let file_name = format!("bypass-import-{name}");
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    candidates.push(store_root.join(".extensions").join(&file_name));
    if let Ok(dir) = std::env::var("PASSWORD_STORE_EXTENSIONS_DIR")
        && !dir.is_empty()
    {
        candidates.push(std::path::PathBuf::from(dir).join(&file_name));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".password-store-extensions").join(&file_name));
    }

    candidates
        .iter()
        .find(|p| is_executable_file(p))
        .cloned()
        .ok_or_else(|| {
            let tried = candidates
                .iter()
                .map(|p| format!("\n  - {}", p.display()))
                .collect::<String>();
            anyhow!("importer extension `bypass-import-{name}` not found (tried:{tried}\n)")
        })
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Wire shape that `serde_json` decodes; converts into
/// [`ImportedEntry`] (whose `password` field is `SecretBytes`,
/// hence not directly `Deserialize`-able).
#[derive(Debug, serde::Deserialize)]
struct WireEntry {
    #[serde(default)]
    folder: Vec<String>,
    name: String,
    password: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    fields: Vec<(String, String)>,
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    uris: Vec<String>,
}

impl From<WireEntry> for ImportedEntry {
    fn from(w: WireEntry) -> Self {
        ImportedEntry {
            folder: w.folder,
            name: w.name,
            password: SecretBytes::new(w.password.into_bytes()),
            username: w.username.filter(|s| !s.is_empty()),
            fields: w.fields,
            totp: w.totp.filter(|s| !s.is_empty()),
            notes: w.notes.filter(|s| !s.is_empty()),
            uris: w.uris,
        }
    }
}

fn read_ndjson_stream<R: Read>(source: R) -> Result<(Vec<ImportedEntry>, LossinessReport)> {
    let mut reader = BufReader::new(source);
    let mut entries: Vec<ImportedEntry> = Vec::new();
    let report = LossinessReport::default();
    let mut line = String::new();
    let mut lineno: usize = 0;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).context("read NDJSON line")?;
        if n == 0 {
            break;
        }
        lineno += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Blank lines are tolerated as separators.
            continue;
        }
        let wire: WireEntry =
            serde_json::from_str(trimmed).with_context(|| format!("parse NDJSON line {lineno}"))?;
        entries.push(wire.into());
    }
    Ok((entries, report))
}

// ===== shared lossiness summary =======================================

/// Mandatory stderr summary (ADR-0027 4.5.g). Prints nothing when
/// the report is empty.
fn emit_report(report: &LossinessReport) {
    if report.is_empty() {
        return;
    }
    eprintln!(
        "lossiness: {} transformation{} or drop{}:",
        report.len(),
        if report.len() == 1 { "" } else { "s" },
        if report.len() == 1 { "" } else { "s" },
    );
    for note in &report.notes {
        if note.entry.is_empty() {
            eprintln!("  - {}", note.message);
        } else {
            eprintln!("  - {}: {}", note.entry, note.message);
        }
    }
}
