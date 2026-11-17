//! Windows bundles: a folder with the player renamed to `<Name>.exe` and
//! `game.pak` next to it (spike S7).
//!
//! The executable's resources are patched with `editpe`, which works on any
//! host: the main icon (from the project's PNG, resized to the icon sizes),
//! the version information (`ProductName`, `FileDescription`,
//! `ProductVersion`, `FileVersion`, `OriginalFilename`, `InternalName`) and
//! the PE subsystem: GUI for release builds (no console window), console for
//! development builds (logs visible). Patching drops an Authenticode
//! signature; templates ship unsigned.

use std::path::Path;

use editpe::types::{VersionU16, VersionU32};
use editpe::{Image, VersionInfo, VersionStringTable};
use image::DynamicImage;

use crate::{AppIdentity, ExportError};
use box_engine::packed::Profile;

/// `IMAGE_SUBSYSTEM_WINDOWS_GUI`: no console window.
pub const SUBSYSTEM_GUI: u16 = 2;
/// `IMAGE_SUBSYSTEM_WINDOWS_CUI`: a console window.
pub const SUBSYSTEM_CONSOLE: u16 = 3;

/// True for a PE file (`MZ` header pointing at a `PE\0\0` signature).
pub fn is_pe(bytes: &[u8]) -> bool {
    if bytes.get(0..2) != Some(b"MZ") {
        return false;
    }
    let Some(at) = bytes.get(0x3c..0x40) else {
        return false;
    };
    let at = u32::from_le_bytes([at[0], at[1], at[2], at[3]]) as usize;
    bytes.get(at..at + 4) == Some(b"PE\0\0")
}

/// The four version numbers of `version` (`1.2.3` → 1, 2, 3, 0).
fn numbers(version: &str) -> [u16; 4] {
    let mut out = [0u16; 4];
    for (slot, part) in out.iter_mut().zip(version.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

/// Patches the PE `bytes` (the player) for `id` and `profile`; returns the
/// new executable.
pub fn patch_executable(
    bytes: &[u8],
    id: &AppIdentity,
    icon: &DynamicImage,
    exe_name: &str,
    profile: Profile,
) -> Result<Vec<u8>, ExportError> {
    if !is_pe(bytes) {
        return Err(ExportError::Template(
            "the Windows player template is not a Windows executable".into(),
        ));
    }
    let pe = |e: &dyn std::fmt::Display| ExportError::Bundle(format!("Windows executable: {e}"));
    let mut image = Image::parse(bytes.to_vec()).map_err(|e| pe(&e))?;
    let mut resources = image.resource_directory().cloned().unwrap_or_default();
    resources.remove_main_icon().map_err(|e| pe(&e))?;
    resources.set_main_icon(icon.clone()).map_err(|e| pe(&e))?;
    let [a, b, c, d] = numbers(&id.version);
    let mut info = VersionInfo::default();
    info.info.file_version = VersionU32 {
        major: (u32::from(a) << 16) | u32::from(b),
        minor: (u32::from(c) << 16) | u32::from(d),
    };
    info.info.product_version = info.info.file_version;
    let mut table = VersionStringTable {
        // US English, Unicode.
        key: "040904b0".into(),
        strings: Default::default(),
    };
    for (key, value) in [
        ("ProductName", id.name.as_str()),
        ("FileDescription", id.name.as_str()),
        ("ProductVersion", id.version.as_str()),
        ("FileVersion", id.version.as_str()),
        ("OriginalFilename", exe_name),
        ("InternalName", id.bundle_id.as_str()),
    ] {
        table.strings.insert(key.into(), value.into());
    }
    info.strings.push(table);
    info.vars.push(VersionU16 {
        major: 0x0409,
        minor: 0x04b0,
    });
    resources.set_version_info(&info).map_err(|e| pe(&e))?;
    image
        .set_resource_directory(resources)
        .map_err(|e| pe(&e))?;
    image.set_subsystem(match profile {
        Profile::Release => SUBSYSTEM_GUI,
        Profile::Development => SUBSYSTEM_CONSOLE,
    });
    let mut out = Vec::new();
    image.write_writer(&mut out).map_err(|e| pe(&e))?;
    Ok(out)
}

/// What [`read_back`] finds in a patched executable (for tests and the
/// export report).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeSummary {
    pub subsystem: u16,
    pub icon_bytes: Option<usize>,
    pub product_name: Option<String>,
    pub file_version: Option<String>,
}

/// Reads the subsystem, icon and version strings of the PE at `path`.
pub fn read_back(path: &Path) -> Result<PeSummary, ExportError> {
    let pe = |e: &dyn std::fmt::Display| ExportError::Bundle(format!("{}: {e}", path.display()));
    let image = Image::parse_file(path).map_err(|e| pe(&e))?;
    let resources = image.resource_directory();
    let icon_bytes = match resources {
        Some(r) => r.get_main_icon().map_err(|e| pe(&e))?.map(<[u8]>::len),
        None => None,
    };
    let version = match resources {
        Some(r) => r.get_version_info().map_err(|e| pe(&e))?,
        None => None,
    };
    let string = |key: &str| {
        version
            .as_ref()
            .and_then(|v| v.strings.first())
            .and_then(|t| t.strings.get(key).cloned())
    };
    Ok(PeSummary {
        subsystem: image.subsystem(),
        icon_bytes,
        product_name: string("ProductName"),
        file_version: string("FileVersion"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numbers_and_pe_detection() {
        assert_eq!(numbers("1.2.3"), [1, 2, 3, 0]);
        assert_eq!(numbers("7"), [7, 0, 0, 0]);
        assert!(!is_pe(b"MZ"));
        assert!(!is_pe(&[0xcf, 0xfa, 0xed, 0xfe]));
    }
}
