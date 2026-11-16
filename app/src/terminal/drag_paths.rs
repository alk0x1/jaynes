//! JAYNES: copied from herdr-app's src/terminal/drag_paths.rs (Apache-2.0) and changed: no drags from mods, and a refused
//! drop is logged instead of shown as a toast.
//! Routing and pasting paths without treating another machine's files as local.

use std::path::PathBuf;

fn validate_paths(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Result<(), &'static str> {
    let mut any = false;
    for path in paths {
        any = true;
        let path = path.as_ref();
        // Even inside shell quotes, a newline becomes Enter in an unbracketed paste.
        if path.is_empty() || path.chars().any(char::is_control) {
            return Err("Paths containing control characters cannot be pasted safely");
        }
    }
    if any { Ok(()) } else { Err("No files were dropped") }
}

pub(super) fn validate_upload(paths: &[PathBuf]) -> Result<(), &'static str> {
    validate_paths(paths.iter().map(|p| p.to_string_lossy()))
}

/// Why this one item can't be copied to another machine; the rest of a drop still goes.
pub(super) fn upload_refusal(path: &std::path::Path) -> Option<&'static str> {
    path.is_dir().then_some("Copying local folders to another machine is not supported yet. Drop the folder into a terminal on this computer instead.")
}

/// Paths as the destination shell reads them, with a trailing space, never Enter.
pub(super) fn quote_paths(paths: impl IntoIterator<Item = String>, target: &str, local_windows: bool) -> Result<String, &'static str> {
    let paths: Vec<_> = paths.into_iter().collect();
    validate_paths(&paths)?;
    // Remote Files and file transfers currently target Unix machines, even from Windows.
    let windows = local_windows && !target.contains('/');
    let quote = |p: String| {
        if p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%~".contains(c) || (!windows && c == ',')) {
            return p;
        }
        if windows { format!("'{}'", p.replace('\'', "''")) } else { format!("'{}'", p.replace('\'', "'\\''")) }
    };
    let mut out = paths.into_iter().map(quote).collect::<Vec<_>>().join(" ");
    out.push(' ');
    Ok(out)
}

pub(super) fn unavailable(body: &str) {
    tracing::warn!("file drop unavailable: {body}");
}
