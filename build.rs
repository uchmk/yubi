//! Embeds the app icon (drawn by `src/appicon_draw.rs`) as the exe's icon resource.

include!("src/appicon_draw.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/appicon_draw.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("yubi.ico");
    std::fs::write(&ico, make_ico(&[16, 20, 24, 32, 40, 48, 64, 256])).expect("write ico");
    let rc = out.join("yubi.rc");
    let path = ico
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write rc");
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .expect("compile resource");
}

/// A multi-size .ico of uncompressed 32-bit DIB entries (with an all-opaque AND mask; the
/// alpha channel carries the shape).
fn make_ico(sizes: &[u32]) -> Vec<u8> {
    let mut images = Vec::new();
    for &n in sizes {
        let rgba = render_rgba(n, false);
        let mut img = Vec::new();
        // BITMAPINFOHEADER: height counts the XOR bitmap and the AND mask.
        img.extend_from_slice(&40u32.to_le_bytes());
        img.extend_from_slice(&(n as i32).to_le_bytes());
        img.extend_from_slice(&((n * 2) as i32).to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes());
        img.extend_from_slice(&32u16.to_le_bytes());
        img.extend_from_slice(&[0u8; 24]);
        // Bottom-up BGRA.
        for y in (0..n).rev() {
            for x in 0..n {
                let i = ((y * n + x) * 4) as usize;
                img.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // AND mask: zeros (alpha decides), rows padded to 4 bytes.
        let row = (n as usize).div_ceil(32) * 4;
        img.extend(std::iter::repeat_n(0u8, row * n as usize));
        images.push((n, img));
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (n, img) in &images {
        let b = if *n >= 256 { 0 } else { *n as u8 };
        ico.extend_from_slice(&[b, b, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for (_, img) in images {
        ico.extend_from_slice(&img);
    }
    ico
}
