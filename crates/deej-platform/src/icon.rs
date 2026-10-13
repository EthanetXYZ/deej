//! App icons for the mixer: an executable's icon as a PNG data URL.

/// The 32×32 icon of an executable as `data:image/png;base64,...`, or `None` if Windows has none for it.
/// Call from a thread with COM initialized (the shell may need it).
#[cfg(windows)]
pub fn exe_icon_png(path: &str) -> Option<String> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits,
    };
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    use windows::core::HSTRING;

    const SIZE: i32 = 32;

    // SAFETY: every handle obtained here is released before returning; buffers outlive the calls using them
    unsafe {
        let mut info = SHFILEINFOW::default();
        let found = SHGetFileInfoW(
            &HSTRING::from(path),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if found == 0 || info.hIcon.is_invalid() {
            return None;
        }

        let mut icon = ICONINFO::default();
        let have_info = GetIconInfo(info.hIcon, &mut icon).is_ok();
        let _ = DestroyIcon(info.hIcon);
        if !have_info {
            return None;
        }

        let dc = CreateCompatibleDC(None);
        let read = |bitmap| {
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: SIZE,
                    biHeight: -SIZE, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
            let lines =
                GetDIBits(dc, bitmap, 0, SIZE as u32, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS);
            (lines == SIZE).then_some(pixels)
        };

        let color = (!icon.hbmColor.is_invalid()).then(|| read(icon.hbmColor)).flatten();
        let mask = (!icon.hbmMask.is_invalid()).then(|| read(icon.hbmMask)).flatten();

        let _ = DeleteDC(dc);
        if !icon.hbmColor.is_invalid() {
            let _ = DeleteObject(icon.hbmColor.into());
        }
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(icon.hbmMask.into());
        }

        let mut bgra = color?;

        // old-style icons have no alpha channel: take transparency from the mask instead
        if bgra.chunks(4).all(|p| p[3] == 0) {
            let mask = mask?;
            for (pixel, m) in bgra.chunks_mut(4).zip(mask.chunks(4)) {
                pixel[3] = if m[0] == 0 { 255 } else { 0 };
            }
        }

        let rgba: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        Some(format!("data:image/png;base64,{}", base64(&png(SIZE as u32, SIZE as u32, &rgba))))
    }
}

#[cfg(not(windows))]
pub fn exe_icon_png(_path: &str) -> Option<String> {
    None
}

/// A minimal PNG encoder (uncompressed deflate). Icons are 4 KB, so compression isn't worth a dependency.
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }

    // zlib stream with stored blocks of up to 65535 bytes
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i == blocks.len() - 1));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    for (kind, data) in [(b"IHDR", &ihdr[..]), (b"IDAT", &zlib[..]), (b"IEND", &[][..])] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_and_base64_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn png_structure() {
        let image = png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&image[12..16], b"IHDR");
        assert!(image.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND chunk with its CRC");
    }

    #[cfg(windows)]
    #[test]
    fn extracts_an_icon_from_a_system_executable() {
        let explorer = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()) + r"\explorer.exe";
        let url = exe_icon_png(&explorer).expect("explorer.exe has an icon");
        assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
    }
}
