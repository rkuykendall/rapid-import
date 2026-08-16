use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDateTime};
use regex::Regex;

/// Locates a `.xmp` sidecar next to `source_path`, checking both naming
/// conventions seen in the wild: extension replaced (`IMG_0001.xmp` for
/// `IMG_0001.CR3`, Lightroom's convention) and extension preserved
/// (`IMG_0001.CR3.xmp`).
fn find_xmp_sidecar(source_path: &Path) -> Option<PathBuf> {
    let parent = source_path.parent()?;
    let stem = source_path.file_stem()?.to_str()?;
    let full_name = source_path.file_name()?.to_str()?;

    [parent.join(format!("{stem}.xmp")), parent.join(format!("{full_name}.xmp"))]
        .into_iter()
        .find(|p| p.is_file())
}

/// Reads `exif:DateTimeOriginal` from a source file's `.xmp` sidecar, if
/// one exists — a corroborating/fallback date source per
/// execution-plan.md §3 and §6. Read-only: RapidImport never writes XMP.
///
/// This is a pragmatic attribute/element regex match rather than full
/// RDF/XML parsing — XMP's RDF structure allows the same property to be
/// expressed several different ways, and we only need this one scalar
/// field out of it.
pub fn read_xmp_date(source_path: &Path) -> Option<NaiveDateTime> {
    let sidecar_path = find_xmp_sidecar(source_path)?;
    let contents = fs::read_to_string(sidecar_path).ok()?;
    extract_date_time_original(&contents)
}

static ATTRIBUTE_FORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"exif:DateTimeOriginal\s*=\s*"([^"]+)""#).unwrap());
static ELEMENT_FORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<exif:DateTimeOriginal>\s*([^<]+?)\s*</exif:DateTimeOriginal>").unwrap());

fn extract_date_time_original(xmp: &str) -> Option<NaiveDateTime> {
    let raw = ATTRIBUTE_FORM
        .captures(xmp)
        .or_else(|| ELEMENT_FORM.captures(xmp))?
        .get(1)?
        .as_str();
    parse_xmp_datetime(raw)
}

/// XMP dates are ISO 8601, commonly with a timezone offset or trailing
/// `Z`; some writers omit the offset entirely.
fn parse_xmp_datetime(raw: &str) -> Option<NaiveDateTime> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.naive_local());
    }
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    #[test]
    fn reads_attribute_form_with_extension_replaced_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("IMG_0001.CR3");
        fs::write(&source, b"raw bytes").unwrap();
        fs::write(
            dir.path().join("IMG_0001.xmp"),
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"
                    exif:DateTimeOriginal="2023-08-15T14:15:23"/>
            </rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();

        assert_eq!(read_xmp_date(&source), Some(dt(2023, 8, 15, 14, 15, 23)));
    }

    #[test]
    fn reads_element_form_with_extension_preserved_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("IMG_0001.CR3");
        fs::write(&source, b"raw bytes").unwrap();
        fs::write(
            dir.path().join("IMG_0001.CR3.xmp"),
            br#"<rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/">
                <exif:DateTimeOriginal>2023-08-15T14:15:23</exif:DateTimeOriginal>
            </rdf:Description>"#,
        )
        .unwrap();

        assert_eq!(read_xmp_date(&source), Some(dt(2023, 8, 15, 14, 15, 23)));
    }

    #[test]
    fn handles_timezone_offset() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("IMG_0001.CR3");
        fs::write(&source, b"raw bytes").unwrap();
        fs::write(
            dir.path().join("IMG_0001.xmp"),
            br#"<rdf:Description exif:DateTimeOriginal="2023-08-15T14:15:23-07:00" xmlns:exif="http://ns.adobe.com/exif/1.0/"/>"#,
        )
        .unwrap();

        assert_eq!(read_xmp_date(&source), Some(dt(2023, 8, 15, 14, 15, 23)));
    }

    #[test]
    fn returns_none_when_no_sidecar_exists() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("IMG_0001.CR3");
        fs::write(&source, b"raw bytes").unwrap();

        assert!(read_xmp_date(&source).is_none());
    }

    #[test]
    fn returns_none_when_sidecar_has_no_date_field() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("IMG_0001.CR3");
        fs::write(&source, b"raw bytes").unwrap();
        fs::write(dir.path().join("IMG_0001.xmp"), b"<rdf:Description/>").unwrap();

        assert!(read_xmp_date(&source).is_none());
    }
}
