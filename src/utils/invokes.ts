// Same idiom RapidRAW uses (see its AppProperties.tsx `Invokes` enum): map
// PascalCase constants to the exact snake_case Tauri command strings, so
// call sites get autocomplete instead of stringly-typed invoke() calls.
export enum Invokes {
  ScanSource = 'scan_source',
}
