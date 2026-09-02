//! Reading a file into something previewable.
//!
//! Everything here runs off the main thread — `PLAN.md` §6.5 is explicit that a
//! preview must never block a keystroke — so this module is deliberately free of
//! any gpui context: it takes a path and returns data. The rendering half lives
//! in `main.rs`, which turns a [`Preview`] into elements at a
//! [`Target`](crate::preview::Target) size.
//!
//! The split matters more than it looks. §6.5 asks for one renderer used at two
//! sizes rather than a pane version and a fullscreen version, and the way to
//! guarantee that is for *loading* to be size-independent: there is exactly one
//! `Preview` per file, and the pane and the overlay disagree only about how much
//! of it to show.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use gpui::{Image, ImageFormat};

use crate::entry::{Entry, Kind};

/// Above this, a file is described rather than shown.
///
/// §6.5's cap. It applies identically fullscreen: a 10 MB text file is no more
/// previewable large than small.
pub const MAX_PREVIEW_BYTES: u64 = 10 * 1024 * 1024;

/// Images decode into GPU textures, so the ceiling is lower than for text.
/// A 40 MP PNG is 160 MB of RGBA before it is drawn.
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// Lines kept from a text or code file.
///
/// Tree-sitter parses the whole buffer and the renderer builds one element per
/// line, so an unbounded file is a frame-time cliff. Cutting at a fixed line
/// count and saying so is honest; silently showing a prefix is not.
pub const MAX_LINES: usize = 4_000;

/// Bytes of a binary file shown as hex.
pub const HEX_BYTES: usize = 512;

/// What identifies a loaded preview.
///
/// `(path, mtime)` rather than the path alone, because §6.5 and M0's finding #3
/// both say so: gpui caches decoded images by path and ignores content, so a
/// file rewritten in place shows the old picture until the key changes.
/// The rendered *size* is not part of the key — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
}

impl Key {
    pub fn of(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            mtime: entry.modified,
        }
    }
}

/// How much room the renderer has.
///
/// Not a pixel size: the two call sites differ in *kind* — the detail panel
/// shows a thumbnail beside a fact table, the expanded view shows the file — and
/// a pixel width would not capture that. §6.5's "function of (file, size)" with
/// the size reduced to the two cases that exist.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// The docked detail panel: a summary, with as much content as fits.
    /// Carries the cover's width, so a picture can be laid out at its exact
    /// size rather than left to float in a box of the wrong shape.
    Pane { width: f32 },
    /// The listing column, taken over by the preview.
    Expanded,
}

/// A file, read and classified.
#[derive(Debug, Clone)]
pub struct Preview {
    pub key: Key,
    pub name: String,
    pub size: Option<u64>,
    /// Independent of what the link resolves to, and worth stating: a symlink
    /// that previews as a 4 KB text file is not a 4 KB file.
    pub is_symlink: bool,
    pub body: Body,
}

#[derive(Debug, Clone)]
pub enum Body {
    /// Directories have no content to show, only a count.
    Directory {
        entries: Option<usize>,
    },
    Image {
        image: Arc<Image>,
        /// `None` when the header could not be read — a corrupt file, or a
        /// format the sniffer does not know. The image may still render.
        dimensions: Option<(u32, u32)>,
    },
    /// Rendered as markup by `TextView`, not shown as source.
    Markdown(String),
    /// Text, highlighted when a grammar is known.
    Text {
        text: String,
        /// A gpui-component grammar name, or `None` for plain text.
        language: Option<&'static str>,
        lines: usize,
        /// True when [`MAX_LINES`] cut it short.
        truncated: bool,
    },
    /// A changed file, shown as what changed (§6.9).
    ///
    /// Structured rather than the raw `git diff` text: the rows are rendered as
    /// lines of the file under a wash, syntax-highlighted by the file's own
    /// grammar, which is what a diff is for. See [`crate::git::Diff`].
    Diff(crate::git::Diff),
    /// A poster frame plus what `ffprobe` knows.
    Video {
        poster: Option<Arc<Image>>,
        /// The poster's pixel size, so it can be laid out at its own shape.
        poster_dimensions: Option<(u32, u32)>,
        facts: Vec<(String, String)>,
    },
    /// Not text: a hex head and a guess at what it is.
    Binary {
        head: Vec<u8>,
        kind: &'static str,
    },
    TooLarge {
        size: u64,
        limit: u64,
    },
    /// Read failed, or the file is not something we can open.
    Unreadable(String),
}

impl Body {
    /// Whether a fullscreen view would show more than the pane already does.
    ///
    /// §6.5 excludes the "too large" and binary states explicitly, and a
    /// directory has nothing to enlarge either.
    pub fn is_expandable(&self) -> bool {
        matches!(
            self,
            Body::Image { .. }
                | Body::Markdown(_)
                | Body::Text { .. }
                | Body::Video { .. }
                // A hunk rarely fits the detail panel, so this is the body
                // expanding was most worth having for.
                | Body::Diff(_)
        )
    }

    /// Whether the pane has anything visual to put in its cover area.
    ///
    /// The placeholder states — a directory, too-large, unreadable — hide
    /// the cover entirely rather than writing a sentence where a picture
    /// goes; the fact sheet's `kind` row already names what they are.
    pub fn has_cover(&self) -> bool {
        !matches!(
            self,
            Body::Directory { .. } | Body::TooLarge { .. } | Body::Unreadable(_)
        )
    }

    /// A one-word label for the fact table.
    pub fn label(&self) -> &'static str {
        match self {
            Body::Directory { .. } => "directory",
            Body::Image { .. } => "image",
            Body::Markdown(_) => "markdown",
            Body::Text { language, .. } => language.unwrap_or("text"),
            Body::Diff(_) => "diff",
            Body::Video { .. } => "video",
            Body::Binary { kind, .. } => kind,
            Body::TooLarge { .. } => "too large",
            Body::Unreadable(_) => "unreadable",
        }
    }
}

impl Preview {
    /// Read and classify. **Blocking** — call it on a background thread.
    pub fn load(entry: &Entry) -> Self {
        let key = Key::of(entry);
        let name = entry.name.clone();
        let size = entry.size;
        let body = Self::read_body(entry);
        Self {
            key,
            name,
            size,
            is_symlink: entry.is_symlink,
            body,
        }
    }

    fn read_body(entry: &Entry) -> Body {
        let path = &entry.path;
        match entry.kind {
            Kind::Directory => Body::Directory {
                entries: std::fs::read_dir(path).ok().map(|d| d.flatten().count()),
            },
            Kind::Unresolved => Body::Unreadable("broken link".to_string()),
            Kind::File => Self::read_file(path, entry.size.unwrap_or(0)),
        }
    }

    fn read_file(path: &Path, size: u64) -> Body {
        let class = classify(path);

        // The cap is checked per class, because the sensible ceiling differs:
        // decoding an image costs far more than reading the same bytes of text.
        let limit = match class {
            Class::Image => MAX_IMAGE_BYTES,
            _ => MAX_PREVIEW_BYTES,
        };
        // Video is exempt: nothing reads the whole file, only ffmpeg's first
        // frame, and every video worth previewing is over the cap.
        if size > limit && class != Class::Video {
            return Body::TooLarge { size, limit };
        }

        match class {
            Class::Video => read_video(path),
            Class::Image => match std::fs::read(path) {
                Ok(bytes) => match image_format(path, &bytes) {
                    Some(format) => Body::Image {
                        dimensions: image_dimensions(&bytes),
                        // Hashed by content, so a file rewritten in place gets a
                        // new id and gpui's cache cannot hand back the old
                        // decode. This is the other half of `Key`'s mtime.
                        image: Arc::new(Image::from_bytes(format, bytes)),
                    },
                    None => Body::Unreadable("unrecognised image format".to_string()),
                },
                Err(err) => Body::Unreadable(err.to_string()),
            },
            Class::Text(language) => match std::fs::read(path) {
                Err(err) => Body::Unreadable(err.to_string()),
                Ok(bytes) => match String::from_utf8(bytes) {
                    // An extension promised text and the bytes disagree. Falling
                    // through to the hex view is better than mangling it with a
                    // lossy conversion.
                    Err(err) => binary_body(err.into_bytes()),
                    Ok(text) => {
                        if language == Some("markdown") {
                            return Body::Markdown(truncate_lines(text, MAX_LINES).0);
                        }
                        let (text, lines, truncated) = {
                            let (text, truncated) = truncate_lines(text, MAX_LINES);
                            let lines = text.lines().count();
                            (text, lines, truncated)
                        };
                        Body::Text {
                            text,
                            language,
                            lines,
                            truncated,
                        }
                    }
                },
            },
            Class::Unknown => match std::fs::read(path) {
                Err(err) => Body::Unreadable(err.to_string()),
                // No extension to go on, so sniff. Config files and scripts
                // routinely have no suffix, and calling them binary would make
                // the preview useless exactly where it is most wanted.
                Ok(bytes) => match String::from_utf8(bytes) {
                    Err(err) => binary_body(err.into_bytes()),
                    Ok(text) if text.contains('\0') => binary_body(text.into_bytes()),
                    Ok(text) => {
                        let (text, truncated) = truncate_lines(text, MAX_LINES);
                        let lines = text.lines().count();
                        Body::Text {
                            text,
                            language: None,
                            lines,
                            truncated,
                        }
                    }
                },
            },
        }
    }
}

fn binary_body(bytes: Vec<u8>) -> Body {
    Body::Binary {
        kind: magic_kind(&bytes),
        head: bytes.into_iter().take(HEX_BYTES).collect(),
    }
}

/// Keep at most `max` lines, reporting whether anything was dropped.
fn truncate_lines(text: String, max: usize) -> (String, bool) {
    // Counting first avoids reallocating the common case, which is every file
    // short enough to show whole.
    if text.lines().take(max + 1).count() <= max {
        return (text, false);
    }
    let kept: Vec<&str> = text.lines().take(max).collect();
    (kept.join("\n"), true)
}

/// What kind of thing an extension promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Image,
    Video,
    /// Text, with the grammar to highlight it by if there is one.
    Text(Option<&'static str>),
    /// No extension, or one we have no rule for. Decided by sniffing.
    Unknown,
}

/// Extension → grammar name, using gpui-component's names.
///
/// Names it does not know degrade to unhighlighted text rather than failing, so
/// listing an extension here is never a risk — the worst case is that it stays
/// plain until the grammar is enabled in `Cargo.toml`.
const LANGUAGES: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("toml", "toml"),
    ("json", "json"),
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("fish", "bash"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "tsx"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("py", "python"),
    ("lua", "lua"),
    ("c", "c"),
    ("h", "c"),
    ("cpp", "cpp"),
    ("cc", "cpp"),
    ("hpp", "cpp"),
    ("go", "go"),
    ("css", "css"),
    ("html", "html"),
    ("htm", "html"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("diff", "diff"),
    ("patch", "diff"),
];

/// Extensions that are text but have no grammar enabled.
const PLAIN_TEXT: &[&str] = &[
    "txt",
    "log",
    "cfg",
    "conf",
    "ini",
    "env",
    "desktop",
    "service",
    "rules",
    "gitignore",
    "gitattributes",
    "editorconfig",
    "lock",
    "csv",
    "tsv",
    "sql",
    "xml",
    "svg",
    "qml",
    "kdl",
    "nix",
    "vim",
    "el",
    "org",
    "rst",
    "tex",
];

const IMAGES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif", "avif", "qoi", "hdr", "exr",
];

const VIDEOS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "wmv", "flv", "mpg", "mpeg", "ogv",
];

/// Whether the file is a picture the preview would draw — what makes the
/// copy verb offer PNG variants rather than the file alone.
pub fn is_image(path: &Path) -> bool {
    matches!(classify(path), Class::Image)
}

fn classify(path: &Path) -> Class {
    // `.gitignore` has no stem, so its "extension" is None and the name is the
    // extension. Checking both is what makes dotfiles preview as text.
    let extension = path
        .extension()
        .or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str()?.strip_prefix('.').map(|s| s.as_ref()))
        })
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);

    let Some(extension) = extension else {
        return Class::Unknown;
    };

    if IMAGES.contains(&extension.as_str()) {
        return Class::Image;
    }
    if VIDEOS.contains(&extension.as_str()) {
        return Class::Video;
    }
    if let Some((_, language)) = LANGUAGES.iter().find(|(e, _)| *e == extension) {
        // SVG is listed as plain text on purpose: gpui renders it through
        // resvg, but a 200 KB icon set is more useful read than drawn at 16 px,
        // and the source is what someone opening it in a file manager wants.
        return Class::Text(Some(language));
    }
    if PLAIN_TEXT.contains(&extension.as_str()) {
        return Class::Text(None);
    }
    Class::Unknown
}

/// The gpui format for an image, from its extension then its magic bytes.
///
/// gpui's `ImageFormat` is a closed set — avif, qoi, hdr and exr have no
/// variant even though `img()` can decode them from a path — so those are the
/// ones the sniff will fail on, and they end up as "unrecognised" rather than
/// silently mis-decoded.
fn image_format(path: &Path, bytes: &[u8]) -> Option<ImageFormat> {
    let by_extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .and_then(|e| match e.as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "gif" => Some(ImageFormat::Gif),
            "webp" => Some(ImageFormat::Webp),
            "bmp" => Some(ImageFormat::Bmp),
            "ico" => Some(ImageFormat::Ico),
            "tif" | "tiff" => Some(ImageFormat::Tiff),
            "svg" => Some(ImageFormat::Svg),
            _ => None,
        });
    // Trust the bytes over the name: a `.jpg` that is really a PNG is common
    // enough that decoding it by extension would be a visible bug.
    magic_image_format(bytes).or(by_extension)
}

fn magic_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some(ImageFormat::Png),
        [0xff, 0xd8, 0xff, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [b'B', b'M', ..] => Some(ImageFormat::Bmp),
        [b'I', b'I', 0x2a, 0x00, ..] | [b'M', b'M', 0x00, 0x2a, ..] => Some(ImageFormat::Tiff),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        _ => None,
    }
}

/// Pixel dimensions from an image header.
///
/// Hand-rolled rather than pulling in the `image` crate: only four formats are
/// worth the code, the headers are fixed-offset, and adding a decoder
/// dependency to read two integers would be a poor trade — especially against a
/// gpui that floats, where a second copy of `image` is a version-skew risk.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(o..o + 4)?.try_into().ok()?))
    };
    let le16 = |o: usize| -> Option<u32> {
        Some(u16::from_le_bytes(bytes.get(o..o + 2)?.try_into().ok()?) as u32)
    };

    match magic_image_format(bytes)? {
        // IHDR is always the first chunk, at a fixed offset.
        ImageFormat::Png => Some((be32(16)?, be32(20)?)),
        ImageFormat::Gif => Some((le16(6)?, le16(8)?)),
        ImageFormat::Bmp => {
            let w = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?);
            let h = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?);
            // A negative height means a top-down bitmap, not a negative size.
            Some((w.unsigned_abs(), h.unsigned_abs()))
        }
        // JPEG has no fixed header: walk the segment chain to the frame marker.
        ImageFormat::Jpeg => jpeg_dimensions(bytes),
        _ => None,
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // past the SOI
    // The frame header's last byte is at `i + 8`, so that is what must be in
    // range — one less than the segment's length.
    while i + 8 < bytes.len() {
        if bytes[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // SOF0..SOF15, excluding the four that are not frame headers.
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return Some((width, height));
        }
        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if length < 2 {
            return None; // malformed: a zero-length segment would not advance
        }
        i += 2 + length;
    }
    None
}

/// A rough type for a binary file, from its magic bytes.
fn magic_kind(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x7f, b'E', b'L', b'F', ..] => "executable",
        [b'P', b'K', 0x03, 0x04, ..] => "zip archive",
        [0x1f, 0x8b, ..] => "gzip",
        [0xfd, b'7', b'z', b'X', b'Z', ..] => "xz",
        [b'%', b'P', b'D', b'F', ..] => "pdf",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        [b'I', b'D', b'3', ..] => "audio",
        [b'f', b'L', b'a', b'C', ..] => "flac",
        [0x00, 0x61, 0x73, 0x6d, ..] => "wasm",
        _ => "binary",
    }
}

/// Poster frame and metadata, via ffmpeg.
///
/// §6.5 chose thumbnail-plus-metadata over inline playback for v1. Both tools
/// are optional at runtime: without them a video previews as a fact-free card
/// rather than an error, because "ffmpeg is not installed" is not the file
/// manager's problem to escalate.
fn read_video(path: &Path) -> Body {
    let poster = video_poster(path);
    // A poster is PNG from ffmpeg, so its header always answers.
    let poster_dimensions = poster.as_ref().and_then(|p| image_dimensions(&p.bytes));
    Body::Video {
        poster,
        poster_dimensions,
        facts: video_facts(path),
    }
}

fn video_facts(path: &Path) -> Vec<(String, String)> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,codec_name,r_frame_rate:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);

    // `default=noprint_wrappers=1` prints one `key=value` per line.
    let field = |key: &str| -> Option<String> {
        let prefix = format!("{key}=");
        text.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "N/A")
            .map(str::to_string)
    };

    let mut facts = Vec::new();
    if let (Some(w), Some(h)) = (field("width"), field("height")) {
        facts.push(("resolution".to_string(), format!("{w} × {h}")));
    }
    if let Some(codec) = field("codec_name") {
        facts.push(("codec".to_string(), codec));
    }
    if let Some(duration) = field("duration").and_then(|d| d.parse::<f64>().ok()) {
        facts.push(("duration".to_string(), format_duration(duration)));
    }
    if let Some(rate) = field("r_frame_rate").and_then(|r| parse_frame_rate(&r)) {
        facts.push(("frame rate".to_string(), format!("{rate:.0} fps")));
    }
    // Deliberately no size: the caller already reports the file's size, and
    // asking ffprobe for it a second time only produced a duplicate row.
    facts
}

/// ffprobe reports the rate as a rational, e.g. `30000/1001`.
fn parse_frame_rate(raw: &str) -> Option<f64> {
    let (num, den) = raw.split_once('/')?;
    let (num, den) = (num.parse::<f64>().ok()?, den.parse::<f64>().ok()?);
    (den != 0.0).then(|| num / den)
}

pub fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn video_poster(path: &Path) -> Option<Arc<Image>> {
    // One second in, so the frame is past the fade-from-black most videos open
    // with, then from the very start for anything shorter than that.
    for seek in ["1", "0"] {
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-ss", seek, "-i"])
            .arg(path)
            .args([
                "-frames:v",
                "1",
                // Cap the long edge: a 4K poster is decoded and uploaded to the
                // GPU for a thumbnail nobody looks at closely.
                "-vf",
                "scale='min(1280,iw)':-2",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-",
            ])
            .output()
            .ok()?;

        if output.status.success() && !output.stdout.is_empty() {
            return Some(Arc::new(Image::from_bytes(ImageFormat::Png, output.stdout)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omafiles-preview-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn preview(path: &Path) -> Preview {
        Preview::load(&Entry::from_path(path.to_path_buf()))
    }

    #[test]
    fn classifies_by_extension_and_falls_back_to_sniffing() {
        assert_eq!(classify(Path::new("a/b.rs")), Class::Text(Some("rust")));
        assert_eq!(classify(Path::new("a/B.RS")), Class::Text(Some("rust")));
        assert_eq!(classify(Path::new("a/b.png")), Class::Image);
        assert_eq!(classify(Path::new("a/b.mkv")), Class::Video);
        assert_eq!(classify(Path::new("a/b.txt")), Class::Text(None));
        // A dotfile's name is its extension, or every config file in ~/.config
        // would preview as binary.
        assert_eq!(classify(Path::new("a/.gitignore")), Class::Text(None));
        // No suffix at all: decided by content, not by guessing here.
        assert_eq!(classify(Path::new("a/README")), Class::Unknown);
    }

    #[test]
    fn a_source_file_carries_its_grammar() {
        let dir = temp("code");
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();

        match preview(&path).body {
            Body::Text {
                language, lines, ..
            } => {
                assert_eq!(language, Some("rust"));
                assert_eq!(lines, 3);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_is_kept_as_markup_not_source() {
        let dir = temp("md");
        let path = dir.join("README.md");
        std::fs::write(&path, "# Title\n\nBody.\n").unwrap();
        assert!(matches!(preview(&path).body, Body::Markdown(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extensionless_text_file_is_sniffed_as_text() {
        let dir = temp("sniff");
        let path = dir.join("README");
        std::fs::write(&path, "plain words\n").unwrap();
        match preview(&path).body {
            Body::Text { language, .. } => assert_eq!(language, None),
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binary_file_gets_a_hex_head_and_a_guess() {
        let dir = temp("bin");
        let path = dir.join("prog");
        let mut bytes = vec![0x7f, b'E', b'L', b'F'];
        bytes.extend(std::iter::repeat_n(0u8, 4096));
        std::fs::write(&path, &bytes).unwrap();

        match preview(&path).body {
            Body::Binary { head, kind } => {
                assert_eq!(kind, "executable");
                assert_eq!(head.len(), HEX_BYTES, "the head is capped, not the file");
            }
            other => panic!("expected binary, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_text_extension_holding_binary_falls_through_to_hex() {
        // A `.txt` full of NULs must not be mangled into lossy UTF-8.
        let dir = temp("liar");
        let path = dir.join("notes.txt");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x01, 0x02]).unwrap();
        assert!(matches!(preview(&path).body, Body::Binary { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_files_are_described_not_read() {
        let dir = temp("big");
        let path = dir.join("huge.txt");
        // Sparse, so the test does not actually write 11 MB.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_PREVIEW_BYTES + 1).unwrap();
        drop(file);

        match preview(&path).body {
            Body::TooLarge { limit, .. } => assert_eq!(limit, MAX_PREVIEW_BYTES),
            other => panic!("expected too-large, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_files_are_cut_at_the_line_cap_and_say_so() {
        let text = (0..MAX_LINES + 50)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let (kept, truncated) = truncate_lines(text, MAX_LINES);
        assert!(truncated);
        assert_eq!(kept.lines().count(), MAX_LINES);

        let (kept, truncated) = truncate_lines("a\nb\n".to_string(), MAX_LINES);
        assert!(!truncated);
        assert_eq!(kept, "a\nb\n", "a short file is not rewritten");
    }

    #[test]
    fn image_headers_yield_dimensions() {
        // A 1×1 PNG, byte for byte.
        let png: [u8; 67] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        assert_eq!(image_dimensions(&png), Some((1, 1)));
        assert_eq!(magic_image_format(&png), Some(ImageFormat::Png));

        // GIF87a, 3×2, little-endian in the header.
        let gif = b"GIF87a\x03\x00\x02\x00\x00\x00\x00";
        assert_eq!(image_dimensions(gif), Some((3, 2)));

        // Truncated input must return None, never panic or index out of range.
        assert_eq!(image_dimensions(&png[..10]), None);
        assert_eq!(image_dimensions(&[]), None);
    }

    #[test]
    fn jpeg_dimensions_walk_the_segment_chain() {
        // SOI, an APP0 segment to skip over, then SOF0 with 4×8.
        let mut bytes = vec![0xff, 0xd8];
        bytes.extend([0xff, 0xe0, 0x00, 0x04, 0x00, 0x00]); // APP0, length 4
        bytes.extend([0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x08, 0x00, 0x04]);
        assert_eq!(jpeg_dimensions(&bytes), Some((4, 8)));

        // A zero-length segment must terminate rather than loop forever.
        let stuck = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x00, 0, 0, 0, 0, 0, 0];
        assert_eq!(jpeg_dimensions(&stuck), None);
    }

    #[test]
    fn the_magic_beats_the_extension() {
        // A PNG named `.jpg` is common enough that decoding by name is a bug.
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(
            image_format(Path::new("x.jpg"), &png),
            Some(ImageFormat::Png)
        );
        // With no magic to go on, the name is all there is.
        assert_eq!(
            image_format(Path::new("x.jpg"), &[0, 0, 0]),
            Some(ImageFormat::Jpeg)
        );
    }

    #[test]
    fn a_directory_previews_as_a_count() {
        let dir = temp("dir");
        std::fs::write(dir.join("a"), b"1").unwrap();
        std::fs::write(dir.join("b"), b"2").unwrap();
        match preview(&dir).body {
            Body::Directory { entries } => assert_eq!(entries, Some(2)),
            other => panic!("expected directory, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_states_with_more_to_show_offer_fullscreen() {
        // §6.5 excludes these two explicitly: they say all they have to say in
        // a pane, and an overlay of the same three lines is a worse pane.
        assert!(!Body::TooLarge { size: 1, limit: 1 }.is_expandable());
        assert!(
            !Body::Binary {
                head: vec![],
                kind: "binary"
            }
            .is_expandable()
        );
        assert!(!Body::Directory { entries: None }.is_expandable());
        assert!(Body::Markdown(String::new()).is_expandable());
    }

    #[test]
    fn frame_rates_and_durations_format_sensibly() {
        assert_eq!(
            parse_frame_rate("30000/1001").map(|r| r.round()),
            Some(30.0)
        );
        assert_eq!(parse_frame_rate("25/1"), Some(25.0));
        // A still image reports 0/0, which must not divide by zero.
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(parse_frame_rate("nonsense"), None);

        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(65.4), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
        assert_eq!(format_duration(-1.0), "0:00");
    }
}

/// Checks that need real files on disk, not synthetic headers.
///
/// Kept apart from the unit tests because they skip rather than fail when the
/// fixture or the tool is missing: a machine without ffmpeg is a valid place to
/// build this, and a red suite there would teach people to ignore it.
#[cfg(test)]
mod integration {
    use super::*;

    #[test]
    fn a_real_jpeg_and_png_report_their_true_size() {
        let png = Path::new("/usr/share/omarchy/icon.png");
        if !png.exists() {
            eprintln!("skipping: Omarchy not installed");
            return;
        }
        let bytes = std::fs::read(png).unwrap();
        assert_eq!(
            image_dimensions(&bytes),
            Some((300, 300)),
            "ffprobe reports 300x300 for this file"
        );

        // Re-encode it as a JPEG so the segment walk meets a real chain of
        // markers rather than the two-segment fixture in the unit test.
        let dir = std::env::temp_dir().join(format!("omafiles-jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpeg = dir.join("real.jpg");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(png)
            .args(["-vf", "scale=321:247"])
            .arg(&jpeg)
            .status();
        if !matches!(made, Ok(status) if status.success()) {
            eprintln!("skipping the jpeg half: ffmpeg unavailable");
            return;
        }

        let bytes = std::fs::read(&jpeg).unwrap();
        assert_eq!(magic_image_format(&bytes), Some(ImageFormat::Jpeg));
        assert_eq!(image_dimensions(&bytes), Some((321, 247)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_video_yields_a_poster_frame_and_facts() {
        let dir = std::env::temp_dir().join(format!("omafiles-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=640x360:rate=30",
                "-t",
                "3",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status();
        if !matches!(made, Ok(status) if status.success()) {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }

        match Preview::load(&Entry::from_path(clip.clone())).body {
            Body::Video { poster, facts, .. } => {
                let poster = poster.expect("a poster frame");
                assert_eq!(poster.format, ImageFormat::Png);
                assert!(!poster.bytes.is_empty());

                let get = |k: &str| facts.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
                assert_eq!(get("resolution").as_deref(), Some("640 × 360"));
                assert!(
                    !facts.iter().any(|(n, _)| n == "size"),
                    "the caller reports the size; ffprobe repeating it was a duplicate row"
                );
                assert_eq!(get("duration").as_deref(), Some("0:03"));
                assert_eq!(get("frame rate").as_deref(), Some("30 fps"));
                assert!(get("codec").is_some());
            }
            other => panic!("expected video, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_three_second_clip_still_gets_a_poster() {
        // The seek is one second in, which is past the end of a very short
        // clip; the fallback to zero is what stops those previewing blank.
        let dir = std::env::temp_dir().join(format!("omafiles-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("blink.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x64:rate=30",
                "-t",
                "0.2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status();
        if !matches!(made, Ok(status) if status.success()) {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        assert!(
            clip.exists() && std::fs::metadata(&clip).unwrap().len() > 0,
            "the fixture must actually have been written, or this proves nothing"
        );
        assert!(video_poster(&clip).is_some(), "the zero-seek fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
