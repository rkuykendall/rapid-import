import { useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-shell';
import { ExternalLink, Loader2, Search } from 'lucide-react';
import TitleBar from './window/TitleBar';
import Button from './components/ui/Button';
import Input from './components/ui/Input';
import Text from './components/ui/Text';
import FolderPicker from './components/FolderPicker';
import PlanItemRow from './components/PlanItemRow';
import { useScan } from './hooks/useScan';
import { useFolderTemplatePreview } from './hooks/useFolderTemplatePreview';
import { DEFAULT_THEME_ID, THEMES } from './utils/themes';
import { TextVariants } from './types/typography';

const DEFAULT_FOLDER_TEMPLATE = '%Y/%Y-%m-%d';
const CHRONO_STRFTIME_DOCS_URL = 'https://docs.rs/chrono/latest/chrono/format/strftime/index.html';

function useAppliedTheme() {
  useEffect(() => {
    const root = document.documentElement;
    const theme = THEMES.find((t) => t.id === DEFAULT_THEME_ID);
    if (!theme) return;

    Object.entries(theme.cssVariables).forEach(([key, value]) => {
      root.style.setProperty(key, value);
    });
    root.style.setProperty('--font-family', "'Poppins', system-ui, sans-serif");
  }, []);
}

export default function App() {
  useAppliedTheme();

  const [sourceRoot, setSourceRoot] = useState('');
  const [destinationRoot, setDestinationRoot] = useState('');
  const [folderTemplate, setFolderTemplate] = useState(DEFAULT_FOLDER_TEMPLATE);
  const { plan, loading, error, scan } = useScan();
  const templatePreview = useFolderTemplatePreview(folderTemplate);

  const canScan = sourceRoot.trim() !== '' && destinationRoot.trim() !== '' && folderTemplate.trim() !== '';

  return (
    <div className="h-screen w-screen flex flex-col bg-bg-primary">
      <TitleBar />
      <div className="flex-1 overflow-y-auto pt-10">
        <div className="max-w-3xl mx-auto p-6 flex flex-col gap-6">
          <Text variant={TextVariants.headline}>RapidImport</Text>

          <div className="bg-surface rounded-lg p-4 flex flex-col gap-4">
            <FolderPicker label="Source folder" value={sourceRoot} onChange={setSourceRoot} />
            <FolderPicker label="Destination folder" value={destinationRoot} onChange={setDestinationRoot} />
            <div className="flex flex-col gap-1">
              <Text variant={TextVariants.label}>Folder template</Text>
              <Input value={folderTemplate} onChange={(e) => setFolderTemplate(e.target.value)} />
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
            <Button
              onClick={() => scan(sourceRoot, destinationRoot, folderTemplate)}
              disabled={!canScan || loading}
              className="self-start"
            >
              {loading ? <Loader2 size={16} className="animate-spin" /> : <Search size={16} />}
              {loading ? 'Scanning… this can take a moment for large folders' : 'Scan (dry run)'}
            </Button>
          </div>

          {error && (
            <Text variant={TextVariants.body} color="error">
              {error}
            </Text>
          )}

          {plan && (
            <div className="bg-surface rounded-lg p-4">
              <Text variant={TextVariants.heading} className="mb-2">
                {plan.items.length} file{plan.items.length === 1 ? '' : 's'} scanned. Nothing has been written — this
                is a dry-run preview only.
              </Text>
              {plan.items.length === 0 ? (
                <Text variant={TextVariants.body}>No files found.</Text>
              ) : (
                <div>
                  {plan.items.map((item) => (
                    <PlanItemRow key={item.source_path} item={item} />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
