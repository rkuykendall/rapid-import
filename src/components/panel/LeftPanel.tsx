import { open } from '@tauri-apps/plugin-shell';
import { ExternalLink, Import, Loader2, Search } from 'lucide-react';
import Button from '../ui/Button';
import Input from '../ui/Input';
import Text from '../ui/Text';
import Switch from '../ui/Switch';
import FolderPicker from '../FolderPicker';
import SourcesPanel from './SourcesPanel';
import { TextVariants } from '../../types/typography';
import { Profile } from '../../types/profile';

const CHRONO_STRFTIME_DOCS_URL = 'https://docs.rs/chrono/latest/chrono/format/strftime/index.html';

interface LeftPanelProps {
  profiles: Profile[];
  activeDestinationRoot: string;
  onSelectDestination(destinationRoot: string): void;
  hasDestination: boolean;
  sourceRoot: string;
  onSourceRootChange(value: string): void;
  reorganizeInPlace: boolean;
  onReorganizeInPlaceChange(value: boolean): void;
  effectiveSourceRoot: string;
  folderTemplate: string;
  onFolderTemplateChange(value: string): void;
  templatePreview: string;
  canScan: boolean;
  isPlanCurrent: boolean;
  loading: boolean;
  scannedCount: number;
  onScan(): void;
  error: string | null;
}

// Owns the whole left column: the Destinations section (via `SourcesPanel`)
// plus, once a destination is picked, the Source and Folder template
// sections and a single pinned action button — "Scan" before a plan exists
// for the current inputs, "Import" once one does (still a disabled stub;
// wiring real commit execution is its own deferred task).
export default function LeftPanel({
  profiles,
  activeDestinationRoot,
  onSelectDestination,
  hasDestination,
  sourceRoot,
  onSourceRootChange,
  reorganizeInPlace,
  onReorganizeInPlaceChange,
  effectiveSourceRoot,
  folderTemplate,
  onFolderTemplateChange,
  templatePreview,
  canScan,
  isPlanCurrent,
  loading,
  scannedCount,
  onScan,
  error,
}: LeftPanelProps) {
  return (
    <div className="flex flex-col h-full">
      <SourcesPanel profiles={profiles} activeDestinationRoot={activeDestinationRoot} onSelect={onSelectDestination} />

      {hasDestination && (
        <>
          <div className="border-t border-border-color p-3 flex-shrink-0 flex flex-col gap-1">
            <Text variant={TextVariants.small} className="uppercase tracking-wide">
              Source
            </Text>
            <FolderPicker
              label="Source folder"
              value={effectiveSourceRoot}
              onChange={onSourceRootChange}
              disabled={reorganizeInPlace}
            />
            <Switch
              label="Same as destination (reorganize)"
              checked={reorganizeInPlace}
              onChange={onReorganizeInPlaceChange}
              className="w-fit gap-3"
            />
          </div>

          <div className="border-t border-border-color p-3 flex-shrink-0 flex flex-col gap-1">
            <Text variant={TextVariants.small} className="uppercase tracking-wide">
              Folder template
            </Text>
            <Input value={folderTemplate} onChange={(e) => onFolderTemplateChange(e.target.value)} />
            <div className="flex items-center justify-between gap-2">
              <Text variant={TextVariants.small}>
                Preview: <span className="text-text-primary">{templatePreview || '—'}</span>
              </Text>
              <button
                type="button"
                onClick={() => open(CHRONO_STRFTIME_DOCS_URL)}
                aria-label="Open chrono strftime format reference in browser"
                className="flex items-center gap-1 text-xs text-text-secondary hover:text-accent transition-colors shrink-0"
              >
                chrono format reference
                <ExternalLink size={12} />
              </button>
            </div>
          </div>
        </>
      )}

      <div className="border-t border-border-color p-3 flex-shrink-0 flex flex-col gap-2">
        {isPlanCurrent ? (
          <Button disabled className="w-full" data-tooltip="Coming soon">
            <Import size={16} />
            Import
          </Button>
        ) : (
          <Button onClick={onScan} disabled={!canScan || loading} className="w-full">
            {loading ? <Loader2 size={16} className="animate-spin" /> : <Search size={16} />}
            {loading ? `Scanning… ${scannedCount} file${scannedCount === 1 ? '' : 's'} so far` : 'Scan'}
          </Button>
        )}

        {error && (
          <Text variant={TextVariants.body} color="error">
            {error}
          </Text>
        )}
      </div>
    </div>
  );
}
