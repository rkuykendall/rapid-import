use std::fs;
use std::io;
use std::path::Path;

/// BLAKE3 content hash, hex-encoded — the basis for exact-duplicate
/// detection and for recognizing files already present in the SQLite
/// index (e.g. re-scanning the same SD card after a prior import).
pub fn content_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Perceptual hash for near-duplicate detection (re-exports, edited
/// copies, burst sequences). `None` for anything the `image` crate can't
/// decode (RAW, video, etc.) — those fall back to exact-hash dedup only.
pub fn perceptual_hash(path: &Path) -> Option<String> {
    let img = image::open(path).ok()?;
    let hasher = image_hasher::HasherConfig::new().to_hasher();
    Some(hasher.hash_image(&img).to_base64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_produce_identical_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"same bytes").unwrap();
        fs::write(&b, b"same bytes").unwrap();

        assert_eq!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    #[test]
    fn different_bytes_produce_different_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"one").unwrap();
        fs::write(&b, b"two").unwrap();

        assert_ne!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    #[test]
    fn perceptual_hash_is_none_for_non_image_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_an_image.jpg");
        fs::write(&path, b"definitely not image bytes").unwrap();

        assert!(perceptual_hash(&path).is_none());
    }

    #[test]
    fn perceptual_hash_is_some_for_a_real_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.png");
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 40]));
        img.save(&path).unwrap();

        assert!(perceptual_hash(&path).is_some());
    }
}
