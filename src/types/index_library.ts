// Mirrors src-tauri/src/index_library.rs's `IndexSummary` — kept in sync by
// hand since the core crate's Serialize output is the source of truth.

export interface IndexSummary {
  batch_id: number;
  unchanged: number;
  moved: number;
  content_changed: number;
  new: number;
  removed: number;
}
