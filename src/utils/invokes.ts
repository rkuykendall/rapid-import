// Same idiom RapidRAW uses (see its AppProperties.tsx `Invokes` enum): map
// PascalCase constants to the exact snake_case Tauri command strings, so
// call sites get autocomplete instead of stringly-typed invoke() calls.
export enum Invokes {
  ScanSource = 'scan_source',
  PreviewFolderTemplate = 'preview_folder_template',
  LoadProfileForDestination = 'load_profile_for_destination',
  SaveProfileForDestination = 'save_profile_for_destination',
  ListProfiles = 'list_profiles',
  RefreshLibraryIndex = 'refresh_library_index',
}
