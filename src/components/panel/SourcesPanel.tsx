import { open } from '@tauri-apps/plugin-dialog';
import clsx from 'clsx';
import { Folder, FolderPlus } from 'lucide-react';
import Text from '../ui/Text';
import { TextVariants } from '../../types/typography';
import { Profile } from '../../types/profile';

interface SourcesPanelProps {
  profiles: Profile[];
  activeDestinationRoot: string;
  onSelect(destinationRoot: string): void;
}

// Row styling mirrors RapidRAW's FolderTree `TreeNode` pattern (icon +
// truncated label, bg-surface when selected, hover:bg-card-active
// otherwise) — flattened, since profiles aren't a nested tree.
export default function SourcesPanel({ profiles, activeDestinationRoot, onSelect }: SourcesPanelProps) {
  const handleAddDestination = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === 'string') {
      onSelect(selected);
    }
  };

  const destinations = profiles.filter((p): p is Profile & { destination_root: string } => p.destination_root !== null);

  return (
    <div className="flex flex-col h-full p-2 gap-1">
      <button
        type="button"
        onClick={handleAddDestination}
        className="flex items-center gap-2 p-1.5 rounded-md text-text-secondary hover:bg-card-active hover:text-text-primary transition-colors"
      >
        <FolderPlus size={16} />
        <Text variant={TextVariants.label} className="select-none">
          Add destination
        </Text>
      </button>

      <div className="h-px bg-border-color my-1" />

      <div className="flex-1 overflow-y-auto flex flex-col gap-0.5">
        {destinations.length === 0 && (
          <Text variant={TextVariants.small} className="px-1.5 py-1">
            No destinations yet — add one to get started.
          </Text>
        )}
        {destinations.map((profile) => {
          const isActive = profile.destination_root === activeDestinationRoot;
          return (
            <button
              key={profile.id}
              type="button"
              onClick={() => onSelect(profile.destination_root)}
              title={profile.destination_root}
              className={clsx('flex items-center gap-2 p-1.5 rounded-md text-left transition-colors', {
                'bg-surface': isActive,
                'hover:bg-card-active': !isActive,
              })}
            >
              <Folder size={16} className={isActive ? 'text-accent' : 'text-text-secondary'} />
              <Text
                variant={TextVariants.label}
                color={isActive ? 'accent' : 'primary'}
                className="truncate select-none flex-1"
              >
                {profile.name}
              </Text>
            </button>
          );
        })}
      </div>
    </div>
  );
}
