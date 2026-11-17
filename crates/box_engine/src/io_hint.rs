//! What to do about common file errors: a short fix appended to an
//! `io::Error` message (no permission, read-only or full disk), so a failed
//! save says more than "os error 28".

use std::io::{Error, ErrorKind};

/// The fix for `e`'s kind, if it has a common one.
pub fn hint(e: &Error) -> Option<&'static str> {
    Some(match e.kind() {
        ErrorKind::PermissionDenied => {
            "no permission to write here: check the file's and its folder's permissions (or whether another program locks it), or choose another location"
        }
        ErrorKind::ReadOnlyFilesystem => "the disk is read-only: choose another location",
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => {
            "the disk is full: free some space and try again"
        }
        ErrorKind::FileTooLarge => "the file is too large for this disk's file system",
        _ => return None,
    })
}

/// `e` as text, followed by its [`hint`] when it has one.
pub fn text(e: &Error) -> String {
    match hint(e) {
        Some(h) => format!("{e}; {h}"),
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_kinds_get_a_fix() {
        let full = text(&Error::from(ErrorKind::StorageFull));
        assert!(full.contains("the disk is full: free some space"), "{full}");
        let denied = text(&Error::from(ErrorKind::PermissionDenied));
        assert!(
            denied.contains("check the file's and its folder's permissions"),
            "{denied}"
        );
        assert_eq!(text(&Error::other("odd")), "odd");
    }
}
