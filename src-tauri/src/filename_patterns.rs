use std::path::Path;
use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;

pub struct FilenameMatch {
    pub date: NaiveDateTime,
    pub pattern_name: &'static str,
    pub confidence: f32,
}

struct PatternDef {
    name: &'static str,
    confidence: f32,
    matcher: fn(&str) -> Option<NaiveDateTime>,
}

/// Tries the seed pattern table against a file's stem (extension stripped),
/// most-specific pattern first, returning the first match. Extensible later
/// via profile config, which would append/override entries in `PATTERNS`.
pub fn match_filename(filename: &str) -> Option<FilenameMatch> {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    PATTERNS.iter().find_map(|pattern| {
        (pattern.matcher)(stem).map(|date| FilenameMatch {
            date,
            pattern_name: pattern.name,
            confidence: pattern.confidence,
        })
    })
}

static PATTERNS: &[PatternDef] = &[
    PatternDef {
        name: "img_timestamp",
        confidence: 0.85,
        matcher: match_img_timestamp,
    },
    PatternDef {
        name: "screenshot",
        confidence: 0.85,
        matcher: match_screenshot,
    },
    PatternDef {
        name: "whatsapp",
        confidence: 0.8,
        matcher: match_whatsapp,
    },
    PatternDef {
        name: "dotted_datetime",
        confidence: 0.8,
        matcher: match_dotted_datetime,
    },
    PatternDef {
        name: "generic_timestamp",
        confidence: 0.75,
        matcher: match_generic_timestamp,
    },
    PatternDef {
        name: "compact_timestamp",
        confidence: 0.72,
        matcher: match_compact_timestamp,
    },
    PatternDef {
        name: "date_only",
        confidence: 0.6,
        matcher: match_date_only,
    },
    PatternDef {
        name: "compact_date_only",
        confidence: 0.5,
        matcher: match_compact_date_only,
    },
];

static RE_IMG_TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"IMG_(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})").unwrap());
fn match_img_timestamp(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_IMG_TIMESTAMP.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], Some((&c[4], &c[5], &c[6])))
}

static RE_SCREENSHOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Screenshot_(\d{4})-(\d{2})-(\d{2})-(\d{2})-(\d{2})-(\d{2})").unwrap()
});
fn match_screenshot(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_SCREENSHOT.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], Some((&c[4], &c[5], &c[6])))
}

static RE_WHATSAPP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"IMG-(\d{4})(\d{2})(\d{2})-WA\d+").unwrap());
fn match_whatsapp(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_WHATSAPP.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], None)
}

static RE_DOTTED_DATETIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4})-(\d{2})-(\d{2}) (\d{2})\.(\d{2})\.(\d{2})").unwrap()
});
fn match_dotted_datetime(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_DOTTED_DATETIME.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], Some((&c[4], &c[5], &c[6])))
}

static RE_GENERIC_TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})").unwrap());
fn match_generic_timestamp(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_GENERIC_TIMESTAMP.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], Some((&c[4], &c[5], &c[6])))
}

static RE_DATE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})-(\d{2})-(\d{2})").unwrap());
fn match_date_only(stem: &str) -> Option<NaiveDateTime> {
    let c = RE_DATE_ONLY.captures(stem)?;
    build_datetime(&c[1], &c[2], &c[3], None)
}

/// Matches a run of digits of exactly `len` characters — not a prefix or
/// suffix of a longer run — by scanning every maximal digit run in the
/// string (`\d+` is greedy, so `find_iter` never splits one run in two) and
/// keeping the first one of the right length. The `regex` crate has no
/// lookaround to express "not preceded/followed by a digit" directly, and a
/// fixed-width `\d{14}` alone would happily match the first 14 digits of an
/// 18-digit serial number; scanning whole runs instead makes that
/// impossible — a run either *is* exactly 14 digits or it isn't matched.
static RE_DIGIT_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
fn find_digit_run(stem: &str, len: usize) -> Option<&str> {
    RE_DIGIT_RUN.find_iter(stem).map(|m| m.as_str()).find(|run| run.len() == len)
}

/// Compact `YYYYMMDDHHMMSS` with no separators at all between date and
/// time — e.g. Android's default video filename `VID20260716183013.mp4`.
/// Distinct from `generic_timestamp`, which requires an underscore between
/// the two halves; this one has nothing to anchor on but the digit count,
/// hence the narrower `find_digit_run` helper (and the lower confidence:
/// slightly more prone to a coincidental match than a separator-delimited
/// pattern, though `build_datetime`'s calendar validation still rejects
/// anything that isn't a real date).
fn match_compact_timestamp(stem: &str) -> Option<NaiveDateTime> {
    let run = find_digit_run(stem, 14)?;
    let (year, rest) = run.split_at(4);
    let (month, rest) = rest.split_at(2);
    let (day, rest) = rest.split_at(2);
    let (hour, rest) = rest.split_at(2);
    let (minute, second) = rest.split_at(2);
    build_datetime(year, month, day, Some((hour, minute, second)))
}

/// Same idea as `match_compact_timestamp` but date-only (`YYYYMMDD`, no
/// separators, no time) — lower confidence still: an 8-digit run alone is
/// common enough (serial numbers, sequence counters) that it's a much
/// weaker signal than a 14-digit one.
fn match_compact_date_only(stem: &str) -> Option<NaiveDateTime> {
    let run = find_digit_run(stem, 8)?;
    let (year, rest) = run.split_at(4);
    let (month, day) = rest.split_at(2);
    build_datetime(year, month, day, None)
}

fn build_datetime(
    year: &str,
    month: &str,
    day: &str,
    time: Option<(&str, &str, &str)>,
) -> Option<NaiveDateTime> {
    let date = NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)?;
    let time = match time {
        Some((h, m, s)) => NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, s.parse().ok()?)?,
        None => NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    };
    Some(NaiveDateTime::new(date, time))
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

    #[test]
    fn matches_img_timestamp() {
        let m = match_filename("IMG_20230815_141523.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 14, 15, 23));
        assert_eq!(m.pattern_name, "img_timestamp");
        assert_eq!(m.confidence, 0.85);
    }

    #[test]
    fn matches_screenshot() {
        let m = match_filename("Screenshot_2023-08-15-14-15-23.png").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 14, 15, 23));
        assert_eq!(m.pattern_name, "screenshot");
    }

    #[test]
    fn matches_whatsapp() {
        let m = match_filename("IMG-20230815-WA0001.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 0, 0, 0));
        assert_eq!(m.pattern_name, "whatsapp");
        assert_eq!(m.confidence, 0.8);
    }

    #[test]
    fn matches_dotted_datetime() {
        let m = match_filename("2023-08-15 14.15.23.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 14, 15, 23));
        assert_eq!(m.pattern_name, "dotted_datetime");
    }

    #[test]
    fn matches_generic_timestamp() {
        let m = match_filename("20230815_141523.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 14, 15, 23));
        assert_eq!(m.pattern_name, "generic_timestamp");
    }

    #[test]
    fn matches_date_only() {
        let m = match_filename("2023-08-15.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 0, 0, 0));
        assert_eq!(m.pattern_name, "date_only");
        assert_eq!(m.confidence, 0.6);
    }

    #[test]
    fn matches_compact_timestamp() {
        // The reported bug: Android's default video filename, date and time
        // run together with no separator at all — generic_timestamp's `_`
        // requirement doesn't cover this.
        let m = match_filename("VID20260716183013.mp4").unwrap();
        assert_eq!(m.date, dt(2026, 7, 16, 18, 30, 13));
        assert_eq!(m.pattern_name, "compact_timestamp");
        assert_eq!(m.confidence, 0.72);
    }

    #[test]
    fn compact_timestamp_is_prefix_agnostic() {
        // No hardcoded "VID" requirement — any prefix (or none) works, same
        // as generic_timestamp already does.
        for name in ["MVI20260716183013.MOV", "MOV20260716183013.mp4", "20260716183013.mp4"] {
            let m = match_filename(name).unwrap();
            assert_eq!(m.date, dt(2026, 7, 16, 18, 30, 13), "failed for {name}");
            assert_eq!(m.pattern_name, "compact_timestamp");
        }
    }

    #[test]
    fn compact_timestamp_does_not_match_inside_a_longer_digit_run() {
        // A 16-digit serial number contains a 14-digit substring that would
        // otherwise look like a valid date+time — must not match, since a
        // fixed-width \d{14} alone (with no lookaround available) would
        // happily grab the first 14 of these.
        assert!(match_filename("SN1234567890123456.jpg").is_none());
    }

    #[test]
    fn compact_timestamp_skips_a_shorter_unrelated_digit_run_to_find_the_real_one() {
        let m = match_filename("clip_9_20260716183013_final.mp4").unwrap();
        assert_eq!(m.date, dt(2026, 7, 16, 18, 30, 13));
        assert_eq!(m.pattern_name, "compact_timestamp");
    }

    #[test]
    fn compact_timestamp_rejects_an_invalid_calendar_date() {
        assert!(match_filename("VID20261316183013.mp4").is_none(), "month 13 isn't real");
        assert!(match_filename("VID20260732183013.mp4").is_none(), "day 32 isn't real");
    }

    #[test]
    fn matches_compact_date_only() {
        let m = match_filename("VID20260716.mp4").unwrap();
        assert_eq!(m.date, dt(2026, 7, 16, 0, 0, 0));
        assert_eq!(m.pattern_name, "compact_date_only");
        assert_eq!(m.confidence, 0.5);
    }

    #[test]
    fn compact_timestamp_wins_over_compact_date_only_when_both_could_apply() {
        // compact_timestamp is tried first, so the full 14-digit reading
        // wins over treating just the leading 8 digits as a bare date.
        let m = match_filename("VID20260716183013.mp4").unwrap();
        assert_eq!(m.pattern_name, "compact_timestamp");
    }

    #[test]
    fn generic_timestamp_still_wins_over_compact_timestamp_when_separated() {
        // Sanity check that adding the no-separator variant didn't disturb
        // the existing underscore-separated pattern's priority.
        let m = match_filename("20230815_141523.jpg").unwrap();
        assert_eq!(m.pattern_name, "generic_timestamp");
    }

    #[test]
    fn prefers_more_specific_pattern_over_generic() {
        // Contains a bare YYYYMMDD_HHMMSS run but the IMG_ prefix pattern
        // should win because it's tried first.
        let m = match_filename("IMG_20230815_141523.jpg").unwrap();
        assert_eq!(m.pattern_name, "img_timestamp");
    }

    #[test]
    fn matches_pattern_embedded_in_longer_name() {
        let m = match_filename("vacation_IMG_20230815_141523_edited.jpg").unwrap();
        assert_eq!(m.date, dt(2023, 8, 15, 14, 15, 23));
    }

    #[test]
    fn no_match_returns_none() {
        assert!(match_filename("DSC00001.jpg").is_none());
        assert!(match_filename("family_photo.jpg").is_none());
    }

    #[test]
    fn rejects_invalid_calendar_dates() {
        // Month 13 can't form a valid NaiveDate, so no pattern matches.
        assert!(match_filename("IMG_20231301_141523.jpg").is_none());
    }
}
