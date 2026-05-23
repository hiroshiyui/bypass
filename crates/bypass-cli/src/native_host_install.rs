// SPDX-License-Identifier: GPL-3.0-or-later

//! Install / uninstall the per-browser native-messaging manifest
//! files that point at this `bypass` binary, per
//! [ADR-0022](../../../doc/adr/0022-native-messaging-wire-protocol.md)
//! +
//! [ADR-0023](../../../doc/adr/0023-browser-extension-architecture.md).
//!
//! Same `current_exe()` baking pattern as
//! [`sync::service`](sync/service.rs): we resolve the running
//! binary's absolute path at install time, write it into the
//! manifest's `path` field, and the browser launches that exact
//! binary when the matching extension calls
//! `chrome.runtime.connectNative`.

#![cfg(unix)]

use std::path::PathBuf;

use anyhow::{Context, Result};

const HOST_NAME: &str = "io.bypass.host";
const HOST_DESC: &str = "bypass password manager native host";
/// Stable extension identifier for the Firefox build. The extension
/// itself pins this via `browser_specific_settings.gecko.id` in its
/// own manifest. If we ever ship a second extension (e.g. a
/// developer build alongside the published one), update this and
/// the extension's manifest in lockstep.
const FIREFOX_EXTENSION_ID: &str = "bypass@bypass.example";

/// Public surface called from `Command::MessagingHost { sub: Install }`
/// in `main.rs`.
pub fn install(chrome_id: Option<String>, firefox_id: Option<String>) -> Result<u8> {
    let exe = std::env::current_exe()
        .context("resolve current `bypass` binary path for native-host install")?;
    let host_cmd = exe.to_string_lossy().into_owned();

    let firefox_ext = firefox_id.unwrap_or_else(|| FIREFOX_EXTENSION_ID.to_owned());
    let firefox_body = render_firefox_manifest(&host_cmd, &firefox_ext);
    write_to_paths(&firefox_paths()?, &firefox_body, "Firefox")?;

    match chrome_id {
        Some(id) => {
            let chrome_body = render_chrome_manifest(&host_cmd, &id);
            write_to_paths(&chrome_paths()?, &chrome_body, "Chrome / Chromium")?;
        }
        None => {
            eprintln!(
                "bypass: --chrome-id <id> not provided; skipped Chrome / Chromium manifests.\n\
                 Load the extension at chrome://extensions, copy the assigned ID, and re-run\n\
                 `bypass messaging-host install --chrome-id <id>` to wire Chrome up."
            );
        }
    }

    eprintln!(
        "bypass: native-messaging host registered at `{HOST_NAME}` pointing to {}.\n\
         Re-run `bypass messaging-host install` after upgrading `bypass` so the\n\
         path above stays in sync with the actual binary.",
        host_cmd
    );
    Ok(0)
}

pub fn uninstall() -> Result<u8> {
    let mut removed = 0;
    for p in firefox_paths()?.iter().chain(chrome_paths()?.iter()) {
        match std::fs::remove_file(p) {
            Ok(()) => {
                eprintln!("bypass: removed {}", p.display());
                removed += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(anyhow::Error::from(source).context(format!("remove {}", p.display())));
            }
        }
    }
    if removed == 0 {
        eprintln!("bypass: no native-messaging manifests to remove");
    }
    Ok(0)
}

// ----- per-browser path lists ----------------------------------------

fn firefox_paths() -> Result<Vec<PathBuf>> {
    let home = home_dir()?;
    let file = format!("{HOST_NAME}.json");
    Ok(vec![
        home.join(".mozilla/native-messaging-hosts").join(&file),
    ])
}

fn chrome_paths() -> Result<Vec<PathBuf>> {
    let home = home_dir()?;
    let file = format!("{HOST_NAME}.json");
    Ok(vec![
        home.join(".config/google-chrome/NativeMessagingHosts")
            .join(&file),
        home.join(".config/chromium/NativeMessagingHosts")
            .join(&file),
    ])
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("cannot resolve $HOME; set the variable manually")
}

// ----- manifest renderers --------------------------------------------

/// Firefox uses `allowed_extensions`, an array of full extension
/// IDs (e.g. `bypass@bypass.example`).
fn render_firefox_manifest(host_cmd: &str, extension_id: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "description": "{desc}",
  "path": "{path}",
  "type": "stdio",
  "allowed_extensions": ["{ext}"]
}}
"#,
        name = HOST_NAME,
        desc = HOST_DESC,
        path = json_escape(host_cmd),
        ext = json_escape(extension_id),
    )
}

/// Chrome / Chromium use `allowed_origins`, an array of
/// `chrome-extension://<id>/` origins.
fn render_chrome_manifest(host_cmd: &str, extension_id: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "description": "{desc}",
  "path": "{path}",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://{ext}/"]
}}
"#,
        name = HOST_NAME,
        desc = HOST_DESC,
        path = json_escape(host_cmd),
        ext = json_escape(extension_id),
    )
}

/// The `path` field in the manifest needs to be a JSON string —
/// escape backslashes (Windows; not actually a target for v1 but
/// cheap to be correct) and double-quotes. We don't generate the
/// surrounding quotes here; the format strings above do.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            _ => out.push(c),
        }
    }
    out
}

fn write_to_paths(paths: &[PathBuf], body: &str, label: &str) -> Result<()> {
    let mut wrote_any = false;
    for path in paths {
        // Only write into parent dirs that already exist OR that we
        // can create. Chrome / Firefox lay these directories down
        // themselves on first launch; we create-with-parents so the
        // user can `install` before ever opening the browser too.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
        eprintln!("bypass: wrote {} manifest at {}", label, path.display());
        wrote_any = true;
    }
    if !wrote_any {
        // Should be impossible given the path lists above are
        // non-empty, but defence-in-depth.
        anyhow::bail!("no install paths configured for {label} on this platform");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_manifest_includes_allowed_extensions_and_stdio_type() {
        let body = render_firefox_manifest("/opt/bin/bypass", "bypass@bypass.example");
        assert!(body.contains("\"name\": \"io.bypass.host\""));
        assert!(body.contains("\"path\": \"/opt/bin/bypass\""));
        assert!(body.contains("\"type\": \"stdio\""));
        assert!(body.contains("\"allowed_extensions\": [\"bypass@bypass.example\"]"));
        // Sanity: valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["name"], "io.bypass.host");
    }

    #[test]
    fn chrome_manifest_includes_allowed_origins_with_extension_prefix() {
        let body = render_chrome_manifest("/usr/local/bin/bypass", "abcdefghijklmnop");
        assert!(body.contains("\"allowed_origins\": [\"chrome-extension://abcdefghijklmnop/\"]"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["path"], "/usr/local/bin/bypass");
    }

    #[test]
    fn json_escape_handles_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"a\b"c"#), r#"a\\b\"c"#);
    }

    #[test]
    fn firefox_paths_include_dot_mozilla_subdir() {
        let paths = firefox_paths().unwrap();
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(".mozilla/native-messaging-hosts/io.bypass.host.json")),
            "got {paths:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn chrome_paths_on_linux_cover_google_chrome_and_chromium() {
        let paths = chrome_paths().unwrap();
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().contains("google-chrome"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().contains("chromium"))
        );
    }
}
