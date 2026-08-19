use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::date_resolution::DateCandidate;
use crate::profiles::ConflictPolicy;

/// Default cutoff below which a chosen date is flagged for manual review
/// rather than auto-filed. See execution-plan.md open questions — tune
/// against real test libraries.
pub const NEEDS_REVIEW_THRESHOLD: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    None,
    /// A *different* file already exists at the computed destination path —
    /// a real naming collision. Never resolved by overwriting; only
    /// `Skip`/`Rename` apply.
    DestinationExists,
    /// The file already at the computed destination path has identical
    /// content — nothing to import, safe to skip without asking.
    DuplicateAtDestination,
    /// Another item in this same plan resolved to the same destination.
    DuplicateInPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub source_path: PathBuf,
    /// Full candidate list, best-first, so the review UI can show alternates.
    pub candidates: Vec<DateCandidate>,
    pub destination_path: Option<PathBuf>,
    pub needs_review: bool,
    pub conflict: ConflictKind,
    /// True when source and destination already resolve to the same file on
    /// disk — reorganize-in-place found nothing to do for this item.
    pub no_op: bool,
    /// True when this file's content hash already exists in the SQLite
    /// index, e.g. re-scanning the same SD card after a prior import.
    pub already_imported: bool,
    /// Caller-set exclusion — "don't import this file, for any reason" —
    /// independent of conflict status. `scan()` always produces `false`;
    /// only ever set by a caller mutating `Plan.items` between `scan()`
    /// and `commit_plan()`.
    pub excluded: bool,
    /// This file's BLAKE3 content hash, computed once by
    /// `scan::build_plan_item` (and reused by `dedup::find_duplicates`
    /// rather than being read/hashed a second time). `None` only when
    /// `ScanOptions.index` was `None` — with nothing indexed, nothing needs
    /// hashing at scan time.
    pub content_hash: Option<String>,
    /// True when this item's own extension is a sidecar format (`.xmp`,
    /// `.rrdata`, `.rrexif`). The item still moves normally at commit time
    /// (see `commit_group`) — this only tells the UI to fold it into its
    /// primary's row instead of showing it as its own.
    pub is_sidecar: bool,
    /// Lowercased, no-dot extensions of this item's own sidecar siblings —
    /// e.g. `["xmp", "rrdata"]` — populated by `scan::align_sidecars_with_primary`
    /// only on the primary of a group that has sidecar siblings, so the Plan
    /// table can render one "Sidecars" column instead of a separate row per
    /// sidecar file. Always empty on an item where `is_sidecar` is true.
    pub sidecar_extensions: Vec<String>,
}

impl PlanItem {
    pub fn chosen(&self) -> Option<&DateCandidate> {
        self.candidates.first()
    }

    /// Best-effort prediction of whether this item will actually move
    /// during `commit_plan` — for sorting/filtering in a future review UI
    /// ("is moving" vs. settled). `commit_plan` itself never consults this;
    /// it always re-derives the truth fresh at commit time (state can
    /// change between scan and commit), so this can't cause a correctness
    /// bug if it's wrong — only a slightly stale-looking preview.
    ///
    /// Exact for every case except `ConflictKind::DuplicateInPlan`: when
    /// two items collide on the same computed destination, whichever one
    /// `commit_plan` processes first actually succeeds — an order this
    /// method can't know — so both are approximated the same way as a
    /// `DestinationExists` conflict.
    pub fn will_move(&self, conflict_policy: ConflictPolicy) -> bool {
        if self.destination_path.is_none() || self.no_op || self.already_imported || self.excluded {
            return false;
        }
        match self.conflict {
            ConflictKind::DuplicateAtDestination => false,
            ConflictKind::None => true,
            ConflictKind::DestinationExists | ConflictKind::DuplicateInPlan => {
                conflict_policy == ConflictPolicy::Rename
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub items: Vec<PlanItem>,
}

/// Renders a folder template against a resolved date using chrono's own
/// strftime syntax directly (`%Y`, `%m`, `%d`, `%B`, ...) — see
/// <https://docs.rs/chrono/latest/chrono/format/strftime/> for the full
/// specifier reference. Deliberately not a custom token vocabulary: an
/// earlier version hand-rolled `{yyyy}`/`{mm}`/`{dd}`/`{mmmm}`-style tokens,
/// which meant maintaining our own (incomplete, undocumented) subset of
/// what chrono already provides — and a specifier we hadn't implemented
/// would silently show up as literal text in the folder name instead of
/// erroring. Delegating to chrono directly gets the full spec for free and
/// gives users a real, authoritative doc page to check against.
///
/// Users write `/` as the folder separator regardless of platform (every
/// example in the docs, and every template we ship, does) — chrono has no
/// idea that's meaningful and emits it as a literal character, so on
/// Windows the rendered string would otherwise mix `/` from the template
/// with `\` from every later `PathBuf::join`. That mismatch is harmless for
/// `Path`/`PathBuf` equality (component-wise, separator-insensitive) but
/// corrupts anything that compares or stores the *string* form — e.g. the
/// `current_path` index lookups `scan.rs` relies on to skip re-hashing an
/// unchanged file, which silently stop matching once a real (all-`\`)
/// filesystem walk is compared against an indexed row saved with a
/// (mixed-`/`-and-`\`) templated path. Normalizing here, once, means every
/// downstream `PathBuf::join`/string conversion is separator-consistent
/// from the start; a no-op on platforms where `/` already is native.
pub fn render_template(template: &str, date: NaiveDateTime) -> String {
    let rendered = date.format(template).to_string();
    if std::path::MAIN_SEPARATOR == '/' {
        rendered
    } else {
        rendered.replace('/', std::path::MAIN_SEPARATOR_STR)
    }
}

/// Groups item indices sharing a source parent directory + associated-file
/// key (`crate::formats::associated_file_key`) — e.g. `IMG_0001.CR3` /
/// `IMG_0001.JPG` / `IMG_0001.xmp` / `IMG_0001.CR3.rrdata`. Each group is
/// returned primary-first: the member with the highest-confidence resolved
/// date, which is also the member whose destination folder the whole group
/// (siblings included) actually lands in at commit time — see
/// `commit.rs`'s `commit_group`. Items with no resolvable date at all (no
/// destination) are dropped entirely; there's nothing to group them by, and
/// they can never be anyone's primary or sibling.
///
/// Shared by `commit.rs` (deciding where a group's files actually move) and
/// `scan.rs` (reconciling a sidecar's *previewed* destination/review status
/// with the primary it will actually follow) so the two can never disagree
/// about which files belong to which group.
pub fn group_associated_indices(items: &[PlanItem]) -> Vec<Vec<usize>> {
    let mut by_key: HashMap<(PathBuf, String), Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        if item.destination_path.is_none() {
            continue;
        }
        let parent = item.source_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let key = crate::formats::associated_file_key(&item.source_path);
        by_key.entry((parent, key)).or_default().push(index);
    }

    by_key
        .into_values()
        .map(|mut indices| {
            indices.sort_by(|&a, &b| {
                let confidence_of = |i: usize| items[i].chosen().map(|c| c.confidence).unwrap_or(0.0);
                confidence_of(b)
                    .partial_cmp(&confidence_of(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            indices
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(0, 0, 0).unwrap()
    }

    #[test]
    fn renders_yyyy_mm_dd_folder_template() {
        let rendered = render_template("%Y/%Y-%m-%d", dt(2023, 8, 15));
        assert_eq!(rendered, format!("2023{sep}2023-08-15", sep = std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn renders_month_name_template() {
        let rendered = render_template("%Y/%m - %B", dt(2023, 8, 15));
        assert_eq!(rendered, format!("2023{sep}08 - August", sep = std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn a_template_separator_that_is_already_native_is_unaffected() {
        // On Unix this is the same case as the two tests above (`/` already
        // is native); on Windows it proves a template segment that doesn't
        // itself contain a separator is left alone.
        let rendered = render_template("%Y", dt(2023, 8, 15));
        assert_eq!(rendered, "2023");
    }

    fn item(destination_path: Option<PathBuf>) -> PlanItem {
        PlanItem {
            source_path: PathBuf::from("/source/IMG_0001.jpg"),
            candidates: vec![],
            destination_path,
            needs_review: false,
            conflict: ConflictKind::None,
            no_op: false,
            already_imported: false,
            excluded: false,
            content_hash: None,
            is_sidecar: false,
            sidecar_extensions: vec![],
        }
    }

    #[test]
    fn will_move_is_false_without_a_destination() {
        assert!(!item(None).will_move(ConflictPolicy::Rename));
    }

    #[test]
    fn will_move_is_true_for_a_clean_plain_move() {
        assert!(item(Some(PathBuf::from("/dest/IMG_0001.jpg"))).will_move(ConflictPolicy::Skip));
    }

    #[test]
    fn will_move_is_false_when_no_op_already_imported_or_excluded() {
        let dest = Some(PathBuf::from("/dest/IMG_0001.jpg"));

        let mut a = item(dest.clone());
        a.no_op = true;
        assert!(!a.will_move(ConflictPolicy::Rename));

        let mut b = item(dest.clone());
        b.already_imported = true;
        assert!(!b.will_move(ConflictPolicy::Rename));

        let mut c = item(dest);
        c.excluded = true;
        assert!(!c.will_move(ConflictPolicy::Rename));
    }

    #[test]
    fn will_move_is_false_for_duplicate_at_destination_regardless_of_policy() {
        let mut a = item(Some(PathBuf::from("/dest/IMG_0001.jpg")));
        a.conflict = ConflictKind::DuplicateAtDestination;
        assert!(!a.will_move(ConflictPolicy::Rename));
        assert!(!a.will_move(ConflictPolicy::Skip));
    }

    #[test]
    fn will_move_for_a_real_conflict_follows_the_batch_policy() {
        let mut a = item(Some(PathBuf::from("/dest/IMG_0001.jpg")));
        a.conflict = ConflictKind::DestinationExists;
        assert!(a.will_move(ConflictPolicy::Rename));
        assert!(!a.will_move(ConflictPolicy::Skip));
    }

    #[test]
    fn will_move_for_duplicate_in_plan_approximates_as_a_conflict() {
        let mut a = item(Some(PathBuf::from("/dest/IMG_0001.jpg")));
        a.conflict = ConflictKind::DuplicateInPlan;
        assert!(a.will_move(ConflictPolicy::Rename));
        assert!(!a.will_move(ConflictPolicy::Skip));
    }
}
