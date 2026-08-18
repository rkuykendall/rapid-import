use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::filename_patterns::match_filename;

const MIN_PLAUSIBLE_YEAR: i32 = 1990;
const CONFLICT_THRESHOLD_DAYS: i64 = 1;
const CONFLICT_DOWNGRADE_FACTOR: f32 = 0.7;

const EXIF_CONFIDENCE: f32 = 0.95;
const XMP_CONFIDENCE: f32 = 0.9;
const FS_CREATED_CONFIDENCE: f32 = 0.3;
const FS_MODIFIED_CONFIDENCE: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    Exif,
    Xmp,
    Filename,
    FsCreated,
    FsModified,
}

impl DateSource {
    /// Fixed tie-break order when two candidates land on equal confidence.
    fn priority(self) -> u8 {
        match self {
            DateSource::Exif => 0,
            DateSource::Xmp => 1,
            DateSource::Filename => 2,
            DateSource::FsCreated => 3,
            DateSource::FsModified => 4,
        }
    }

    fn is_authoritative(self) -> bool {
        matches!(self, DateSource::Exif | DateSource::Xmp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateCandidate {
    pub date: NaiveDateTime,
    pub source: DateSource,
    pub confidence: f32,
    // Owned (not `&'static str`, unlike `filename_patterns::MatchResult`)
    // so a `PlanItem` round-trips through the frontend intact — the UI
    // sends the scanned `Plan` back as-is (only `excluded` may have
    // changed) when the user commits it.
    pub pattern_name: Option<String>,
}

/// Inputs already read into memory for one file — no filesystem or Tauri
/// access happens inside this module.
#[derive(Debug, Default)]
pub struct DateInputs<'a> {
    pub filename: &'a str,
    pub exif_date_time_original: Option<NaiveDateTime>,
    pub xmp_date_time_original: Option<NaiveDateTime>,
    pub fs_created: Option<NaiveDateTime>,
    pub fs_modified: Option<NaiveDateTime>,
}

#[derive(Debug, Default)]
pub struct DateResolution {
    /// All surviving candidates, sorted best-first. The review UI shows the
    /// full list, not just the winner.
    pub candidates: Vec<DateCandidate>,
    /// True when at least two present sources disagree by more than
    /// `CONFLICT_THRESHOLD_DAYS`, regardless of whether it changed the winner.
    pub conflict: bool,
}

impl DateResolution {
    pub fn chosen(&self) -> Option<&DateCandidate> {
        self.candidates.first()
    }
}

/// Gathers a `DateCandidate` per present source, rejects implausible dates,
/// downgrades non-authoritative candidates that conflict with EXIF/XMP, then
/// ranks by confidence (fixed source priority breaks ties).
pub fn resolve(inputs: &DateInputs, now: NaiveDate) -> DateResolution {
    let mut candidates = Vec::new();

    if let Some(date) = inputs.exif_date_time_original.filter(|d| is_plausible(*d, now)) {
        candidates.push(DateCandidate {
            date,
            source: DateSource::Exif,
            confidence: EXIF_CONFIDENCE,
            pattern_name: None,
        });
    }
    if let Some(date) = inputs.xmp_date_time_original.filter(|d| is_plausible(*d, now)) {
        candidates.push(DateCandidate {
            date,
            source: DateSource::Xmp,
            confidence: XMP_CONFIDENCE,
            pattern_name: None,
        });
    }
    if let Some(m) = match_filename(inputs.filename)
        && is_plausible(m.date, now)
    {
        candidates.push(DateCandidate {
            date: m.date,
            source: DateSource::Filename,
            confidence: m.confidence,
            pattern_name: Some(m.pattern_name.to_string()),
        });
    }
    if let Some(date) = inputs.fs_created.filter(|d| is_plausible(*d, now)) {
        candidates.push(DateCandidate {
            date,
            source: DateSource::FsCreated,
            confidence: FS_CREATED_CONFIDENCE,
            pattern_name: None,
        });
    }
    if let Some(date) = inputs.fs_modified.filter(|d| is_plausible(*d, now)) {
        candidates.push(DateCandidate {
            date,
            source: DateSource::FsModified,
            confidence: FS_MODIFIED_CONFIDENCE,
            pattern_name: None,
        });
    }

    let conflict = has_conflict(&candidates);
    if conflict {
        downgrade_conflicting(&mut candidates);
    }

    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap()
            .then_with(|| a.source.priority().cmp(&b.source.priority()))
    });

    DateResolution { candidates, conflict }
}

fn is_plausible(date: NaiveDateTime, now: NaiveDate) -> bool {
    date.date().year() >= MIN_PLAUSIBLE_YEAR && date.date() <= now
}

fn conflicts(a: NaiveDateTime, b: NaiveDateTime) -> bool {
    (a - b).abs() > Duration::days(CONFLICT_THRESHOLD_DAYS)
}

fn has_conflict(candidates: &[DateCandidate]) -> bool {
    candidates.iter().enumerate().any(|(i, a)| {
        candidates[i + 1..].iter().any(|b| conflicts(a.date, b.date))
    })
}

/// Non-authoritative candidates (filename, filesystem times) that disagree
/// with a present EXIF/XMP date get their confidence pulled down rather than
/// dropped outright — they stay visible for manual override.
fn downgrade_conflicting(candidates: &mut [DateCandidate]) {
    let authoritative: Vec<NaiveDateTime> = candidates
        .iter()
        .filter(|c| c.source.is_authoritative())
        .map(|c| c.date)
        .collect();

    if authoritative.is_empty() {
        return;
    }

    for candidate in candidates.iter_mut() {
        if candidate.source.is_authoritative() {
            continue;
        }
        if authoritative.iter().any(|d| conflicts(*d, candidate.date)) {
            candidate.confidence *= CONFLICT_DOWNGRADE_FACTOR;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()
    }

    #[test]
    fn no_sources_present_yields_no_candidates() {
        let inputs = DateInputs {
            filename: "DSC00001.jpg",
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(result.candidates.is_empty());
        assert!(result.chosen().is_none());
        assert!(!result.conflict);
    }

    #[test]
    fn exif_wins_over_agreeing_filename() {
        let inputs = DateInputs {
            filename: "IMG_20230815_141523.jpg",
            exif_date_time_original: Some(dt(2023, 8, 15, 14, 15, 0)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(!result.conflict);
        assert_eq!(result.chosen().unwrap().source, DateSource::Exif);
        assert_eq!(result.candidates.len(), 2);
    }

    #[test]
    fn filename_used_as_fallback_when_no_exif() {
        let inputs = DateInputs {
            filename: "IMG_20230815_141523.jpg",
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        let chosen = result.chosen().unwrap();
        assert_eq!(chosen.source, DateSource::Filename);
        assert_eq!(chosen.confidence, 0.85);
    }

    #[test]
    fn conflicting_filename_date_is_downgraded_but_kept() {
        // Forwarded WhatsApp photo: filename date is the forward date, EXIF
        // holds the true capture date, and both stay visible for review.
        let inputs = DateInputs {
            filename: "IMG-20230901-WA0007.jpg",
            exif_date_time_original: Some(dt(2023, 8, 15, 9, 0, 0)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(result.conflict);
        assert_eq!(result.candidates.len(), 2);

        let chosen = result.chosen().unwrap();
        assert_eq!(chosen.source, DateSource::Exif);

        let filename_candidate = result
            .candidates
            .iter()
            .find(|c| c.source == DateSource::Filename)
            .unwrap();
        assert_eq!(filename_candidate.confidence, 0.8 * CONFLICT_DOWNGRADE_FACTOR);
    }

    #[test]
    fn fs_modified_is_last_resort() {
        let inputs = DateInputs {
            filename: "DSC00001.jpg",
            fs_modified: Some(dt(2022, 1, 1, 0, 0, 0)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert_eq!(result.chosen().unwrap().source, DateSource::FsModified);
    }

    #[test]
    fn implausibly_old_date_is_rejected() {
        let inputs = DateInputs {
            filename: "DSC00001.jpg",
            exif_date_time_original: Some(dt(1975, 1, 1, 0, 0, 0)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn future_date_is_rejected() {
        let inputs = DateInputs {
            filename: "DSC00001.jpg",
            exif_date_time_original: Some(dt(2030, 1, 1, 0, 0, 0)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn exif_priority_breaks_ties_over_xmp_at_equal_confidence() {
        // Synthetic tie: same instant from both sources, XMP corroborates.
        let inputs = DateInputs {
            filename: "DSC00001.jpg",
            exif_date_time_original: Some(dt(2023, 8, 15, 14, 15, 23)),
            xmp_date_time_original: Some(dt(2023, 8, 15, 14, 15, 23)),
            ..Default::default()
        };
        let result = resolve(&inputs, today());
        assert!(!result.conflict);
        assert_eq!(result.chosen().unwrap().source, DateSource::Exif);
    }
}
