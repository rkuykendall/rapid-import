import { open } from '@tauri-apps/plugin-dialog';
import clsx from 'clsx';
import { Folder, FolderPlus } from 'lucide-react';
import Text from '../ui/Text';
import SectionLabel from '../ui/SectionLabel';
import { TextVariants } from '../../types/typography';

export interface RecentFolderItem {
  key: string | number;
  path: string;
  name: string;
}

interface RecentFoldersListProps {
  label: string;
  addLabel: string;
  emptyMessage: string;
  items: RecentFolderItem[];
  activeValue: string;
  onSelect(path: string): void;
  disabled?: boolean;
}

// Shared by the Destinations and Source sections in `LeftPanel` — same
// "+ Add" button, uppercase section label (mirrors RapidRAW's own
// ALBUMS/FOLDERS convention), and row styling (RapidRAW's FolderTree
// `TreeNode` pattern: icon + truncated label, bg-surface when active).
// Capped height with internal scroll rather than stretching to fill the
// column, so multiple sections can stack compactly at the top instead of
// one section pushing the rest of the panel down.
export default function RecentFoldersList({
  label,
  addLabel,
  emptyMessage,
  items,
  activeValue,
  onSelect,
  disabled = false,
}: RecentFoldersListProps) {
  const handleAdd = async () => {
    if (disabled) return;
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === 'string') {
      onSelect(selected);
    }
  };

  return (
    <div className={clsx('flex flex-col p-2 gap-1', disabled && 'opacity-50 pointer-events-none')}>
      <button
        type="button"
        onClick={handleAdd}
        disabled={disabled}
        className="flex items-center gap-2 p-1.5 rounded-md text-text-secondary hover:bg-card-active hover:text-text-primary transition-colors"
      >
        <FolderPlus size={16} />
        <Text variant={TextVariants.label} className="select-none">
          {addLabel}
        </Text>
      </button>

      <div className="h-px bg-border-color my-1" />

      <SectionLabel className="px-1.5">{label}</SectionLabel>

      <div className="max-h-52 overflow-y-auto flex flex-col gap-0.5">
        {items.length === 0 && (
          <Text variant={TextVariants.small} className="px-1.5 py-1">
            {emptyMessage}
          </Text>
        )}
        {items.map((item) => {
          const isActive = item.path === activeValue;
          return (
            <button
              key={item.key}
              type="button"
              onClick={() => onSelect(item.path)}
              data-tooltip={item.path}
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
                {item.name}
              </Text>
            </button>
          );
        })}
      </div>
    </div>
  );
}
