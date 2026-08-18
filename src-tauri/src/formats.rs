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
/// alongside their primary file. `rrdata`/`rrexif` are RapidRAW's own
/// edit-history and cached-EXIF sidecars (`file_management.rs` in that
/// repo) — always extension-*preserved* (`IMG_0001.CR3.rrdata`, never
/// `IMG_0001.rrdata`), unlike `.xmp` which shows up in the wild both ways.
const SIDECAR_EXTENSIONS: &[&str] = &["xmp", "rrdata", "rrexif"];

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

/// Whether `path`'s own extension is a sidecar format (`.xmp`, `.rrdata`,
/// `.rrexif`) rather than an actual photo/video. Used to tell `scan.rs`
/// which items in an associated-file group are true sidecars — folded into
/// their primary's `sidecar_extensions` for display — versus a real paired
/// file like a RAW+JPEG sibling, which still gets its own row.
pub fn is_sidecar_file(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| SIDECAR_EXTENSIONS.iter().any(|c| ext.eq_ignore_ascii_case(c)))
}

/// Recovers the base name a primary media file shares with its sidecars,
/// regardless of which sidecar convention is in play — extension-replaced
/// (`IMG_0001.xmp` for `IMG_0001.CR3`) or extension-preserved
/// (`IMG_0001.CR3.xmp`, and `.rrdata`/`.rrexif`, which are *always*
/// extension-preserved). `Path::file_stem` alone only strips the last
/// extension, so on `IMG_0001.CR3.rrdata` it yields `"IMG_0001.CR3"` — not
/// `"IMG_0001"`, and not equal to the primary's own stem. `commit.rs`'s
/// associated-file grouping keys on this instead, so both conventions land
/// on the same key. Case-folded so grouping is also case-insensitive.
pub fn associated_file_key(path: &Path) -> String {
    let mut name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string();

    // Peel off any stacked sidecar extensions first.
    while let Some(ext) = Path::new(&name).extension().and_then(|s| s.to_str()) {
        if SIDECAR_EXTENSIONS.iter().any(|c| ext.eq_ignore_ascii_case(c)) {
            name = Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
        } else {
            break;
        }
    }

    // Then strip exactly one real media extension, if what's left has one,
    // to reach the true base name shared with any sidecars.
    if let Some(ext) = Path::new(&name).extension().and_then(|s| s.to_str())
        && RAW_EXTENSIONS
            .iter()
            .chain(IMAGE_EXTENSIONS)
            .chain(VIDEO_EXTENSIONS)
            .any(|c| ext.eq_ignore_ascii_case(c))
    {
        name = Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
    }

    name.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_photo_video_and_sidecar_extensions() {
        for name in [
            "IMG_0001.JPG",
            "DSC00001.cr2",
            "clip.MP4",
            "IMG_0001.xmp",
            "IMG_0001.CR3.rrdata",
            "IMG_0001.CR3.rrexif",
        ] {
            assert!(is_supported_media_file(Path::new(name)), "{name} should be supported");
        }
    }

    #[test]
    fn rejects_os_metadata_and_extensionless_files() {
        for name in [".DS_Store", "Thumbs.db", ".store.db", ".localized", "README"] {
            assert!(!is_supported_media_file(Path::new(name)), "{name} should not be supported");
        }
    }

    #[test]
    fn associated_file_key_matches_extension_replaced_sidecar() {
        assert_eq!(
            associated_file_key(Path::new("IMG_0001.CR3")),
            associated_file_key(Path::new("IMG_0001.xmp")),
        );
    }

    #[test]
    fn associated_file_key_matches_extension_preserved_sidecars() {
        let primary = associated_file_key(Path::new("IMG_0001.CR3"));
        for sidecar in ["IMG_0001.CR3.xmp", "IMG_0001.CR3.rrdata", "IMG_0001.CR3.rrexif"] {
            assert_eq!(
                associated_file_key(Path::new(sidecar)),
                primary,
                "{sidecar} should key the same as its primary"
            );
        }
    }

    #[test]
    fn associated_file_key_does_not_merge_unrelated_dotted_filenames() {
        // A literal dot in a real capture/export filename shouldn't cause
        // unrelated files to collide on the same grouping key.
        assert_ne!(
            associated_file_key(Path::new("Trip.001.CR2")),
            associated_file_key(Path::new("Trip.002.CR2")),
        );
    }
}
