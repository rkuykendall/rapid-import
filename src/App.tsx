import { useEffect, useState } from 'react';
import TitleBar from './window/TitleBar';
import Text from './components/ui/Text';
import Resizer, { Orientation } from './components/ui/Resizer';
import GlobalTooltip from './components/ui/GlobalTooltip';
import LeftPanel from './components/panel/LeftPanel';
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
  const { plan, scannedFor, loading, error, scannedCount, scan } = useScan();
  const templatePreview = useFolderTemplatePreview(folderTemplate);
  const { profiles, refresh: refreshProfiles } = useProfiles();
  const leftPanel = useResizablePanel(320, 'left');
  const rightPanel = useResizablePanel(320, 'right');

  const hasDestination = destinationRoot.trim() !== '';
  // "Same as destination" means reorganizing an existing library in place —
  // the source and destination roots are the same tree; see scan.rs's
  // no-op/legal-subfolder handling for what that actually does on disk.
  const effectiveSourceRoot = reorganizeInPlace ? destinationRoot : sourceRoot;
  const canScan = hasDestination && effectiveSourceRoot.trim() !== '' && folderTemplate.trim() !== '';
  // Whether the displayed plan (if any) was actually scanned for exactly
  // today's inputs — governs the action button's "Scan" vs "Import" label
  // without needing to imperatively clear `plan` on every input change.
  const isPlanCurrent =
    plan !== null &&
    scannedFor !== null &&
    scannedFor.sourceRoot === effectiveSourceRoot &&
    scannedFor.destinationRoot === destinationRoot &&
    scannedFor.folderTemplate === folderTemplate;

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
              <LeftPanel
                profiles={profiles}
                activeDestinationRoot={destinationRoot}
                onSelectDestination={setDestinationRoot}
                hasDestination={hasDestination}
                onSourceRootChange={setSourceRoot}
                reorganizeInPlace={reorganizeInPlace}
                onReorganizeInPlaceChange={setReorganizeInPlace}
                effectiveSourceRoot={effectiveSourceRoot}
                folderTemplate={folderTemplate}
                onFolderTemplateChange={setFolderTemplate}
                templatePreview={templatePreview}
              />
            </div>

            <Resizer direction={Orientation.Vertical} onMouseDown={leftPanel.onResizeStart} />

            <div className="flex-1 flex flex-col min-w-0 bg-bg-secondary rounded-lg overflow-hidden">
              {!hasDestination ? (
                <div className="flex-1 flex items-center justify-center">
                  <Text variant={TextVariants.body}>Select or add a destination on the left to get started.</Text>
                </div>
              ) : (
                <PlanTable plan={plan} />
              )}
            </div>

            <Resizer direction={Orientation.Vertical} onMouseDown={rightPanel.onResizeStart} />

            <div
              className="flex-shrink-0 bg-bg-secondary rounded-lg overflow-hidden"
              style={{ width: rightPanel.width }}
            >
              <SummaryPanel
                plan={plan}
                canScan={canScan}
                isPlanCurrent={isPlanCurrent}
                loading={loading}
                scannedCount={scannedCount}
                onScan={handleScan}
                error={error}
              />
            </div>
          </div>
        </div>
      </div>
      <GlobalTooltip />
    </>
  );
}
