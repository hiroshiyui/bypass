// SPDX-License-Identifier: GPL-3.0-or-later

//! KeePass KDBX-XML export parser.
//!
//! KeePass 2.x (and KeePassXC) can export the database as XML via
//! `File → Export → KeePass XML (2.x)`. The resulting file is
//! *unencrypted* — this importer expects that shape, not the
//! encrypted `.kdbx` binary (which requires the user's master
//! password to decrypt and is out of scope for v1; pair the
//! database first or export it).
//!
//! Schema (abbreviated):
//!
//! ```xml
//! <KeePassFile>
//!   <Root>
//!     <Group>
//!       <Name>Root</Name>
//!       <Group>
//!         <Name>Email</Name>
//!         <Entry>
//!           <String><Key>Title</Key><Value>Gmail</Value></String>
//!           <String><Key>UserName</Key><Value>alice</Value></String>
//!           <String><Key>Password</Key><Value Protected="False">p4ss</Value></String>
//!           <String><Key>URL</Key><Value>https://gmail.com</Value></String>
//!           <String><Key>Notes</Key><Value>multi line</Value></String>
//!           <String><Key>otp</Key><Value>otpauth://...</Value></String>
//!           <String><Key>recovery</Key><Value>kitten</Value></String>
//!         </Entry>
//!       </Group>
//!     </Group>
//!   </Root>
//! </KeePassFile>
//! ```
//!
//! Mapping:
//!
//! - The implicit root `<Group>` *name* (commonly "Root", "NewDatabase",
//!   or the database name) is dropped — users don't expect it as a
//!   folder segment. All other group names become folder components,
//!   in order.
//! - Standard string keys map to bypass entry roles:
//!   - `Title` → entry name.
//!   - `UserName` → `login:`.
//!   - `Password` → first line.
//!   - `URL` → `url:`.
//!   - `Notes` → free-form notes body.
//!   - `otp` (KeePassXC convention) → `otpauth:`.
//! - Any other `<String>` becomes a custom field with that key.
//!
//! Out of scope:
//!
//! - Encrypted KDBX binary (`.kdbx`) — needs master-password decrypt;
//!   surface as a clean error if the caller hands us non-XML bytes.
//! - Attachments (`<Binary>` under `<Entry>`) — surfaced as a
//!   lossiness count.
//! - Entry history (`<History>` inside `<Entry>`) — silently
//!   ignored; we only carry the current version of each entry.

use thiserror::Error;

use super::{ImportedEntry, LossinessReport};
use crate::crypto::SecretBytes;

#[derive(Debug, Error)]
pub enum KeepassError {
    #[error("KeePass KDBX-XML parse: {0}")]
    Parse(#[from] roxmltree::Error),
    #[error("not a KeePass XML export: missing <KeePassFile> root")]
    NotKeepass,
    #[error(
        "this looks like the binary KDBX format — `bypass` imports only the *XML* export (File → Export → KeePass XML (2.x)). Re-export from KeePass / KeePassXC."
    )]
    LooksLikeBinaryKdbx,
}

/// Parse a KeePass XML export. Returns the importable entries plus
/// any lossiness notes the parser itself produced (attachments
/// dropped, etc.).
pub fn parse(bytes: &[u8]) -> Result<(Vec<ImportedEntry>, LossinessReport), KeepassError> {
    // Quick sniff: KDBX binary starts with the signature 0x9aa2d903.
    if bytes.len() >= 4 && bytes[0..4] == [0x03, 0xd9, 0xa2, 0x9a] {
        return Err(KeepassError::LooksLikeBinaryKdbx);
    }

    let text = std::str::from_utf8(bytes).map_err(|_| KeepassError::NotKeepass)?;
    let doc = roxmltree::Document::parse(text)?;

    let root = doc.root_element();
    if !root.has_tag_name("KeePassFile") {
        return Err(KeepassError::NotKeepass);
    }
    let root_section = root
        .children()
        .find(|n| n.is_element() && n.has_tag_name("Root"))
        .ok_or(KeepassError::NotKeepass)?;

    let mut entries = Vec::new();
    let mut report = LossinessReport::default();
    let mut binary_total: usize = 0;

    // KeePass nests a single top-level <Group> directly under <Root>,
    // whose name is conventionally the database name. We drop that
    // outermost name so users see their familiar group hierarchy
    // without an extra leading segment.
    let mut top_groups = root_section
        .children()
        .filter(|n| n.is_element() && n.has_tag_name("Group"));
    if let Some(first_group) = top_groups.next() {
        walk_group(
            first_group,
            &[], // ← outermost group name is dropped
            &mut entries,
            &mut report,
            &mut binary_total,
            true,
        );
        // Anything *else* at the same level is unusual; treat as a
        // peer group with its own name preserved.
        for sibling in top_groups {
            walk_group(
                sibling,
                &[],
                &mut entries,
                &mut report,
                &mut binary_total,
                true,
            );
        }
    }

    if binary_total > 0 {
        report.push(
            "",
            format!(
                "{binary_total} entry attachment{} dropped (KeePass `<Binary>` references are not carried into the bypass entry body)",
                if binary_total == 1 { "" } else { "s" },
            ),
        );
    }

    Ok((entries, report))
}

fn walk_group(
    group: roxmltree::Node<'_, '_>,
    folder_so_far: &[String],
    out: &mut Vec<ImportedEntry>,
    report: &mut LossinessReport,
    binary_total: &mut usize,
    drop_own_name: bool,
) {
    let name = text_of_child(group, "Name").unwrap_or_default();
    let mut folder: Vec<String> = folder_so_far.to_vec();
    if !drop_own_name && !name.is_empty() {
        folder.push(name);
    }

    for child in group.children().filter(|n| n.is_element()) {
        if child.has_tag_name("Entry") {
            let imported = build_entry(child, &folder, report, binary_total);
            if let Some(e) = imported {
                out.push(e);
            }
        } else if child.has_tag_name("Group") {
            walk_group(child, &folder, out, report, binary_total, false);
        }
        // Other elements (Times, IconID, ...) are silently ignored.
    }
}

fn build_entry(
    entry: roxmltree::Node<'_, '_>,
    folder: &[String],
    report: &mut LossinessReport,
    binary_total: &mut usize,
) -> Option<ImportedEntry> {
    let mut title = String::new();
    let mut username: Option<String> = None;
    let mut password = String::new();
    let mut url: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut totp: Option<String> = None;
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut binaries_here = 0usize;

    for child in entry.children().filter(|n| n.is_element()) {
        if child.has_tag_name("String") {
            let key = text_of_child(child, "Key").unwrap_or_default();
            let value = text_of_child(child, "Value").unwrap_or_default();
            match key.as_str() {
                "Title" => title = value,
                "UserName" if !value.is_empty() => {
                    username = Some(value);
                }
                "Password" => password = value,
                "URL" if !value.is_empty() => {
                    url = Some(value);
                }
                "Notes" if !value.is_empty() => {
                    notes = Some(value);
                }
                "otp" | "TOTP" | "totp" if !value.is_empty() => {
                    totp = Some(value);
                }
                _ if !key.is_empty() && !value.is_empty() => {
                    fields.push((key, value));
                }
                _ => {}
            }
        } else if child.has_tag_name("Binary") {
            binaries_here += 1;
        }
        // History, Times, IconID, AutoType, … silently dropped.
    }

    if title.is_empty() {
        // KeePass allows empty titles, but a bypass entry needs an
        // addressable name. Skip these with a lossiness note.
        report.push(
            "<untitled>",
            format!(
                "skipped a KeePass entry with no Title (folder: {})",
                if folder.is_empty() {
                    "<root>".to_owned()
                } else {
                    folder.join("/")
                },
            ),
        );
        return None;
    }

    *binary_total += binaries_here;

    let uris = url.into_iter().collect();
    Some(ImportedEntry {
        folder: folder.to_vec(),
        name: title,
        password: SecretBytes::new(password.into_bytes()),
        username,
        fields,
        totp,
        notes,
        uris,
    })
}

/// Get the text content of the first child element with the given
/// tag name. Returns `""` if missing or empty.
fn text_of_child(parent: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    parent
        .children()
        .find(|n| n.is_element() && n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(|s| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<KeePassFile>
  <Meta><DatabaseName>Test</DatabaseName></Meta>
  <Root>
    <Group>
      <Name>NewDatabase</Name>
      <Entry>
        <String><Key>Title</Key><Value>Root Entry</Value></String>
        <String><Key>UserName</Key><Value>alice</Value></String>
        <String><Key>Password</Key><Value Protected="False">p1</Value></String>
        <String><Key>URL</Key><Value>https://example</Value></String>
      </Entry>
      <Group>
        <Name>Email</Name>
        <Entry>
          <String><Key>Title</Key><Value>Gmail</Value></String>
          <String><Key>UserName</Key><Value>bob</Value></String>
          <String><Key>Password</Key><Value>p2</Value></String>
          <String><Key>otp</Key><Value>otpauth://totp/x?secret=ABC</Value></String>
          <String><Key>recovery</Key><Value>kitten</Value></String>
          <String><Key>Notes</Key><Value>multi
line</Value></String>
          <Binary><Key>screenshot.png</Key></Binary>
        </Entry>
        <Group>
          <Name>Work</Name>
          <Entry>
            <String><Key>Title</Key><Value>Office365</Value></String>
            <String><Key>Password</Key><Value>p3</Value></String>
          </Entry>
        </Group>
      </Group>
    </Group>
  </Root>
</KeePassFile>
"#;

    fn body(entry: &ImportedEntry) -> String {
        String::from_utf8(entry.password.as_slice().to_vec()).unwrap()
    }

    #[test]
    fn parses_nested_groups_and_drops_outer_name() {
        let (entries, report) = parse(SAMPLE.as_bytes()).unwrap();
        let by_path: std::collections::HashMap<String, &ImportedEntry> = entries
            .iter()
            .map(|e| {
                let folder = e.folder.join("/");
                let path = if folder.is_empty() {
                    e.name.clone()
                } else {
                    format!("{folder}/{}", e.name)
                };
                (path, e)
            })
            .collect();

        // Top-level entry has no folder (NewDatabase name dropped).
        assert!(by_path.contains_key("Root Entry"));
        // Nested entry under "Email" group.
        let gmail = by_path.get("Email/Gmail").expect("nested entry");
        assert_eq!(gmail.username.as_deref(), Some("bob"));
        assert_eq!(body(gmail), "p2");
        assert_eq!(gmail.totp.as_deref(), Some("otpauth://totp/x?secret=ABC"));
        assert_eq!(gmail.notes.as_deref(), Some("multi\nline"));
        assert_eq!(gmail.fields, vec![("recovery".into(), "kitten".into())]);
        // Twice-nested entry.
        assert!(by_path.contains_key("Email/Work/Office365"));

        // Attachments are flagged in the lossiness report.
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].message.contains("attachment"));
    }

    #[test]
    fn refuses_non_keepass_xml() {
        let xml = r#"<?xml version="1.0"?><other/>"#;
        let err = parse(xml.as_bytes()).unwrap_err();
        assert!(matches!(err, KeepassError::NotKeepass));
    }

    #[test]
    fn refuses_binary_kdbx_with_a_hint() {
        let bytes = [0x03, 0xd9, 0xa2, 0x9a, 0x00, 0x00];
        let err = parse(&bytes).unwrap_err();
        assert!(matches!(err, KeepassError::LooksLikeBinaryKdbx));
    }

    #[test]
    fn skips_entries_with_no_title() {
        let xml = r#"<?xml version="1.0"?>
<KeePassFile>
  <Root>
    <Group>
      <Name>Top</Name>
      <Entry>
        <String><Key>Password</Key><Value>p</Value></String>
      </Entry>
      <Entry>
        <String><Key>Title</Key><Value>Real</Value></String>
        <String><Key>Password</Key><Value>p2</Value></String>
      </Entry>
    </Group>
  </Root>
</KeePassFile>"#;
        let (entries, report) = parse(xml.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Real");
        assert!(report.notes.iter().any(|n| n.message.contains("no Title")));
    }
}
