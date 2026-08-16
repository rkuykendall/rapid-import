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
/// folder, etc. — per §4 Tier 3.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub folder_template: String,
    pub filename_template: Option<String>,
    pub date_fallback_order: Vec<String>,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone)]
pub struct NewProfile {
    pub name: String,
    pub folder_template: String,
    pub filename_template: Option<String>,
    pub date_fallback_order: Vec<String>,
    pub conflict_policy: ConflictPolicy,
}

pub fn save_profile(conn: &Connection, profile: &NewProfile) -> rusqlite::Result<i64> {
    let fallback_json = serde_json::to_string(&profile.date_fallback_order)
        .expect("Vec<String> always serializes");
    conn.execute(
        "INSERT INTO profiles (name, folder_template, filename_template, date_fallback_order, conflict_policy)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            profile.name,
            profile.folder_template,
            profile.filename_template,
            fallback_json,
            profile.conflict_policy.as_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn load_profiles(conn: &Connection) -> rusqlite::Result<Vec<Profile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, folder_template, filename_template, date_fallback_order, conflict_policy
         FROM profiles ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_profile)?;
    rows.collect()
}

pub fn load_profile(conn: &Connection, id: i64) -> rusqlite::Result<Option<Profile>> {
    conn.query_row(
        "SELECT id, name, folder_template, filename_template, date_fallback_order, conflict_policy
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
    let fallback_json: String = row.get(4)?;
    let date_fallback_order: Vec<String> = serde_json::from_str(&fallback_json)
        .map_err(|e| rusqlite::Error::InvalidColumnType(4, e.to_string(), rusqlite::types::Type::Text))?;
    let conflict_policy_str: String = row.get(5)?;

    Ok(Profile {
        id: row.get(0)?,
        name: row.get(1)?,
        folder_template: row.get(2)?,
        filename_template: row.get(3)?,
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
            folder_template: "{yyyy}/{yyyy}-{mm}-{dd}".to_string(),
            filename_template: None,
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
        assert_eq!(loaded.folder_template, "{yyyy}/{yyyy}-{mm}-{dd}");
        assert_eq!(loaded.date_fallback_order, vec!["exif", "filename", "xmp", "mtime"]);
        assert_eq!(loaded.conflict_policy, ConflictPolicy::Rename);
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
