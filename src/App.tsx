import { useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-shell';
import { ExternalLink, Loader2, Search } from 'lucide-react';
import TitleBar from './window/TitleBar';
import Button from './components/ui/Button';
import Input from './components/ui/Input';
import Text from './components/ui/Text';
import Switch from './components/ui/Switch';
import Resizer, { Orientation } from './components/ui/Resizer';
import GlobalTooltip from './components/ui/GlobalTooltip';
import FolderPicker from './components/FolderPicker';
import SourcesPanel from './components/panel/SourcesPanel';
import PlanTable from './components/panel/PlanTable';
import SummaryPanel from './components/panel/SummaryPanel';
import { useScan } from './hooks/useScan';
import { useFolderTemplatePreview } from './hooks/useFolderTemplatePreview';
import { useDestinationProfile } from './hooks/useDestinationProfile';
import { useProfiles } from './hooks/useProfiles';
import { useResizablePanel } from './hooks/useResizablePanel';
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

  // Destination is picked first, RapidRAW-style — everything else (source,
  // template) is recalled per-destination once it's set, via
  // useDestinationProfile. The Sources sidebar lists every destination
  // ever used (via useProfiles) so switching between libraries is a click,
  // not retyping a path.
  const [destinationRoot, setDestinationRoot] = useState('');
  const [reorganizeInPlace, setReorganizeInPlace] = useState(false);
  const { sourceRoot, setSourceRoot, folderTemplate, setFolderTemplate, save } = useDestinationProfile(
    destinationRoot,
    DEFAULT_FOLDER_TEMPLATE,
  );
  const { plan, loading, error, scannedCount, scan } = useScan();
  const templatePreview = useFolderTemplatePreview(folderTemplate);
  const { profiles, refresh: refreshProfiles } = useProfiles();
  const leftPanel = useResizablePanel(256, 'left');
  const rightPanel = useResizablePanel(320, 'right');

  const hasDestination = destinationRoot.trim() !== '';
  // "Same as destination" means reorganizing an existing library in place —
  // the source and destination roots are the same tree; see scan.rs's
  // no-op/legal-subfolder handling for what that actually does on disk.
  const effectiveSourceRoot = reorganizeInPlace ? destinationRoot : sourceRoot;
  const canScan = hasDestination && effectiveSourceRoot.trim() !== '' && folderTemplate.trim() !== '';

  const handleScan = async () => {
    await save(effectiveSourceRoot);
    refreshProfiles();
    scan(effectiveSourceRoot, destinationRoot, folderTemplate);
  };

  return (
    <>
      <div className="h-screen w-screen flex flex-col bg-bg-primary overflow-hidden">
        <div className="shrink-0 overflow-hidden z-50">
          <TitleBar />
        </div>
        <div className="flex-1 flex flex-col min-h-0 p-2 gap-2">
          <div className="flex flex-row flex-grow h-full min-h-0 gap-2">
            <div
              className="flex-shrink-0 bg-bg-secondary rounded-lg overflow-hidden"
              style={{ width: leftPanel.width }}
            >
              <SourcesPanel profiles={profiles} activeDestinationRoot={destinationRoot} onSelect={setDestinationRoot} />
            </div>

            <Resizer direction={Orientation.Vertical} onMouseDown={leftPanel.onResizeStart} />

            <div className="flex-1 flex flex-col min-w-0 bg-bg-secondary rounded-lg overflow-hidden">
              {!hasDestination ? (
                <div className="flex-1 flex items-center justify-center">
                  <Text variant={TextVariants.body}>Select or add a destination on the left to get started.</Text>
                </div>
              ) : (
                <>
                  <div className="p-4 border-b border-border-color flex-shrink-0 flex flex-col gap-3">
                    <Text variant={TextVariants.heading} className="truncate" data-tooltip={destinationRoot}>
                      {destinationRoot}
                    </Text>

                    <div className="flex flex-col gap-1">
                      <FolderPicker
                        label="Source folder"
                        value={effectiveSourceRoot}
                        onChange={setSourceRoot}
                        disabled={reorganizeInPlace}
                      />
                      <Switch
                        label="Same as destination (reorganize)"
                        checked={reorganizeInPlace}
                        onChange={setReorganizeInPlace}
                        className="w-fit gap-3"
                      />
                    </div>

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

                    <Button onClick={handleScan} disabled={!canScan || loading} className="self-start">
                      {loading ? <Loader2 size={16} className="animate-spin" /> : <Search size={16} />}
                      {loading
                        ? `Scanning… ${scannedCount} file${scannedCount === 1 ? '' : 's'} so far`
                        : 'Scan (dry run)'}
                    </Button>

                    {error && (
                      <Text variant={TextVariants.body} color="error">
                        {error}
                      </Text>
                    )}
                  </div>

                  <PlanTable plan={plan} />
                </>
              )}
            </div>

            <Resizer direction={Orientation.Vertical} onMouseDown={rightPanel.onResizeStart} />

            <div
              className="flex-shrink-0 bg-bg-secondary rounded-lg overflow-hidden"
              style={{ width: rightPanel.width }}
            >
              <SummaryPanel plan={plan} />
            </div>
          </div>
        </div>
      </div>
      <GlobalTooltip />
    </>
  );
}
