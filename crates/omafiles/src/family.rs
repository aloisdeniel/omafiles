//! What an entry *is*, for the icon that stands for it and the hue it wears.
//!
//! A listing where every file is the same grey glyph tells you only that
//! things are files. The theme carries six hues beside its roles, and a
//! file manager has the vocabulary to spend them on: directories, pictures,
//! code, archives. The mapping lives here, once, so the listing, the
//! sidebar and the detail sheet all say the same thing about the same file.
//!
//! By extension, like [`crate::preview`]'s classifier, and sharing its
//! tables: what previews as an image is drawn as one.

use std::path::Path;

use omarchy_ui::Hue;

use crate::entry::{Entry, Kind};
use crate::preview;

/// The families an entry sorts into. Each has a glyph; most have a hue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Directory,
    /// A symlink that resolves to nothing.
    Broken,
    Image,
    Video,
    Audio,
    Archive,
    /// Source with a grammar the preview knows.
    Code,
    /// Text with no grammar: notes, logs, configuration.
    Text,
    Pdf,
    Document,
    Spreadsheet,
    Slides,
    /// A file with an extension nothing here has a rule for, or none.
    Other,
}

const AUDIO: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "wav", "aac", "m4a", "wma", "aiff", "alac",
];

const ARCHIVES: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "tbz", "xz", "txz", "zst", "7z", "rar", "deb", "rpm", "iso",
    "jar", "apk", "dmg",
];

const DOCUMENTS: &[&str] = &["doc", "docx", "odt", "rtf", "epub", "pages"];
const SPREADSHEETS: &[&str] = &["xls", "xlsx", "ods", "numbers"];
const SLIDES: &[&str] = &["ppt", "pptx", "odp", "key"];

impl Family {
    pub fn of(entry: &Entry) -> Self {
        match entry.kind {
            Kind::Directory => Self::Directory,
            Kind::Unresolved => Self::Broken,
            Kind::File => Self::of_file(&entry.path),
        }
    }

    /// A file's family, from its name alone.
    pub fn of_file(path: &Path) -> Self {
        let Some(extension) = preview::extension_of(path) else {
            return Self::Other;
        };
        let extension = extension.as_str();
        if preview::IMAGES.contains(&extension) {
            Self::Image
        } else if preview::VIDEOS.contains(&extension) {
            Self::Video
        } else if AUDIO.contains(&extension) {
            Self::Audio
        } else if ARCHIVES.contains(&extension) {
            Self::Archive
        } else if extension == "pdf" {
            Self::Pdf
        } else if DOCUMENTS.contains(&extension) {
            Self::Document
        } else if SPREADSHEETS.contains(&extension) {
            Self::Spreadsheet
        } else if SLIDES.contains(&extension) {
            Self::Slides
        } else if preview::LANGUAGES.iter().any(|(e, _)| *e == extension) {
            Self::Code
        } else if preview::PLAIN_TEXT.contains(&extension) {
            Self::Text
        } else {
            Self::Other
        }
    }

    /// The Nerd Font glyph that stands for this family.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Directory => "\u{f07b}",   // nf-fa-folder
            Self::Broken => "\u{f127}",      // nf-fa-chain_broken
            Self::Image => "\u{f1c5}",       // nf-fa-file_image_o
            Self::Video => "\u{f1c8}",       // nf-fa-file_video_o
            Self::Audio => "\u{f1c7}",       // nf-fa-file_audio_o
            Self::Archive => "\u{f1c6}",     // nf-fa-file_archive_o
            Self::Code => "\u{f1c9}",        // nf-fa-file_code_o
            Self::Text => "\u{f15c}",        // nf-fa-file_text
            Self::Pdf => "\u{f1c1}",         // nf-fa-file_pdf_o
            Self::Document => "\u{f1c2}",    // nf-fa-file_word_o
            Self::Spreadsheet => "\u{f1c3}", // nf-fa-file_excel_o
            Self::Slides => "\u{f1c4}",      // nf-fa-file_powerpoint_o
            Self::Other => "\u{f15b}",       // nf-fa-file
        }
    }

    /// The hue this family wears, or none for the families that are just
    /// files: plain text and the unknown stay in the secondary colour, so
    /// the hues mark what is *distinctive* rather than painting every row.
    ///
    /// One hue per idea, not per family: pictures and video share magenta,
    /// the office trio shares orange, so the listing reads as a handful of
    /// colours with meaning rather than a dozen without.
    pub fn hue(self) -> Option<Hue> {
        match self {
            Self::Directory => Some(Hue::Blue),
            Self::Broken => Some(Hue::Red),
            Self::Image | Self::Video => Some(Hue::Magenta),
            Self::Audio => Some(Hue::Cyan),
            Self::Archive => Some(Hue::Yellow),
            Self::Code => Some(Hue::Green),
            Self::Pdf | Self::Document | Self::Spreadsheet | Self::Slides => Some(Hue::Orange),
            Self::Text | Self::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(name: &str) -> Family {
        Family::of_file(&PathBuf::from(name))
    }

    #[test]
    fn sorts_by_extension_case_insensitively() {
        assert_eq!(file("photo.JPG"), Family::Image);
        assert_eq!(file("clip.mkv"), Family::Video);
        assert_eq!(file("song.flac"), Family::Audio);
        assert_eq!(file("site.tar.gz"), Family::Archive);
        assert_eq!(file("main.rs"), Family::Code);
        assert_eq!(file("notes.txt"), Family::Text);
        assert_eq!(file("paper.pdf"), Family::Pdf);
        assert_eq!(file("letter.docx"), Family::Document);
        assert_eq!(file("books.xlsx"), Family::Spreadsheet);
        assert_eq!(file("talk.pptx"), Family::Slides);
        assert_eq!(file("mystery.xyz"), Family::Other);
        assert_eq!(file("README"), Family::Other);
    }

    #[test]
    fn a_dotfile_is_read_by_its_name() {
        // `.gitignore` has no stem; its name is its extension, as the
        // preview reads it.
        assert_eq!(file(".gitignore"), Family::Text);
    }

    #[test]
    fn the_entry_kind_wins_over_the_name() {
        let mut entry = Entry {
            path: PathBuf::from("/tmp/archive.zip"),
            name: "archive.zip".into(),
            kind: Kind::Directory,
            size: None,
            modified: None,
            is_symlink: false,
        };
        assert_eq!(Family::of(&entry), Family::Directory);
        entry.kind = Kind::Unresolved;
        assert_eq!(Family::of(&entry), Family::Broken);
        entry.kind = Kind::File;
        assert_eq!(Family::of(&entry), Family::Archive);
    }

    #[test]
    fn plain_files_wear_no_hue_and_the_rest_do() {
        assert_eq!(Family::Text.hue(), None);
        assert_eq!(Family::Other.hue(), None);
        for family in [
            Family::Directory,
            Family::Broken,
            Family::Image,
            Family::Archive,
            Family::Code,
            Family::Pdf,
        ] {
            assert!(family.hue().is_some(), "{family:?}");
        }
    }
}
