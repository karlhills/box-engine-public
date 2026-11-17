//! macOS bundles: `<Name>.app` with a generated `Info.plist`, an `.icns`
//! icon, the player and `game.pak`, ad-hoc signed (spike S7).
//!
//! ```text
//! <Name>.app/Contents/Info.plist
//! <Name>.app/Contents/MacOS/<Name>          the player template (or a native game's player)
//! <Name>.app/Contents/Resources/AppIcon.icns
//! <Name>.app/Contents/Resources/game.pak
//! ```
//!
//! Signing is `codesign --force --sign - --timestamp=none` (shipped with
//! macOS; no Xcode), run last because the signature seals `Info.plist` and
//! `Resources`. Ad-hoc signed apps run on the Mac that made them and on
//! others after right-click ▸ Open; distributing without that step needs a
//! Developer ID signature and notarization with the developer's own
//! credentials.

use std::path::Path;
use std::process::Command;

use image::DynamicImage;

use crate::{AppIdentity, ExportError};

/// The `Info.plist` of an exported game.
pub fn info_plist(id: &AppIdentity, executable: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundleExecutable</key>
	<string>{exe}</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleIdentifier</key>
	<string>{bundle_id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.games</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
</dict>
</plist>
"#,
        name = esc(&id.name),
        exe = esc(executable),
        bundle_id = esc(&id.bundle_id),
        version = esc(&id.version),
    )
}

/// An `.icns` file of PNG images resized from `icon` (deterministic, no
/// `iconutil` needed).
pub fn icns(icon: &DynamicImage) -> Result<Vec<u8>, ExportError> {
    // (type, pixel size): the 1x and 2x slots macOS reads.
    const SLOTS: [(&[u8; 4], u32); 11] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ];
    let mut body = Vec::new();
    for (kind, size) in SLOTS {
        let resized = icon.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
        let mut png = Vec::new();
        resized
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| ExportError::Settings(format!("icon: {e}")))?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// True for a Mach-O executable (thin 64-bit or universal).
pub fn is_mach_o(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(0..4),
        Some([0xcf, 0xfa, 0xed, 0xfe]) | Some([0xca, 0xfe, 0xba, 0xbe])
    )
}

/// Writes the bundle skeleton into `app` (which must not exist): the
/// player copied as `executable`, `Info.plist`, the icon. The pak is moved
/// in by the caller.
pub fn write_bundle(
    app: &Path,
    id: &AppIdentity,
    player: &Path,
    executable: &str,
    icon: &DynamicImage,
) -> Result<(), ExportError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ExportError::Io { path, source }
    };
    let bytes = std::fs::read(player).map_err(io(player))?;
    if !is_mach_o(&bytes) {
        return Err(ExportError::Template(format!(
            "{} is not a macOS executable",
            player.display()
        )));
    }
    let macos = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    std::fs::create_dir_all(&macos).map_err(io(&macos))?;
    std::fs::create_dir_all(&resources).map_err(io(&resources))?;
    let exe = macos.join(executable);
    std::fs::copy(player, &exe).map_err(io(&exe))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(io(&exe))?;
    }
    let plist = app.join("Contents/Info.plist");
    std::fs::write(&plist, info_plist(id, executable)).map_err(io(&plist))?;
    let icns_path = resources.join("AppIcon.icns");
    std::fs::write(&icns_path, icns(icon)?).map_err(io(&icns_path))?;
    Ok(())
}

/// How signing went.
pub enum Signed {
    Yes,
    /// `codesign` is not available (not a Mac): the reason.
    No(String),
}

/// Ad-hoc signs `app` with `codesign` and verifies the signature.
pub fn sign(app: &Path) -> Result<Signed, ExportError> {
    let run = |args: &[&str]| Command::new("codesign").args(args).arg(app).output();
    let out = match run(&["--force", "--sign", "-", "--timestamp=none"]) {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Signed::No(
                "codesign not found (not a Mac): the bundle is unsigned; Apple Silicon Macs run only signed apps, so sign it on a Mac with `codesign --force --sign - <app>`".into(),
            ));
        }
        Err(e) => return Err(ExportError::Sign(format!("could not run codesign: {e}"))),
    };
    if !out.status.success() {
        return Err(ExportError::Sign(format!(
            "codesign failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let verify = run(&["--verify", "--strict"]).map_err(|e| ExportError::Sign(e.to_string()))?;
    if !verify.status.success() {
        return Err(ExportError::Sign(format!(
            "the signature does not verify: {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        )));
    }
    Ok(Signed::Yes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_and_icns_has_every_slot() {
        let id = AppIdentity {
            name: "Fish & <Chips>".into(),
            bundle_id: "com.example.fish".into(),
            version: "1.2.3".into(),
            icon: None,
        };
        let plist = info_plist(&id, "Fish & Chips");
        assert!(plist.contains("<string>Fish &amp; &lt;Chips&gt;</string>"));
        assert!(
            plist.contains("<key>CFBundleExecutable</key>\n\t<string>Fish &amp; Chips</string>")
        );
        assert!(plist.contains("<string>1.2.3</string>"));
        let icon = DynamicImage::new_rgba8(64, 64);
        let bytes = icns(&icon).unwrap();
        assert_eq!(&bytes[0..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len()
        );
        // Walk the entries.
        let mut at = 8;
        let mut kinds = Vec::new();
        while at < bytes.len() {
            kinds.push(String::from_utf8(bytes[at..at + 4].to_vec()).unwrap());
            let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            assert_eq!(&bytes[at + 9..at + 12], b"PNG");
            at += len;
        }
        assert_eq!(at, bytes.len());
        assert_eq!(kinds.len(), 11);
        assert_eq!(icns(&icon).unwrap(), bytes, "deterministic");
        assert!(is_mach_o(&[0xcf, 0xfa, 0xed, 0xfe, 7]));
        assert!(!is_mach_o(b"MZ\x90\x00"));
    }
}
