import { useMemo } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink } from 'lucide-react';
import Input from '../ui/Input';
import Text from '../ui/Text';
import Switch from '../ui/Switch';
import RecentFoldersList, { RecentFolderItem } from './RecentFoldersList';
import { TextVariants } from '../../types/typography';
import { Profile } from '../../types/profile';

const CHRONO_STRFTIME_DOCS_URL = 'https://docs.rs/chrono/latest/chrono/format/strftime/index.html';

function folderName(path: string): string {
  return (
    path
      .split('/')
      .filter((part) => part.length > 0)
      .pop() ?? path
  );
}

// Distinct, order-preserving folder paths pulled from a `Profile` field —
// shared by both the Destinations and Source recents lists, since both are
// just "every distinct root we've ever seen in `profiles`."
function distinctFolders(profiles: Profile[], field: 'source_root' | 'destination_root'): RecentFolderItem[] {
  const seen = new Set<string>();
  const items: RecentFolderItem[] = [];
  for (const profile of profiles) {
    const path = profile[field];
    if (path && !seen.has(path)) {
      seen.add(path);
      items.push({ key: path, path, name: folderName(path) });
    }
  }
  return items;
}

interface LeftPanelProps {
  profiles: Profile[];
  activeDestinationRoot: string;
  onSelectDestination(destinationRoot: string): void;
  hasDestination: boolean;
  sourceRoot: string;
  onSourceRootChange(value: string): void;
  isReorganizeInPlace: boolean;
  folderTemplate: string;
  onFolderTemplateChange(value: string): void;
  templatePreview: string;
}

// Owns the whole left column. Destinations and Source are the same
// "+ Add" button + recents list pattern (via `RecentFoldersList`) — Source
// recents are just the distinct `source_root` values already sitting in
// `profiles`, no separate tracking needed. Every section stacks at its
// natural height so they float to the top under Destinations. The
// Scan/Import action button lives in the right column (`SummaryPanel`),
// not here.
export default function LeftPanel({
  profiles,
  activeDestinationRoot,
  onSelectDestination,
  hasDestination,
  sourceRoot,
  onSourceRootChange,
  isReorganizeInPlace,
  folderTemplate,
  onFolderTemplateChange,
  templatePreview,
}: LeftPanelProps) {
  const destinationItems = useMemo(() => distinctFolders(profiles, 'destination_root'), [profiles]);
  const sourceItems = useMemo(() => distinctFolders(profiles, 'source_root'), [profiles]);

  return (
    <div className="h-full flex flex-col">
      <div className="p-3 flex justify-between items-center shrink-0 border-b border-surface">
        <Text variant={TextVariants.title}>Setup</Text>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto flex flex-col">
        <RecentFoldersList
          label="Destinations"
          addLabel="Add destination"
          emptyMessage="No destinations yet — add one to get started."
          items={destinationItems}
          activeValue={activeDestinationRoot}
          onSelect={onSelectDestination}
        />

        {hasDestination && (
          <>
            <div className="border-t border-border-color/50">
              <RecentFoldersList
                label="Source"
                addLabel="Add source"
                emptyMessage="No sources yet — add one to get started."
                items={sourceItems}
                activeValue={sourceRoot}
                onSelect={onSourceRootChange}
              />
              <div className="px-3 pb-3">
                <Switch
                  label="Same as destination (reorganize)"
                  checked={isReorganizeInPlace}
                  onChange={() => {}}
                  disabled
                  className="w-fit gap-3"
                  tooltip="Reflects whether the chosen source matches the destination — pick a source folder that matches to reorganize in place."
                />
              </div>
            </div>

            <div className="border-t border-border-color/50 p-3 flex-shrink-0 flex flex-col gap-1">
              <Text variant={TextVariants.small} className="uppercase tracking-wide">
                Folder template
              </Text>
              <Input value={folderTemplate} onChange={(e) => onFolderTemplateChange(e.target.value)} />
              <Text variant={TextVariants.small}>
                Preview: <span className="text-text-primary">{templatePreview || '—'}</span>
              </Text>
              <button
                type="button"
                onClick={() => openUrl(CHRONO_STRFTIME_DOCS_URL)}
                aria-label="Open chrono strftime format reference in browser"
                className="flex items-center gap-1 text-xs text-text-secondary hover:text-accent transition-colors w-fit"
              >
                chrono format reference
                <ExternalLink size={12} />
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
