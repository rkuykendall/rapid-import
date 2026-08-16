use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// What to do when a *different* file already occupies the computed
/// destination path — a real naming collision, not a duplicate. There is
/// deliberately no `Overwrite` variant: if the existing file has identical
/// content, `commit.rs` recognizes that on its own and skips without ever
/// consulting this policy (nothing would be lost); if the content differs,
/// overwriting would silently destroy data that has nothing to do with the
/// incoming file, so it's never offered as an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    Skip,
    Rename,
}

impl ConflictPolicy {
    fn as_str(self) -> &'static str {
        match self {
            ConflictPolicy::Skip => "skip",
            ConflictPolicy::Rename => "rename",
        }
    }

    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "skip" => Ok(ConflictPolicy::Skip),
            "rename" => Ok(ConflictPolicy::Rename),
            other => Err(rusqlite::Error::InvalidColumnType(
                0,
                format!("unknown conflict_policy '{other}'"),
                rusqlite::types::Type::Text,
            )),
        }
    }
}

/// One saved import configuration — SD card model, phone dump, Downloads
/// folder, etc. — per §4 Tier 3. Filenames are always preserved as-is
/// (touched only when `commit.rs` must disambiguate a genuine conflict) —
/// there is deliberately no filename template.
///
/// `source_root`/`destination_root` are optional: a profile might only
/// have settled on a destination so far (e.g. "my library lives here,
/// I'll pick whatever card is mounted each time"), or neither yet. When
/// present, a caller (future UI) can pre-fill folder pickers with these —
/// the paths that "will probably be saved between sessions."
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub folder_template: String,
    pub source_root: Option<String>,
    pub destination_root: Option<String>,
    pub date_fallback_order: Vec<String>,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone)]
pub struct NewProfile {
    pub name: String,
    pub folder_template: String,
    pub source_root: Option<String>,
    pub destination_root: Option<String>,
    pub date_fallback_order: Vec<String>,
    pub conflict_policy: ConflictPolicy,
}

pub fn save_profile(conn: &Connection, profile: &NewProfile) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO profiles (name, folder_template, source_root, destination_root, date_fallback_order, conflict_policy)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile.name,
            profile.folder_template,
            profile.source_root,
            profile.destination_root,
            fallback_json(profile),
            profile.conflict_policy.as_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Full replace of an existing profile — e.g. persisting a newly-chosen
/// destination folder back onto the profile the user just used.
pub fn update_profile(conn: &Connection, id: i64, profile: &NewProfile) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE profiles
         SET name = ?1, folder_template = ?2, source_root = ?3, destination_root = ?4,
             date_fallback_order = ?5, conflict_policy = ?6
         WHERE id = ?7",
        params![
            profile.name,
            profile.folder_template,
            profile.source_root,
            profile.destination_root,
            fallback_json(profile),
            profile.conflict_policy.as_str(),
            id,
        ],
    )?;
    Ok(())
}

fn fallback_json(profile: &NewProfile) -> String {
    serde_json::to_string(&profile.date_fallback_order).expect("Vec<String> always serializes")
}

pub fn load_profiles(conn: &Connection) -> rusqlite::Result<Vec<Profile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, folder_template, source_root, destination_root, date_fallback_order, conflict_policy
         FROM profiles ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_profile)?;
    rows.collect()
}

pub fn load_profile(conn: &Connection, id: i64) -> rusqlite::Result<Option<Profile>> {
    conn.query_row(
        "SELECT id, name, folder_template, source_root, destination_root, date_fallback_order, conflict_policy
         FROM profiles WHERE id = ?1",
        params![id],
        row_to_profile,
    )
    .optional()
}

pub fn delete_profile(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM profiles WHERE id = ?1", params![id])?;
    Ok(())
}

fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<Profile> {
    let fallback_json: String = row.get(5)?;
    let date_fallback_order: Vec<String> = serde_json::from_str(&fallback_json)
        .map_err(|e| rusqlite::Error::InvalidColumnType(5, e.to_string(), rusqlite::types::Type::Text))?;
    let conflict_policy_str: String = row.get(6)?;

    Ok(Profile {
        id: row.get(0)?,
        name: row.get(1)?,
        folder_template: row.get(2)?,
        source_root: row.get(3)?,
        destination_root: row.get(4)?,
        date_fallback_order,
        conflict_policy: ConflictPolicy::parse(&conflict_policy_str)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn sample() -> NewProfile {
        NewProfile {
            name: "SD Card".to_string(),
            folder_template: "%Y/%Y-%m-%d".to_string(),
            source_root: Some("/Volumes/SDCARD/DCIM".to_string()),
            destination_root: Some("/Users/me/Photos".to_string()),
            date_fallback_order: vec![
                "exif".to_string(),
                "filename".to_string(),
                "xmp".to_string(),
                "mtime".to_string(),
            ],
            conflict_policy: ConflictPolicy::Rename,
        }
    }

    #[test]
    fn save_and_load_round_trips() {
        let conn = db::open_in_memory().unwrap();
        let id = save_profile(&conn, &sample()).unwrap();

        let loaded = load_profile(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.name, "SD Card");
        assert_eq!(loaded.folder_template, "%Y/%Y-%m-%d");
        assert_eq!(loaded.source_root.as_deref(), Some("/Volumes/SDCARD/DCIM"));
        assert_eq!(loaded.destination_root.as_deref(), Some("/Users/me/Photos"));
        assert_eq!(loaded.date_fallback_order, vec!["exif", "filename", "xmp", "mtime"]);
        assert_eq!(loaded.conflict_policy, ConflictPolicy::Rename);
    }

    #[test]
    fn paths_are_optional() {
        let conn = db::open_in_memory().unwrap();
        let mut profile = sample();
        profile.source_root = None;
        profile.destination_root = None;

        let id = save_profile(&conn, &profile).unwrap();
        let loaded = load_profile(&conn, id).unwrap().unwrap();
        assert!(loaded.source_root.is_none());
        assert!(loaded.destination_root.is_none());
    }

    #[test]
    fn update_profile_persists_a_newly_chosen_destination() {
        let conn = db::open_in_memory().unwrap();
        let id = save_profile(&conn, &sample()).unwrap();

        let mut updated = sample();
        updated.destination_root = Some("/Volumes/External/Photos".to_string());
        update_profile(&conn, id, &updated).unwrap();

        let loaded = load_profile(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.destination_root.as_deref(), Some("/Volumes/External/Photos"));
        // Untouched fields survive the full replace unchanged.
        assert_eq!(loaded.source_root.as_deref(), Some("/Volumes/SDCARD/DCIM"));
        assert_eq!(loaded.name, "SD Card");
    }

    #[test]
    fn load_profiles_returns_all_in_id_order() {
        let conn = db::open_in_memory().unwrap();
        let mut first = sample();
        first.name = "First".to_string();
        let mut second = sample();
        second.name = "Second".to_string();

        save_profile(&conn, &first).unwrap();
        save_profile(&conn, &second).unwrap();

        let profiles = load_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "First");
        assert_eq!(profiles[1].name, "Second");
    }

    #[test]
    fn load_profile_returns_none_when_missing() {
        let conn = db::open_in_memory().unwrap();
        assert!(load_profile(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn delete_profile_removes_it() {
        let conn = db::open_in_memory().unwrap();
        let id = save_profile(&conn, &sample()).unwrap();
        delete_profile(&conn, id).unwrap();
        assert!(load_profile(&conn, id).unwrap().is_none());
    }
}
