// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass import --format=<name> <file>` driver — foreign-vault
//! importers (ADR-0027 / Milestone 4.5).
//!
//! Format-specific parsing lives in `bypass_core::import::<format>`.
//! This module owns the I/O and write path: read the source file,
//! call the parser, run the result through
//! [`bypass_core::import::prepare`] for slugging + collision
//! handling, then encrypt-and-write each entry through
//! [`bypass_core::store::Store::insert_no_commit`] (same code path
//! `bypass restore` uses, per ADR-0027 "one write path"). Finally,
//! emit the lossiness summary to stderr.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use bypass_core::import::{self, LossinessReport};
use bypass_core::path::RelPath;

/// Recognised values for the `--format` argument. Clap enforces the
/// match via `value_enum`.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum Format {
    Bitwarden,
    Csv,
}

pub fn run(
    format: Format,
    source: &Path,
    csv_schema: Option<&str>,
    csv_has_header: bool,
) -> Result<u8> {
    let bytes =
        fs::read(source).with_context(|| format!("read import source {}", source.display()))?;

    // ----- parse format → ImportedEntry list ---------------------------
    let (entries, mut report) = match format {
        Format::Bitwarden => {
            if csv_schema.is_some() {
                bail!("--csv-schema is not applicable to --format=bitwarden");
            }
            bypass_core::import::bitwarden::parse(&bytes).context("parse Bitwarden export")?
        }
        Format::Csv => {
            let schema_str =
                csv_schema.ok_or_else(|| anyhow!("--format=csv requires --csv-schema"))?;
            let schema = bypass_core::import::csv::CsvSchema::parse(schema_str)
                .context("parse --csv-schema")?;
            bypass_core::import::csv::parse(&bytes[..], &schema, csv_has_header)
                .context("parse CSV import")?
        }
    };

    if entries.is_empty() {
        emit_report(&report);
        bail!("source contained no importable entries");
    }

    // ----- canonical mapping ------------------------------------------
    let mut store = crate::open_store()?;
    let existing: Vec<RelPath> = store.list(None).map_err(crate::map_store_err)?;
    let (prepared, mapping_report) =
        import::prepare(entries, &existing).context("canonical-mapping import entries")?;
    report.notes.extend(mapping_report.notes);

    // ----- encrypt + write each entry; one bulk commit -----------------
    let mut blobs: Vec<RelPath> = Vec::with_capacity(prepared.len());
    for entry in prepared {
        let blob = store
            .insert_no_commit(&entry.path, entry.body.as_slice())
            .map_err(crate::map_store_err)?;
        blobs.push(blob);
    }
    let label = match format {
        Format::Bitwarden => "Bitwarden",
        Format::Csv => "CSV",
    };
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
