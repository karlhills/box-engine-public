//! Spike S7: export pipeline risks (docs/RISKS.md R16). Throwaway code.
//!
//! - `pak <project>... [--synthetic <dir>]`: write each project as a custom
//!   indexed pak, a stored zip and a deflated zip; measure write time, size
//!   and the time to open and read every file (warm cache) against loose
//!   files; hash throughput of xxh3-128, blake3 and sha256.
//! - `app <out_dir> <icon.png>`: wrap this binary in a `.app` skeleton with
//!   a generated Info.plist and .icns, ad-hoc sign it with `codesign -s -`,
//!   verify, and launch it with `open`; the launched copy writes a marker.
//! - `marker <file>`: what the launched app runs (records exe path, args,
//!   and whether `Contents/Resources/game.pak` is visible).
//! - `pe <in.exe> <icon.png> <out.exe>`: patch icon, version info and
//!   subsystem into a Windows PE with `editpe`, then re-read them.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Launched as the bundle's executable: record the launch.
    if std::env::current_exe()?
        .to_string_lossy()
        .contains(".app/Contents/MacOS/")
    {
        return marker(&args);
    }
    match args.first().map(String::as_str) {
        Some("pak") => pak_bench(&args[1..]),
        Some("app") => app(Path::new(&args[1]), Path::new(&args[2])),
        Some("marker") => marker(&args[1..]),
        Some("pe") => pe(Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3])),
        _ => bail!("usage: s7_export pak|app|marker|pe ..."),
    }
}

// ------------------------------------------------------------------ files

/// Files a cooked game needs: box.project.json and assets/** (with .meta),
/// leaving out hidden entries (.box/, .vscode/), authoring/ and docs.
fn project_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = vec!["box.project.json".to_string()];
    walk(root, &root.join("assets"), &mut out);
    out.sort();
    out
}

// -------------------------------------------------------------- custom pak

const MAGIC: &[u8; 8] = b"BOXPAK\r\n";
const HEADER: usize = 64;

fn xxh3(bytes: &[u8]) -> [u8; 16] {
    twox_hash::XxHash3_128::oneshot(bytes).to_le_bytes()
}

/// Header (64 bytes): magic, version u32, count u32, index offset u64,
/// index length u64, xxh3-128 of the index, reserved. Data: files back to
/// back, 16-byte aligned. Index (sorted by path): u16 path length, path,
/// u64 offset, u64 length, xxh3-128 of the content.
fn write_pak(root: &Path, files: &[String], out: &Path) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(out)?);
    f.write_all(&[0u8; HEADER])?;
    let mut offset = HEADER as u64;
    let mut index = Vec::new();
    for rel in files {
        let bytes = std::fs::read(root.join(rel))?;
        let pad = (16 - offset % 16) % 16;
        f.write_all(&vec![0u8; pad as usize])?;
        offset += pad;
        f.write_all(&bytes)?;
        index.extend_from_slice(&(rel.len() as u16).to_le_bytes());
        index.extend_from_slice(rel.as_bytes());
        index.extend_from_slice(&offset.to_le_bytes());
        index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        index.extend_from_slice(&xxh3(&bytes));
        offset += bytes.len() as u64;
    }
    f.write_all(&index)?;
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&(files.len() as u32).to_le_bytes());
    header.extend_from_slice(&offset.to_le_bytes());
    header.extend_from_slice(&(index.len() as u64).to_le_bytes());
    header.extend_from_slice(&xxh3(&index));
    header.resize(HEADER, 0);
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&header)?;
    f.flush()?;
    Ok(())
}

struct Pak {
    file: std::fs::File,
    entries: BTreeMap<String, (u64, u64, [u8; 16])>,
}

fn open_pak(path: &Path) -> Result<Pak> {
    let mut file = std::fs::File::open(path)?;
    let mut h = [0u8; HEADER];
    file.read_exact(&mut h)?;
    if &h[0..8] != MAGIC {
        bail!("not a pak");
    }
    let count = u32::from_le_bytes(h[12..16].try_into()?) as usize;
    let off = u64::from_le_bytes(h[16..24].try_into()?);
    let len = u64::from_le_bytes(h[24..32].try_into()?) as usize;
    let mut index = vec![0u8; len];
    file.seek(SeekFrom::Start(off))?;
    file.read_exact(&mut index)?;
    if xxh3(&index) != h[32..48] {
        bail!("index hash mismatch");
    }
    let mut entries = BTreeMap::new();
    let mut i = 0;
    for _ in 0..count {
        let n = u16::from_le_bytes(index[i..i + 2].try_into()?) as usize;
        let name = String::from_utf8(index[i + 2..i + 2 + n].to_vec())?;
        i += 2 + n;
        let o = u64::from_le_bytes(index[i..i + 8].try_into()?);
        let l = u64::from_le_bytes(index[i + 8..i + 16].try_into()?);
        let hash: [u8; 16] = index[i + 16..i + 32].try_into()?;
        i += 32;
        entries.insert(name, (o, l, hash));
    }
    Ok(Pak { file, entries })
}

fn pak_read(pak: &mut Pak, name: &str, verify: bool) -> Result<Vec<u8>> {
    let (o, l, hash) = *pak.entries.get(name).context("missing")?;
    let mut buf = vec![0u8; l as usize];
    pak.file.seek(SeekFrom::Start(o))?;
    pak.file.read_exact(&mut buf)?;
    if verify && xxh3(&buf) != hash {
        bail!("{name}: content hash mismatch");
    }
    Ok(buf)
}

// --------------------------------------------------------------------- zip

fn write_zip(root: &Path, files: &[String], out: &Path, deflate: bool) -> Result<()> {
    let mut z = zip::ZipWriter::new(std::fs::File::create(out)?);
    let method = if deflate {
        zip::CompressionMethod::Deflated
    } else {
        zip::CompressionMethod::Stored
    };
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default());
    for rel in files {
        z.start_file(rel.as_str(), opts)?;
        z.write_all(&std::fs::read(root.join(rel))?)?;
    }
    z.finish()?;
    Ok(())
}

// ------------------------------------------------------------- benchmarks

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn time(n: usize, mut f: impl FnMut() -> Result<usize>) -> Result<(Duration, usize)> {
    let mut times = Vec::new();
    let mut bytes = 0;
    for _ in 0..n {
        let t = Instant::now();
        bytes = f()?;
        times.push(t.elapsed());
    }
    Ok((median(times), bytes))
}

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1000.0)
}

fn synthetic(dir: &Path) -> Result<()> {
    // 2,000 small files (1-64 KiB) and 8 large ones (8 MiB): a mid-size game.
    std::fs::create_dir_all(dir.join("assets/small"))?;
    std::fs::create_dir_all(dir.join("assets/large"))?;
    std::fs::write(dir.join("box.project.json"), b"{}")?;
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let mut rnd = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                // Half-compressible content.
                if i % 2 == 0 { (x & 0xff) as u8 } else { 0 }
            })
            .collect()
    };
    for i in 0..2000 {
        let size = 1024 + (i * 7919) % (63 * 1024);
        std::fs::write(dir.join(format!("assets/small/f{i:04}.bin")), rnd(size))?;
    }
    for i in 0..8 {
        std::fs::write(dir.join(format!("assets/large/t{i}.bin")), rnd(8 << 20))?;
    }
    Ok(())
}

fn pak_bench(args: &[String]) -> Result<()> {
    let scratch = std::env::temp_dir().join("s7_pak");
    std::fs::create_dir_all(&scratch)?;
    let mut projects: Vec<PathBuf> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--synthetic" {
            let dir = PathBuf::from(it.next().context("--synthetic <dir>")?);
            if !dir.join("box.project.json").exists() {
                synthetic(&dir)?;
            }
            projects.push(dir);
        } else {
            projects.push(a.into());
        }
    }
    println!(
        "| project | files | loose bytes | pak bytes | zip stored | zip deflate | write pak / zip stored / zip deflate | read all: loose | pak (verify) | pak (no verify) | zip stored | zip deflate | open pak index |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for root in &projects {
        let name = root.file_name().unwrap().to_string_lossy().into_owned();
        let files = project_files(root);
        let loose: u64 = files
            .iter()
            .map(|f| std::fs::metadata(root.join(f)).unwrap().len())
            .sum();
        let pak = scratch.join(format!("{name}.pak"));
        let zs = scratch.join(format!("{name}.stored.zip"));
        let zd = scratch.join(format!("{name}.deflate.zip"));
        let t = Instant::now();
        write_pak(root, &files, &pak)?;
        let w_pak = t.elapsed();
        let t = Instant::now();
        write_zip(root, &files, &zs, false)?;
        let w_zs = t.elapsed();
        let t = Instant::now();
        write_zip(root, &files, &zd, true)?;
        let w_zd = t.elapsed();
        // Determinism: same input, same bytes.
        let again = scratch.join(format!("{name}.again.pak"));
        write_pak(root, &files, &again)?;
        assert_eq!(std::fs::read(&pak)?, std::fs::read(&again)?, "pak not deterministic");
        let n = if loose > 50 << 20 { 5 } else { 31 };
        let (r_loose, b1) = time(n, || {
            let mut b = 0;
            for f in &files {
                b += std::fs::read(root.join(f))?.len();
            }
            Ok(b)
        })?;
        let read_pak = |verify: bool| {
            time(n, || {
                let mut p = open_pak(&pak)?;
                let names: Vec<String> = p.entries.keys().cloned().collect();
                let mut b = 0;
                for f in &names {
                    b += pak_read(&mut p, f, verify)?.len();
                }
                Ok(b)
            })
        };
        let (r_pak, b2) = read_pak(true)?;
        let (r_pak_nv, _) = read_pak(false)?;
        let (open_only, _) = time(n, || Ok(open_pak(&pak)?.entries.len()))?;
        let read_zip = |path: &Path| {
            time(n, || {
                let mut z = zip::ZipArchive::new(std::fs::File::open(path)?)?;
                let mut b = 0;
                for i in 0..z.len() {
                    let mut e = z.by_index(i)?;
                    let mut v = Vec::with_capacity(e.size() as usize);
                    e.read_to_end(&mut v)?;
                    b += v.len();
                }
                Ok(b)
            })
        };
        let (r_zs, b3) = read_zip(&zs)?;
        let (r_zd, b4) = read_zip(&zd)?;
        assert!(b1 == b2 && b2 == b3 && b3 == b4);
        let size = |p: &Path| std::fs::metadata(p).unwrap().len();
        println!(
            "| {name} | {} | {loose} | {} | {} | {} | {} / {} / {} | {} | {} | {} | {} | {} | {} |",
            files.len(),
            size(&pak),
            size(&zs),
            size(&zd),
            ms(w_pak),
            ms(w_zs),
            ms(w_zd),
            ms(r_loose),
            ms(r_pak),
            ms(r_pak_nv),
            ms(r_zs),
            ms(r_zd),
            ms(open_only),
        );
    }
    // Hash throughput over 64 MiB.
    let buf: Vec<u8> = (0..64u32 << 20).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let gbs = |d: Duration| buf.len() as f64 / d.as_secs_f64() / 1e9;
    let (d, _) = time(5, || Ok(xxh3(&buf)[0] as usize))?;
    println!("\nxxh3-128: {:.1} GB/s", gbs(d));
    let (d, _) = time(5, || Ok(blake3::hash(&buf).as_bytes()[0] as usize))?;
    println!("blake3 (pure Rust, 1 thread): {:.1} GB/s", gbs(d));
    let (d, _) = time(5, || {
        use sha2::Digest;
        Ok(sha2::Sha256::digest(&buf)[0] as usize)
    })?;
    println!("sha256: {:.1} GB/s", gbs(d));
    // Corruption: flip one byte in the data area and in the index.
    let root = &projects[0];
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    let pak = scratch.join(format!("{name}.pak"));
    let mut bytes = std::fs::read(&pak)?;
    bytes[HEADER + 3] ^= 0x40;
    let bad = scratch.join("bad_data.pak");
    std::fs::write(&bad, &bytes)?;
    let mut p = open_pak(&bad)?;
    let first = p.entries.keys().next().unwrap().clone();
    println!(
        "flipped data byte: {}",
        pak_read(&mut p, &first, true).err().map_or("NOT DETECTED".into(), |e| e.to_string())
    );
    let mut bytes = std::fs::read(&pak)?;
    let n = bytes.len();
    bytes[n - 5] ^= 0x01;
    std::fs::write(&bad, &bytes)?;
    println!(
        "flipped index byte: {}",
        open_pak(&bad).err().map_or("NOT DETECTED".into(), |e| e.to_string())
    );
    Ok(())
}

// --------------------------------------------------------------------- app

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("{cmd:?}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("$ {cmd:?} -> {}\n{text}", out.status);
    if !out.status.success() {
        bail!("{cmd:?} failed");
    }
    Ok(text)
}

fn plist(name: &str, exe: &str, id: &str, version: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleDisplayName</key><string>{name}</string>
	<key>CFBundleExecutable</key><string>{exe}</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIdentifier</key><string>{id}</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>{name}</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>{version}</string>
	<key>CFBundleVersion</key><string>{version}</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#,
        name = esc(name),
        exe = esc(exe),
        id = esc(id),
        version = esc(version)
    )
}

fn app(out: &Path, icon: &Path) -> Result<()> {
    let app = out.join("S7 Spike & Test.app");
    let _ = std::fs::remove_dir_all(&app);
    let macos = app.join("Contents/MacOS");
    let res = app.join("Contents/Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&res)?;
    let exe = "s7-spike";
    std::fs::copy(std::env::current_exe()?, macos.join(exe))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        plist("S7 Spike & Test", exe, "dev.box-engine.s7-spike", "0.1.0"),
    )?;
    std::fs::write(res.join("game.pak"), b"BOXPAK\r\n")?;
    // .icns with tools macOS ships (sips, iconutil).
    let iconset = out.join("AppIcon.iconset");
    std::fs::create_dir_all(&iconset)?;
    for size in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let px = (size * scale).to_string();
            run(Command::new("sips")
                .args(["-z", &px, &px])
                .arg(icon)
                .arg("--out")
                .arg(iconset.join(format!("icon_{size}x{size}{suffix}.png"))))?;
        }
    }
    run(Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&iconset)
        .arg("-o")
        .arg(res.join("AppIcon.icns")))?;
    std::fs::remove_dir_all(&iconset)?;
    // The binary as built is already linker-signed (ad-hoc).
    run(Command::new("codesign").args(["-dv"]).arg(macos.join(exe)))?;
    let t = Instant::now();
    run(Command::new("codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(&app))?;
    println!("sign: {}", ms(t.elapsed()));
    run(Command::new("codesign").args(["--verify", "--strict", "--verbose=2"]).arg(&app))?;
    run(Command::new("codesign").args(["-dv"]).arg(&app))?;
    // Gatekeeper's view: ad-hoc is not notarized; expect a rejection.
    let _ = run(Command::new("spctl").args(["-a", "-vv", "-t", "exec"]).arg(&app));
    // Launch through LaunchServices, as Finder does.
    let marker = out.join("marker.txt");
    let _ = std::fs::remove_file(&marker);
    run(Command::new("open")
        .args(["-W", "-n"])
        .arg(&app)
        .arg("--args")
        .arg(&marker))?;
    println!(
        "marker: {}",
        std::fs::read_to_string(&marker).unwrap_or_else(|_| "MISSING".into())
    );
    // Editing a signed bundle breaks the signature.
    let plist_path = app.join("Contents/Info.plist");
    let text = std::fs::read_to_string(&plist_path)?;
    std::fs::write(&plist_path, text.replace("0.1.0", "0.1.1"))?;
    let broken = Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(&app)
        .output()?;
    println!(
        "verify after editing Info.plist: {} {}",
        broken.status,
        String::from_utf8_lossy(&broken.stderr).trim()
    );
    Ok(())
}

fn marker(args: &[String]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let pak = exe
        .parent()
        .and_then(Path::parent)
        .map(|c| c.join("Resources/game.pak"));
    let text = format!(
        "exe={}\nargs={args:?}\ncwd={:?}\npak_visible={}\n",
        exe.display(),
        std::env::current_dir().ok(),
        pak.as_deref().is_some_and(Path::is_file)
    );
    if let Some(path) = args.first() {
        std::fs::write(path, text)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------- pe

fn pe(input: &Path, icon: &Path, output: &Path) -> Result<()> {
    use editpe::{Image, VersionInfo, VersionStringTable, constants::RT_VERSION};
    let _ = RT_VERSION;
    let t = Instant::now();
    let mut image = Image::parse_file(input)?;
    println!(
        "in: {} bytes, subsystem {}, resources: {}",
        std::fs::metadata(input)?.len(),
        image.subsystem(),
        image.resource_directory().is_some()
    );
    let mut res = image.resource_directory().cloned().unwrap_or_default();
    let png = image::open(icon)?;
    res.set_main_icon(png)?;
    let mut info = VersionInfo::default();
    info.info.file_version = editpe::types::VersionU32 { major: 1 << 16 | 2, minor: 3 << 16 };
    info.info.product_version = info.info.file_version;
    let mut strings = editpe::types::VersionU16 { major: 0x0409, minor: 0x04b0 };
    let _ = &mut strings;
    let mut table = VersionStringTable {
        key: "040904b0".into(),
        strings: Default::default(),
    };
    for (k, v) in [
        ("ProductName", "S7 Spike"),
        ("FileDescription", "S7 Spike"),
        ("CompanyName", "box-engine"),
        ("ProductVersion", "1.2.3"),
        ("FileVersion", "1.2.3"),
        ("OriginalFilename", "S7 Spike.exe"),
    ] {
        table.strings.insert(k.into(), v.into());
    }
    info.strings.push(table);
    info.vars.push(strings);
    res.set_version_info(&info)?;
    image.set_resource_directory(res)?;
    let old = image.set_subsystem(2); // IMAGE_SUBSYSTEM_WINDOWS_GUI: no console
    image.write_file(output)?;
    println!("patched in {} (subsystem {old} -> 2)", ms(t.elapsed()));
    let back = Image::parse_file(output)?;
    let res = back.resource_directory().context("no resources after patch")?;
    let icon = res.get_main_icon()?.map(<[u8]>::len);
    let version = res.get_version_info()?;
    println!(
        "out: {} bytes, subsystem {}, main icon {:?} bytes, version strings {:?}",
        std::fs::metadata(output)?.len(),
        back.subsystem(),
        icon,
        version.map(|v| v.strings.first().map(|t| t.strings.clone()))
    );
    run(Command::new("file").arg(output))?;
    Ok(())
}
