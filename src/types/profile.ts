// Mirrors src-tauri/src/profiles.rs's Profile struct.
export interface Profile {
  id: number;
  name: string;
  folder_template: string;
  source_root: string | null;
  destination_root: string | null;
  date_fallback_order: string[];
  conflict_policy: 'skip' | 'rename';
}
