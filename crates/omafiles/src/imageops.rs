//! Turning a picture into something the clipboard can carry: PNG bytes, at
//! the original size or scaled down.
//!
//! Through `ffmpeg`, the way video posters already are, rather than a decoder
//! crate: gpui floats (see the root `Cargo.toml`) and carries its own copy of
//! `image`, so a second one is a version-skew risk for two integers and a
//! resize. ffmpeg is a runtime dependency already, and it reads every format
//! the preview shows. **Blocking** — background executors only.

use std::path::Path;
use std::process::Command;

/// Widths a copy is offered at, besides the original. Descending, and only
/// the ones smaller than the picture — a 640 px screenshot gets no "1280".
const SCALED_WIDTHS: [u32; 4] = [1920, 1280, 800, 400];

/// One way to copy a picture: as a PNG, at this many pixels across.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub width: u32,
    pub height: u32,
    /// The original, byte for byte, when the file is already a PNG; else a
    /// re-encode at full size.
    pub original: bool,
}

/// The PNG variants worth offering for a `width × height` picture, largest
/// first: the original size, then each scaled width that is actually smaller.
pub fn variants(width: u32, height: u32) -> Vec<Variant> {
    let mut out = vec![Variant {
        width,
        height,
        original: true,
    }];
    if width == 0 || height == 0 {
        return out;
    }
    for &w in &SCALED_WIDTHS {
        if w < width {
            // The same rounding ffmpeg's `-2` does: even, never zero.
            let h = ((u64::from(height) * u64::from(w) + u64::from(width) / 2) / u64::from(width))
                .max(2) as u32;
            let h = h + (h % 2);
            out.push(Variant {
                width: w,
                height: h,
                original: false,
            });
        }
    }
    out
}

/// Pixel dimensions, from `ffprobe`. For the formats whose headers
/// `preview` reads directly this is never needed; it is the answer for the
/// rest (webp, tiff, avif).
pub fn probe_dimensions(path: &Path) -> Option<(u32, u32)> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim().split(',');
    let w = parts.next()?.trim().parse().ok()?;
    let h = parts.next()?.trim().parse().ok()?;
    Some((w, h))
}

/// The picture as PNG bytes, scaled to `width` across when given.
///
/// A PNG asked for at its own size is read, not re-encoded: the bytes on
/// disk are the answer, and re-encoding could only make them different.
pub fn png_bytes(path: &Path, width: Option<u32>) -> Result<Vec<u8>, String> {
    if width.is_none() && is_png(path) {
        return std::fs::read(path).map_err(|err| format!("could not read the file: {err}"));
    }
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-i"]).arg(path);
    if let Some(width) = width {
        command.args(["-vf", &format!("scale={width}:-2")]);
    }
    let output = command
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "install ffmpeg to copy pictures as PNG".to_string()
            } else {
                format!("could not run ffmpeg: {err}")
            }
        })?;
    if !output.status.success() || output.stdout.is_empty() {
        let why = String::from_utf8_lossy(&output.stderr);
        let why = why.lines().last().unwrap_or("ffmpeg produced nothing").trim();
        return Err(format!("could not convert to PNG: {why}"));
    }
    Ok(output.stdout)
}

/// Put PNG bytes on the system clipboard as `image/png`, through
/// `wl-copy`.
///
/// Not gpui's clipboard: on Wayland it offers text mime types only and
/// serves an image entry as nothing, so a browser sees no picture to paste.
/// `wl-copy` reads stdin, then forks to keep serving the selection after
/// this returns — which is what lets the paste happen after the modal has
/// long closed. wl-clipboard is part of Omarchy's base install.
pub fn copy_png_to_clipboard(bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::Stdio;
    // Nothing of wl-copy's is piped back: the server it forks inherits every
    // fd, and a pipe it holds open would keep a `wait` here waiting for as
    // long as the clipboard holds the picture — which froze the window.
    let mut child = Command::new("wl-copy")
        .args(["--type", "image/png"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "install wl-clipboard to copy pictures".to_string()
            } else {
                format!("could not run wl-copy: {err}")
            }
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .map_err(|err| format!("could not hand the picture to wl-copy: {err}"))?;
    }
    let status = child
        .wait()
        .map_err(|err| format!("wl-copy failed: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy refused ({status})"))
    }
}

fn is_png(path: &Path) -> bool {
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read as _;
            let mut head = [0u8; 8];
            f.read_exact(&mut head)?;
            Ok(head == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_offer_only_smaller_widths_largest_first() {
        let v = variants(1600, 900);
        let widths: Vec<u32> = v.iter().map(|v| v.width).collect();
        assert_eq!(widths, vec![1600, 1280, 800, 400]);
        assert!(v[0].original);
        assert!(v.iter().skip(1).all(|v| !v.original));
        // Heights keep the ratio and stay even, as ffmpeg's `-2` does.
        assert_eq!(v[1].height, 720);
        assert_eq!(v[2].height, 450);
        assert_eq!(v[3].height, 226);
    }

    #[test]
    fn a_small_picture_has_only_itself() {
        assert_eq!(variants(320, 200).len(), 1);
        assert_eq!(variants(0, 0).len(), 1);
    }

    #[test]
    fn a_png_at_its_own_size_is_read_verbatim() {
        let dir = std::env::temp_dir().join(format!("omafiles-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        // Not a valid picture past the signature — which is the point: nothing
        // decodes it, the bytes come straight back.
        let bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(png_bytes(&path, None).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scaled_copy_is_a_png_of_that_width() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("omafiles-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("src.png");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=red:s=64x32", "-frames:v", "1"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(made.success());
        let scaled = png_bytes(&source, Some(32)).unwrap();
        assert_eq!(&scaled[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let w = u32::from_be_bytes(scaled[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(scaled[20..24].try_into().unwrap());
        assert_eq!((w, h), (32, 16));
        assert_eq!(probe_dimensions(&source), Some((64, 32)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
