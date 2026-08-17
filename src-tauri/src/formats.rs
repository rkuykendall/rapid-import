//! Extension whitelist for what counts as an importable media file. Same
//! approach RapidRAW uses (`formats.rs`'s `is_supported_image_file`) rather
//! than trying to blacklist junk — a blacklist can't anticipate every OS
//! metadata file (`.DS_Store`, `Thumbs.db`, `.store.db`, ...) a source
//! folder might contain, but a whitelist only ever admits what we actually
//! know how to import.

use std::path::Path;

/// RAW formats, from RapidRAW's `RAW_EXTENSIONS` (`src-tauri/src/formats.rs`).
const RAW_EXTENSIONS: &[&str] = &[
    "dng", "pro", "ari", "crw", "cr2", "cr3", "bay", "raw", "erf", "raf", "3fr", "fff", "iiq",
    "kdc", "k25", "dcs", "dcr", "mos", "rwl", "mef", "mrw", "nef", "nrw", "orf", "rw2", "pef",
    "ptx", "srw", "x3f", "arw", "srf", "sr2",
];

/// Non-RAW image formats, from RapidRAW's `NON_RAW_EXTENSIONS`.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "exr", "qoi"];

/// RapidRAW doesn't handle video at all, so this list has no upstream
/// counterpart — it's just the common camera/phone container formats.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mts", "m2ts", "m4v", "3gp", "mkv", "webm", "wmv",
];

/// Sidecar formats that must stay scannable as their own `PlanItem` so
/// `commit.rs`'s associated-file grouping can find and relocate them
/// alongside their primary file.
const SIDECAR_EXTENSIONS: &[&str] = &["xmp"];

/// Whether `path`'s extension matches a known importable format. Purely
/// extension-based, case-insensitive — matches RapidRAW's approach. A file
/// with no extension (including dotfiles like `.DS_Store`, where the whole
/// name is the stem) never matches.
pub fn is_supported_media_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };

    RAW_EXTENSIONS
        .iter()
        .chain(IMAGE_EXTENSIONS)
        .chain(VIDEO_EXTENSIONS)
        .chain(SIDECAR_EXTENSIONS)
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_photo_video_and_sidecar_extensions() {
        for name in ["IMG_0001.JPG", "DSC00001.cr2", "clip.MP4", "IMG_0001.xmp"] {
            assert!(is_supported_media_file(Path::new(name)), "{name} should be supported");
        }
    }

    #[test]
    fn rejects_os_metadata_and_extensionless_files() {
        for name in [".DS_Store", "Thumbs.db", ".store.db", ".localized", "README"] {
            assert!(!is_supported_media_file(Path::new(name)), "{name} should not be supported");
        }
    }
}
