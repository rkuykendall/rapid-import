// Mirrors src-tauri/src/date_resolution.rs and plan.rs — kept in sync by
// hand since the core crate's Serialize output is the source of truth.

export type DateSource = 'exif' | 'xmp' | 'filename' | 'fs_created' | 'fs_modified';

export interface DateCandidate {
  date: string; // ISO-ish NaiveDateTime string, e.g. "2023-08-15T14:15:23"
  source: DateSource;
  confidence: number;
  pattern_name: string | null;
}

export type ConflictKind = 'none' | 'destination_exists' | 'duplicate_at_destination' | 'duplicate_in_plan';

export interface PlanItem {
  source_path: string;
  candidates: DateCandidate[];
  destination_path: string | null;
  needs_review: boolean;
  conflict: ConflictKind;
  no_op: boolean;
  already_imported: boolean;
  excluded: boolean;
}

export interface Plan {
  items: PlanItem[];
}
