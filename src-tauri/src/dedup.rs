use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use image_hasher::{HashAlg, HasherConfig, ImageHash};

use crate::db;
use crate::plan::PlanItem;

/// Default cutoff (out of 256 possible bits, see `hash_config`) below which
/// two perceptual hashes are considered a near-duplicate — re-exports,
/// edited copies, burst sequences. Starting point, not tuned against real
/// libraries yet; see execution-plan.md §11 open questions for the same
/// caveat on `NEEDS_REVIEW_THRESHOLD`.
pub const NEAR_DUPLICATE_MAX_DISTANCE: u32 = 24;

/// BLAKE3 content hash, hex-encoded — the basis for exact-duplicate
/// detection and for recognizing files already present in the SQLite
/// index (e.g. re-scanning the same SD card after a prior import).
pub fn content_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Same hasher config RapidRAW uses for its burst-culling feature
/// (`culling.rs`) — `DoubleGradient` at 16x16 rather than the crate's
/// default `Gradient`/8x8 — so hashes stay commensurable with RapidRAW's
/// own if the two tools are ever compared. RapidRAW never persists this
/// hash anywhere (not in `.rrdata`, nowhere) — it's recomputed from image
/// bytes per session there too, so there's no precomputed hash to borrow.
fn hash_config() -> HasherConfig {
    HasherConfig::new().hash_alg(HashAlg::DoubleGradient).hash_size(16, 16)
}

/// Perceptual hash for near-duplicate detection (re-exports, edited
/// copies, burst sequences). `None` for anything the `image` crate can't
/// decode (RAW, video, etc.) — those fall back to exact-hash dedup only.
pub fn perceptual_hash(path: &Path) -> Option<String> {
    let img = image::open(path).ok()?;
    let hasher = hash_config().to_hasher();
    Some(hasher.hash_image(&img).to_base64())
}

/// Hamming distance between two base64-encoded perceptual hashes. `None`
/// if either string isn't a validly-encoded hash from this same config.
pub fn hamming_distance(a: &str, b: &str) -> Option<u32> {
    let hash_a: ImageHash = ImageHash::from_base64(a).ok()?;
    let hash_b: ImageHash = ImageHash::from_base64(b).ok()?;
    Some(hash_a.dist(&hash_b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateKind {
    /// Identical content hash.
    Exact,
    /// Different bytes, perceptual hash within `NEAR_DUPLICATE_MAX_DISTANCE`
    /// — re-exports, edited copies, burst sequences.
    Near { distance: u32 },
}

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub kind: DuplicateKind,
    pub members: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
struct HashRef<'a> {
    path: &'a Path,
    content_hash: &'a str,
    perceptual_hash: Option<&'a str>,
}

fn compare(a: HashRef, b: HashRef) -> Option<DuplicateGroup> {
    if a.content_hash == b.content_hash {
        return Some(DuplicateGroup {
            kind: DuplicateKind::Exact,
            members: vec![a.path.to_path_buf(), b.path.to_path_buf()],
        });
    }
    let distance = hamming_distance(a.perceptual_hash?, b.perceptual_hash?)?;
    (distance <= NEAR_DUPLICATE_MAX_DISTANCE).then(|| DuplicateGroup {
        kind: DuplicateKind::Near { distance },
        members: vec![a.path.to_path_buf(), b.path.to_path_buf()],
    })
}

/// Cross-references every scanned file's content/perceptual hash against
/// both the other files in this same scan and the SQLite index, per §7
/// (`get_duplicates`: "against the SQLite index and against other files in
/// the current scan"). Each result is one pairing (2 members), not a fully
/// clustered group — a file that matches three others surfaces as three
/// separate pairs. Content hashes still get computed here even when a
/// `PlanItem` already carries `already_imported` — that only tells us a
/// hash exists somewhere in the index, not what it collides with, or
/// whether two files within the same scan duplicate each other.
pub fn find_duplicates(conn: &rusqlite::Connection, items: &[PlanItem]) -> Vec<DuplicateGroup> {
    struct Hashed {
        source_path: PathBuf,
        content_hash: String,
        perceptual_hash: Option<String>,
    }

    let hashed: Vec<Hashed> = items
        .iter()
        .filter_map(|item| {
            let content_hash = content_hash(&item.source_path).ok()?;
            Some(Hashed {
                perceptual_hash: perceptual_hash(&item.source_path),
                source_path: item.source_path.clone(),
                content_hash,
            })
        })
        .collect();

    let indexed = db::all_file_hashes(conn).unwrap_or_default();

    let mut groups = Vec::new();
    for i in 0..hashed.len() {
        let a = HashRef {
            path: &hashed[i].source_path,
            content_hash: &hashed[i].content_hash,
            perceptual_hash: hashed[i].perceptual_hash.as_deref(),
        };

        for hashed_b in &hashed[i + 1..] {
            let b = HashRef {
                path: &hashed_b.source_path,
                content_hash: &hashed_b.content_hash,
                perceptual_hash: hashed_b.perceptual_hash.as_deref(),
            };
            if let Some(group) = compare(a, b) {
                groups.push(group);
            }
        }

        for existing in &indexed {
            if existing.current_path == hashed[i].source_path.to_string_lossy() {
                continue;
            }
            let b = HashRef {
                path: Path::new(&existing.current_path),
                content_hash: &existing.content_hash,
                perceptual_hash: existing.perceptual_hash.as_deref(),
            };
            if let Some(group) = compare(a, b) {
                groups.push(group);
            }
        }
    }

    groups
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

    #[test]
    fn hamming_distance_of_identical_images_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        let img = image::RgbImage::from_pixel(32, 32, image::Rgb([120, 60, 200]));
        img.save(&a).unwrap();
        img.save(&b).unwrap();

        let hash_a = perceptual_hash(&a).unwrap();
        let hash_b = perceptual_hash(&b).unwrap();
        assert_eq!(hamming_distance(&hash_a, &hash_b), Some(0));
    }

    #[test]
    fn hamming_distance_of_dissimilar_images_exceeds_near_duplicate_threshold() {
        // Gradient-family hashes encode edge structure, not raw color
        // equality, so the fixtures need distinct large-scale structure
        // (a vertical split vs. a horizontal split) rather than fine detail
        // like a checkerboard, which downsamples away into noise.
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");

        let mut vertical_split = image::RgbImage::new(32, 32);
        for (x, _y, pixel) in vertical_split.enumerate_pixels_mut() {
            *pixel = if x < 16 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) };
        }
        vertical_split.save(&a).unwrap();

        let mut horizontal_split = image::RgbImage::new(32, 32);
        for (_x, y, pixel) in horizontal_split.enumerate_pixels_mut() {
            *pixel = if y < 16 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) };
        }
        horizontal_split.save(&b).unwrap();

        let hash_a = perceptual_hash(&a).unwrap();
        let hash_b = perceptual_hash(&b).unwrap();
        let distance = hamming_distance(&hash_a, &hash_b).unwrap();
        assert!(distance > NEAR_DUPLICATE_MAX_DISTANCE, "distance was {distance}");
    }

    #[test]
    fn hamming_distance_is_none_for_invalid_base64() {
        assert!(hamming_distance("not a valid hash", "also not valid").is_none());
    }

    fn plan_item(source_path: PathBuf) -> PlanItem {
        PlanItem {
            source_path,
            candidates: vec![],
            destination_path: None,
            needs_review: true,
            conflict: crate::plan::ConflictKind::None,
            no_op: false,
            already_imported: false,
            excluded: false,
        }
    }

    #[test]
    fn flags_exact_matches_within_the_same_scan() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"identical bytes").unwrap();
        fs::write(&b, b"identical bytes").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let items = vec![plan_item(a.clone()), plan_item(b.clone())];
        let groups = find_duplicates(&conn, &items);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DuplicateKind::Exact);
        assert!(groups[0].members.contains(&a) && groups[0].members.contains(&b));
    }

    #[test]
    fn flags_exact_matches_against_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let scanned = dir.path().join("scanned.jpg");
        fs::write(&scanned, b"already imported bytes").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let batch_id = crate::db::insert_batch(
            &conn,
            &crate::db::NewBatch {
                started_at: "2026-08-01T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo.json".to_string(),
            },
        )
        .unwrap();
        crate::db::insert_file(
            &conn,
            &crate::db::NewFileRecord {
                content_hash: content_hash(&scanned).unwrap(),
                perceptual_hash: None,
                current_path: "/library/2023/2023-08-15/existing.jpg".to_string(),
                capture_date: None,
                date_source: None,
                date_confidence: None,
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();

        let items = vec![plan_item(scanned.clone())];
        let groups = find_duplicates(&conn, &items);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DuplicateKind::Exact);
        assert!(groups[0].members.contains(&scanned));
        assert!(groups[0].members.contains(&PathBuf::from("/library/2023/2023-08-15/existing.jpg")));
    }

    #[test]
    fn flags_near_duplicates_within_scan_but_not_exact() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");

        let base = image::RgbImage::from_pixel(32, 32, image::Rgb([120, 60, 200]));
        base.save(&a).unwrap();
        // One pixel different: different bytes/content hash, but visually
        // indistinguishable at the hash's resolution.
        let mut nearly_identical = base.clone();
        nearly_identical.put_pixel(0, 0, image::Rgb([121, 60, 200]));
        nearly_identical.save(&b).unwrap();

        assert_ne!(content_hash(&a).unwrap(), content_hash(&b).unwrap());

        let conn = crate::db::open_in_memory().unwrap();
        let items = vec![plan_item(a.clone()), plan_item(b.clone())];
        let groups = find_duplicates(&conn, &items);

        assert_eq!(groups.len(), 1);
        assert!(matches!(groups[0].kind, DuplicateKind::Near { .. }));
    }

    #[test]
    fn no_groups_for_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"completely unrelated content one").unwrap();
        fs::write(&b, b"totally different stuff over here").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let items = vec![plan_item(a), plan_item(b)];
        let groups = find_duplicates(&conn, &items);

        assert!(groups.is_empty());
    }
}
