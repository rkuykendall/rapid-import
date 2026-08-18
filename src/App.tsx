import { useEffect, useState } from 'react';
import TitleBar from './window/TitleBar';
import Text from './components/ui/Text';
import Resizer, { Orientation } from './components/ui/Resizer';
import GlobalTooltip from './components/ui/GlobalTooltip';
import Panel from './components/ui/Panel';
import PanelHeader from './components/ui/PanelHeader';
import LeftPanel from './components/panel/LeftPanel';
import PlanTable from './components/panel/PlanTable';
import SummaryPanel, { DuplicatePolicy, TransferMode } from './components/panel/SummaryPanel';
import { useScan } from './hooks/useScan';
import { useCommit } from './hooks/useCommit';
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
  const { sourceRoot, setSourceRoot, folderTemplate, setFolderTemplate, save } = useDestinationProfile(
    destinationRoot,
    DEFAULT_FOLDER_TEMPLATE,
  );
  const { plan, duplicates, scannedFor, loading, error, scannedCount, scan, toggleExcluded } = useScan();
  const { committing, error: commitError, summary: commitSummary, commit, resetSummary } = useCommit();
  const templatePreview = useFolderTemplatePreview(folderTemplate);
  const { profiles, refresh: refreshProfiles } = useProfiles();
  // Kept as raw state (not reset) so a choice made while reorganizing
  // survives toggling out and back in, but "duplicates_folder" only ever
  // applies while actually reorganizing — see `effectiveDuplicatePolicy`
  // below.
  const [duplicatePolicy, setDuplicatePolicy] = useState<DuplicatePolicy>('skip');
  // Always freely pickable (unlike duplicate policy above) — it just
  // re-defaults to Move whenever `isReorganizeInPlace` flips true and Copy
  // when it flips false, via the effect below, rather than being forced
  // either way.
  const [transferMode, setTransferMode] = useState<TransferMode>('copy');
  const leftPanel = useResizablePanel(320, 'left');
  const rightPanel = useResizablePanel(320, 'right');

  const hasDestination = destinationRoot.trim() !== '';
  // "Same as destination" is derived, not a separate mode to manage — it's
  // just true whenever the chosen source happens to equal the destination.
  // scan.rs's no-op/legal-subfolder handling already does the right thing
  // based on the paths alone, so there was never a real distinct "reorganize
  // mode" to track; the checkbox is purely a reflection of that fact.
  const isReorganizeInPlace = hasDestination && sourceRoot === destinationRoot;
  // "Move to Duplicates folder" only makes sense while reorganizing an
  // existing library — for a plain import, skip (leaving the source
  // device/card untouched) is already the standard, correct behavior.
  const effectiveDuplicatePolicy: DuplicatePolicy = isReorganizeInPlace ? duplicatePolicy : 'skip';
  const canScan = hasDestination && sourceRoot.trim() !== '' && folderTemplate.trim() !== '';
  // Whether the displayed plan (if any) was actually scanned for exactly
  // today's inputs — governs the action button's "Scan" vs "Import" label
  // without needing to imperatively clear `plan` on every input change.
  const isPlanCurrent =
    plan !== null &&
    scannedFor !== null &&
    scannedFor.sourceRoot === sourceRoot &&
    scannedFor.destinationRoot === destinationRoot &&
    scannedFor.folderTemplate === folderTemplate;

  const handleScan = async () => {
    await save(sourceRoot);
    refreshProfiles();
    scan(sourceRoot, destinationRoot, folderTemplate);
  };

  // Re-scans after a successful commit rather than clearing the plan —
  // moved/copied items settle into `no_op`/`already_imported`, so the same
  // Plan table view keeps working as a live "what's left" picture instead
  // of dropping the user back to an empty state.
  const handleCommit = async () => {
    if (!plan) return;
    resetSummary();
    const result = await commit(
      plan,
      destinationRoot,
      effectiveDuplicatePolicy,
      transferMode,
      isReorganizeInPlace ? 'reorganize' : 'import',
    );
    if (result) {
      scan(sourceRoot, destinationRoot, folderTemplate);
    }
  };

  // Re-default Transfer to Move the moment source/destination start
  // matching, and back to Copy the moment they stop — a starting point
  // only, not an override, so a deliberate choice made mid-mode (e.g.
  // Copy while reorganizing) survives until the mode itself flips again.
  useEffect(() => {
    setTransferMode(isReorganizeInPlace ? 'move' : 'copy');
  }, [isReorganizeInPlace]);

  return (
    <>
      <div className="h-screen w-screen flex flex-col bg-bg-primary overflow-hidden">
        <div className="shrink-0 overflow-hidden z-50">
          <TitleBar />
        </div>
        <div className="flex-1 flex flex-col min-h-0 p-2 gap-2">
          <div className="flex flex-row flex-grow h-full min-h-0">
            <Panel className="flex-shrink-0" style={{ width: leftPanel.width }}>
              <LeftPanel
                profiles={profiles}
                activeDestinationRoot={destinationRoot}
                onSelectDestination={setDestinationRoot}
                hasDestination={hasDestination}
                sourceRoot={sourceRoot}
                onSourceRootChange={setSourceRoot}
                isReorganizeInPlace={isReorganizeInPlace}
                folderTemplate={folderTemplate}
                onFolderTemplateChange={setFolderTemplate}
                templatePreview={templatePreview}
              />
            </Panel>

            <Resizer direction={Orientation.Vertical} onMouseDown={leftPanel.onResizeStart} />

            <Panel className="flex-1 flex flex-col min-w-0">
              {!hasDestination ? (
                <>
                  <PanelHeader>Plan</PanelHeader>
                  <div className="flex-1 flex items-center justify-center">
                    <Text variant={TextVariants.body}>Select or add a destination on the left to get started.</Text>
                  </div>
                </>
              ) : (
                <PlanTable
                  plan={plan}
                  duplicates={duplicates}
                  sourceRoot={scannedFor?.sourceRoot ?? ''}
                  destinationRoot={scannedFor?.destinationRoot ?? ''}
                  onToggleExcluded={toggleExcluded}
                />
              )}
            </Panel>

            <Resizer direction={Orientation.Vertical} onMouseDown={rightPanel.onResizeStart} />

            <Panel className="flex-shrink-0" style={{ width: rightPanel.width }}>
              <SummaryPanel
                plan={plan}
                canScan={canScan}
                isPlanCurrent={isPlanCurrent}
                loading={loading}
                scannedCount={scannedCount}
                onScan={handleScan}
                error={error}
                isReorganizeInPlace={isReorganizeInPlace}
                duplicatePolicy={effectiveDuplicatePolicy}
                onDuplicatePolicyChange={setDuplicatePolicy}
                transferMode={transferMode}
                onTransferModeChange={setTransferMode}
                destinationRoot={destinationRoot}
                committing={committing}
                commitError={commitError}
                commitSummary={commitSummary}
                onCommit={handleCommit}
              />
            </Panel>
          </div>
        </div>
      </div>
      <GlobalTooltip />
    </>
  );
}
