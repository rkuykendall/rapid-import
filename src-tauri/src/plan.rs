use std::path::PathBuf;

use chrono::NaiveDateTime;
use serde::Serialize;

use crate::date_resolution::DateCandidate;
use crate::profiles::ConflictPolicy;

/// Default cutoff below which a chosen date is flagged for manual review
/// rather than auto-filed. See execution-plan.md open questions — tune
/// against real test libraries.
pub const NEEDS_REVIEW_THRESHOLD: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Default, Serialize)]
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
pub fn render_template(template: &str, date: NaiveDateTime) -> String {
    date.format(template).to_string()
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
        assert_eq!(rendered, "2023/2023-08-15");
    }

    #[test]
    fn renders_month_name_template() {
        let rendered = render_template("%Y/%m - %B", dt(2023, 8, 15));
        assert_eq!(rendered, "2023/08 - August");
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
