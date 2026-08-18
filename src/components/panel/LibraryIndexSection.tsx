import { open } from '@tauri-apps/plugin-dialog';
import { Database, FolderSearch, Loader2 } from 'lucide-react';
import Button from '../ui/Button';
import Text from '../ui/Text';
import PanelSection from '../ui/PanelSection';
import { TextVariants } from '../../types/typography';
import { useIndexLibrary } from '../../hooks/useIndexLibrary';

interface LibraryIndexSectionProps {
  destinationRoot: string;
}

// Lets the user build/refresh the SQLite index ("dedupe hashes") from what's
// already on disk at the destination — the mechanism for recognizing
// content that was never imported *through* this app (an existing library,
// or one imported by another tool) as "already present" on a future scan.
// Explicit and user-triggered only, same reasoning as the Scan action
// itself: a plain scan never re-walks the destination, which is what keeps
// every import fast regardless of how large the destination has grown.
export default function LibraryIndexSection({ destinationRoot }: LibraryIndexSectionProps) {
  const { loading, error, indexedCount, summary, refreshIndex } = useIndexLibrary();

  const disabled = loading || destinationRoot.trim() === '';

  const handleHashAll = () => {
    refreshIndex(destinationRoot);
  };

  const handleHashSubfolder = async () => {
    const selected = await open({ directory: true, multiple: false, defaultPath: destinationRoot });
    if (typeof selected === 'string') {
      refreshIndex(destinationRoot, selected);
    }
  };

  return (
    <PanelSection label="Dedupe hashes">
      <div className="flex flex-col gap-2">
        <Button onClick={handleHashAll} disabled={disabled} className="w-full bg-surface">
          {loading ? <Loader2 size={16} className="animate-spin" /> : <Database size={16} />}
          Hash missing
        </Button>
        <Button onClick={handleHashSubfolder} disabled={disabled} className="w-full bg-surface">
          <FolderSearch size={16} />
          Hash missing in subfolder
        </Button>
      </div>

      {loading && (
        <Text variant={TextVariants.small}>
          Hashing… {indexedCount} file{indexedCount === 1 ? '' : 's'} so far
        </Text>
      )}

      {error && (
        <Text variant={TextVariants.body} color="error">
          {error}
        </Text>
      )}

      {summary && !loading && (
        <Text variant={TextVariants.small}>
          {summary.new} new, {summary.unchanged} unchanged, {summary.moved} moved, {summary.content_changed}{' '}
          changed, {summary.removed} removed.
        </Text>
      )}
    </PanelSection>
  );
}
