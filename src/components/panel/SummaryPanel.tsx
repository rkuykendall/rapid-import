import { Import, Loader2, Search } from 'lucide-react';
import Button from '../ui/Button';
import Text from '../ui/Text';
import { TextVariants } from '../../types/typography';
import { Plan } from '../../types/plan';

interface StatRowProps {
  label: string;
  value: number;
}

function StatRow({ label, value }: StatRowProps) {
  return (
    <div className="flex items-center justify-between py-1.5">
      <Text variant={TextVariants.body}>{label}</Text>
      <Text variant={TextVariants.heading}>{value}</Text>
    </div>
  );
}

// Same categories scan_cli's summary line prints, so the CLI and this
// panel never disagree about the same plan.
function summarize(plan: Plan) {
  const items = plan.items;
  return {
    total: items.length,
    alreadyOrganized: items.filter((i) => i.no_op).length,
    alreadyImported: items.filter((i) => i.already_imported).length,
    duplicateAtDestination: items.filter((i) => i.conflict === 'duplicate_at_destination').length,
    needsReview: items.filter((i) => i.needs_review).length,
    conflicts: items.filter((i) => i.conflict === 'destination_exists' || i.conflict === 'duplicate_in_plan').length,
  };
}

interface SummaryPanelProps {
  plan: Plan | null;
  canScan: boolean;
  isPlanCurrent: boolean;
  loading: boolean;
  scannedCount: number;
  onScan(): void;
  error: string | null;
}

export default function SummaryPanel({
  plan,
  canScan,
  isPlanCurrent,
  loading,
  scannedCount,
  onScan,
  error,
}: SummaryPanelProps) {
  const stats = plan ? summarize(plan) : null;

  return (
    <div className="flex flex-col h-full">
      <div className="p-4 border-b border-border-color flex-shrink-0">
        <Text variant={TextVariants.heading}>Summary</Text>
      </div>

      <div className="flex-grow overflow-y-auto p-4">
        {!stats ? (
          <Text variant={TextVariants.body}>Run a scan to see a plan summary.</Text>
        ) : (
          <div className="flex flex-col divide-y divide-border-color">
            <StatRow label="Files scanned" value={stats.total} />
            <StatRow label="Already organized" value={stats.alreadyOrganized} />
            <StatRow label="Already imported" value={stats.alreadyImported} />
            <StatRow label="Duplicate at destination" value={stats.duplicateAtDestination} />
            <StatRow label="Needs review" value={stats.needsReview} />
            <StatRow label="Conflicts" value={stats.conflicts} />
          </div>
        )}
      </div>

      <div className="p-4 border-t border-border-color flex-shrink-0 flex flex-col gap-2">
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
