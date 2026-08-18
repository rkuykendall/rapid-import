import { Import, Loader2, Search } from "lucide-react";
import Button from "../ui/Button";
import Text from "../ui/Text";
import Dropdown from "../ui/Dropdown";
import PanelHeader from "../ui/PanelHeader";
import PanelSection from "../ui/PanelSection";
import LibraryIndexSection from "./LibraryIndexSection";
import { TextVariants } from "../../types/typography";
import { CommitSummary, Plan } from "../../types/plan";

export type DuplicatePolicy = "skip" | "duplicates_folder";

const DUPLICATE_POLICY_OPTIONS: Array<{
  value: DuplicatePolicy;
  label: string;
}> = [
  { value: "skip", label: "Skip (leave in place)" },
  { value: "duplicates_folder", label: "Move to Duplicates folder" },
];

export type TransferMode = "copy" | "move";

const TRANSFER_MODE_OPTIONS: Array<{ value: TransferMode; label: string }> = [
  { value: "copy", label: "Copy (leave source untouched)" },
  { value: "move", label: "Move (remove from source)" },
];

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
    duplicateAtDestination: items.filter(
      (i) => i.conflict === "duplicate_at_destination",
    ).length,
    needsReview: items.filter((i) => i.needs_review).length,
    conflicts: items.filter(
      (i) =>
        i.conflict === "destination_exists" ||
        i.conflict === "duplicate_in_plan",
    ).length,
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
  isReorganizeInPlace: boolean;
  duplicatePolicy: DuplicatePolicy;
  onDuplicatePolicyChange(value: DuplicatePolicy): void;
  transferMode: TransferMode;
  onTransferModeChange(value: TransferMode): void;
  destinationRoot: string;
  committing: boolean;
  commitError: string | null;
  commitSummary: CommitSummary | null;
  onCommit(): void;
}

export default function SummaryPanel({
  plan,
  canScan,
  isPlanCurrent,
  loading,
  scannedCount,
  onScan,
  error,
  isReorganizeInPlace,
  duplicatePolicy,
  onDuplicatePolicyChange,
  transferMode,
  onTransferModeChange,
  destinationRoot,
  committing,
  commitError,
  commitSummary,
  onCommit,
}: SummaryPanelProps) {
  const stats = plan ? summarize(plan) : null;

  return (
    <div className="flex flex-col h-full">
      <PanelHeader>Summary</PanelHeader>

      <div className="flex-grow overflow-y-auto p-4">
        {!stats ? (
          <Text variant={TextVariants.body}>
            Run a scan to see a plan summary.
          </Text>
        ) : (
          <div className="flex flex-col divide-y divide-border-color">
            <StatRow label="Files scanned" value={stats.total} />
            <StatRow label="Already organized" value={stats.alreadyOrganized} />
            <StatRow label="Already imported" value={stats.alreadyImported} />
            <StatRow
              label="Duplicate at destination"
              value={stats.duplicateAtDestination}
            />
            <StatRow label="Needs review" value={stats.needsReview} />
            <StatRow label="Conflicts" value={stats.conflicts} />
          </div>
        )}
      </div>

      <LibraryIndexSection destinationRoot={destinationRoot} />

      <PanelSection label="Transfer">
        <Dropdown
          value={transferMode}
          onChange={onTransferModeChange}
          options={TRANSFER_MODE_OPTIONS}
        />
      </PanelSection>

      <PanelSection label="Duplicates">
        <Dropdown
          disabled={!isReorganizeInPlace}
          value={duplicatePolicy}
          onChange={onDuplicatePolicyChange}
          options={DUPLICATE_POLICY_OPTIONS}
        />
      </PanelSection>

      <PanelSection>
        {isPlanCurrent ? (
          <Button
            onClick={onCommit}
            disabled={committing || stats?.total === 0}
            className="w-full"
          >
            {committing ? (
              <Loader2 size={16} className="animate-spin" />
            ) : (
              <Import size={16} />
            )}
            {committing ? "Importing…" : "Import"}
          </Button>
        ) : (
          <Button
            onClick={onScan}
            disabled={!canScan || loading}
            className="w-full"
          >
            {loading ? (
              <Loader2 size={16} className="animate-spin" />
            ) : (
              <Search size={16} />
            )}
            {loading
              ? `Scanning… ${scannedCount} file${scannedCount === 1 ? "" : "s"} so far`
              : "Scan"}
          </Button>
        )}

        {error && (
          <Text variant={TextVariants.body} color="error">
            {error}
          </Text>
        )}
        {commitError && (
          <Text variant={TextVariants.body} color="error">
            {commitError}
          </Text>
        )}
        {commitSummary && (
          <Text variant={TextVariants.body}>
            Imported {commitSummary.moved}, skipped{" "}
            {commitSummary.skipped +
              commitSummary.excluded +
              commitSummary.already_imported +
              commitSummary.duplicate_at_destination}
            .
          </Text>
        )}
      </PanelSection>
    </div>
  );
}
