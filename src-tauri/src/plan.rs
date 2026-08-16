use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::date_resolution::DateCandidate;

/// Default cutoff below which a chosen date is flagged for manual review
/// rather than auto-filed. See execution-plan.md open questions — tune
/// against real test libraries.
pub const NEEDS_REVIEW_THRESHOLD: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    None,
    /// A file already exists at the computed destination path.
    DestinationExists,
    /// Another item in this same plan resolved to the same destination.
    DuplicateInPlan,
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub source_path: PathBuf,
    /// Full candidate list, best-first, so the review UI can show alternates.
    pub candidates: Vec<DateCandidate>,
    pub destination_path: Option<PathBuf>,
    pub needs_review: bool,
    pub conflict: ConflictKind,
}

impl PlanItem {
    pub fn chosen(&self) -> Option<&DateCandidate> {
        self.candidates.first()
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<PlanItem>,
}

/// Renders folder/filename templates against a resolved date. Recognized
/// tokens: `{yyyy}`, `{mm}`, `{dd}`, `{month_name}` — each just delegates to
/// chrono's own strftime directives, same idiom RapidRAW uses for its
/// `{YYYY}`/`{MM}`/`{DD}` export-filename tokens in `file_management.rs`.
pub fn render_template(template: &str, date: NaiveDateTime) -> String {
    template
        .replace("{yyyy}", &date.format("%Y").to_string())
        .replace("{mm}", &date.format("%m").to_string())
        .replace("{dd}", &date.format("%d").to_string())
        .replace("{month_name}", &date.format("%B").to_string())
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
        let rendered = render_template("{yyyy}/{yyyy}-{mm}-{dd}", dt(2023, 8, 15));
        assert_eq!(rendered, "2023/2023-08-15");
    }

    #[test]
    fn renders_month_name_template() {
        let rendered = render_template("{yyyy}/{mm} - {month_name}", dt(2023, 8, 15));
        assert_eq!(rendered, "2023/08 - August");
    }
}
