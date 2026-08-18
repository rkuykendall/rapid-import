import { useMemo, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import clsx from "clsx";
import { ArrowDown, ArrowUp } from "lucide-react";
import Text from "../ui/Text";
import Dropdown from "../ui/Dropdown";
import PanelHeader from "../ui/PanelHeader";
import { TextVariants } from "../../types/typography";
import { Plan, PlanItem, DuplicateGroup } from "../../types/plan";

function Badge({
  children,
  tone,
  tooltip,
}: {
  children: string;
  tone: "info" | "warning" | "error" | "success";
  tooltip?: string;
}) {
  const toneClasses = {
    info: "bg-blue-400/10 text-blue-400",
    warning: "bg-yellow-400/10 text-yellow-400",
    error: "bg-red-400/10 text-red-400",
    success: "bg-green-400/10 text-green-400",
  }[tone];

  return (
    <span
      className={clsx(
        "px-2 py-0.5 rounded-md text-xs font-medium whitespace-nowrap",
        toneClasses,
      )}
      data-tooltip={tooltip}
    >
      {children}
    </span>
  );
}

// Strips `root` off the front of `path`, leaving the part relative to it —
// e.g. a source/destination root of "/Users/x/Pictures" turns
// "/Users/x/Pictures/2024/IMG_0001.jpg" into "2024/IMG_0001.jpg". Falls
// back to the full path if `root` is empty or isn't actually a prefix
// (shouldn't happen given `scannedFor`, but stay safe rather than lie).
function relativeTo(path: string, root: string): string {
  if (!root) return path;
  const normalizedRoot = root.endsWith("/") ? root : `${root}/`;
  return path.startsWith(normalizedRoot)
    ? path.slice(normalizedRoot.length)
    : path;
}

// Builds a lookup from source path to every `DuplicateGroup` it's a member
// of — `find_duplicates` returns one entry per *pairing* (a file matching
// three others is three separate groups), so a given path can appear in
// more than one.
function groupsByPath(
  duplicates: DuplicateGroup[],
): Map<string, DuplicateGroup[]> {
  const map = new Map<string, DuplicateGroup[]>();
  for (const group of duplicates) {
    for (const member of group.members) {
      const existing = map.get(member);
      if (existing) existing.push(group);
      else map.set(member, [group]);
    }
  }
  return map;
}

// Same badge set (and the same precedence) as scan_cli's flag rendering in
// src-tauri/src/bin/scan_cli.rs, so the CLI dry-run and this UI never say
// different things about the same plan — plus exact matches from
// `find_duplicates`, which scan_cli's own summary line also prints
// separately (see its "Duplicate groups:" section).
function badgesFor(
  item: PlanItem,
  duplicateGroups: DuplicateGroup[] | undefined,
  sourceRoot: string,
) {
  const badges: Array<{
    tone: "info" | "warning" | "error" | "success";
    text: string;
    tooltip?: string;
  }> = [];
  if (item.no_op) badges.push({ tone: "success", text: "Already organized" });
  if (item.already_imported)
    badges.push({ tone: "success", text: "Already imported" });
  if (item.excluded) badges.push({ tone: "info", text: "Excluded" });
  if (item.needs_review) badges.push({ tone: "warning", text: "Needs review" });
  switch (item.conflict) {
    case "destination_exists":
      badges.push({ tone: "error", text: "Conflict: destination exists" });
      break;
    case "duplicate_at_destination":
      badges.push({ tone: "info", text: "Duplicate at destination" });
      break;
    case "duplicate_in_plan":
      badges.push({ tone: "error", text: "Conflict: duplicate in plan" });
      break;
  }
  if (duplicateGroups?.length) {
    const others = duplicateGroups
      .flatMap((g) => g.members)
      .filter((m) => m !== item.source_path)
      .map((m) => relativeTo(m, sourceRoot));
    badges.push({
      tone: "info",
      text: "Duplicate found",
      tooltip: `Matches: ${[...new Set(others)].join(", ")}`,
    });
  }
  return badges;
}

type StatusFilter =
  | "all"
  | "needs_review"
  | "conflict"
  | "duplicate"
  | "already_organized";

const STATUS_FILTER_OPTIONS: Array<{ value: StatusFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "needs_review", label: "Needs review" },
  { value: "conflict", label: "Conflict" },
  { value: "duplicate", label: "Duplicate" },
  { value: "already_organized", label: "Already organized" },
];

function matchesFilter(
  item: PlanItem,
  filter: StatusFilter,
  hasDuplicateGroup: boolean,
): boolean {
  switch (filter) {
    case "all":
      return true;
    case "needs_review":
      return item.needs_review;
    case "conflict":
      return (
        item.conflict === "destination_exists" ||
        item.conflict === "duplicate_in_plan"
      );
    case "duplicate":
      return (
        item.conflict === "duplicate_at_destination" ||
        item.already_imported ||
        hasDuplicateGroup
      );
    case "already_organized":
      return item.no_op;
  }
}

type SortKey = "source" | "destination" | "date" | "confidence";

function sortValue(item: PlanItem, key: SortKey): string | number {
  switch (key) {
    case "source":
      return item.source_path;
    case "destination":
      return item.destination_path ?? "";
    case "date":
      return item.candidates[0]?.date ?? "";
    case "confidence":
      return item.candidates[0]?.confidence ?? 0;
  }
}

const COLUMNS: Array<{ key: SortKey; label: string }> = [
  { key: "source", label: "Source" },
  { key: "destination", label: "Destination" },
  { key: "date", label: "Resolved date" },
  { key: "confidence", label: "Confidence" },
];

interface PlanTableProps {
  plan: Plan | null;
  duplicates: DuplicateGroup[];
  sourceRoot: string;
  destinationRoot: string;
  onToggleExcluded(sourcePath: string): void;
}

export default function PlanTable({
  plan,
  duplicates,
  sourceRoot,
  destinationRoot,
  onToggleExcluded,
}: PlanTableProps) {
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [sortKey, setSortKey] = useState<SortKey>("source");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("asc");

  const duplicateGroupsByPath = useMemo(
    () => groupsByPath(duplicates),
    [duplicates],
  );

  const rows = useMemo(() => {
    if (!plan) return [];
    // Sidecars (.xmp/.rrdata/.rrexif) always follow their primary photo's
    // destination/review status (see scan.rs's align_sidecars_with_primary)
    // and move alongside it at commit time — they don't need their own row.
    // The primary instead lists them in the Sidecars column.
    const filtered = plan.items.filter(
      (item) =>
        !item.is_sidecar &&
        matchesFilter(
          item,
          statusFilter,
          duplicateGroupsByPath.has(item.source_path),
        ),
    );
    return [...filtered].sort((a, b) => {
      const va = sortValue(a, sortKey);
      const vb = sortValue(b, sortKey);
      const cmp =
        typeof va === "number" && typeof vb === "number"
          ? va - vb
          : String(va).localeCompare(String(vb));
      return sortDir === "asc" ? cmp : -cmp;
    });
  }, [plan, statusFilter, sortKey, sortDir, duplicateGroupsByPath]);

  const handleSortClick = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
  };

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <PanelHeader
        actions={
          plan && (
            <Dropdown
              className="w-48 shrink-0"
              value={statusFilter}
              onChange={(value) => setStatusFilter(value)}
              options={STATUS_FILTER_OPTIONS}
            />
          )
        }
      >
        Plan
      </PanelHeader>

      {!plan ? (
        <div className="flex-1 flex items-center justify-center">
          <Text variant={TextVariants.body}>
            Run a scan to see results here.
          </Text>
        </div>
      ) : (
        <>
          <div className="px-4 py-2 border-b border-border-color/50 flex-shrink-0">
            <Text variant={TextVariants.body}>
              {plan.items.length} file{plan.items.length === 1 ? "" : "s"}{" "}
              scanned. Nothing has been written — this is a dry-run preview
              only.
            </Text>
          </div>

          {rows.length === 0 ? (
            <div className="flex-1 flex items-center justify-center">
              <Text variant={TextVariants.body}>
                No files match this filter.
              </Text>
            </div>
          ) : (
            <div className="flex-1 overflow-auto">
              <table className="w-full text-sm border-collapse">
                <thead className="sticky top-0 bg-bg-secondary">
                  <tr>
                    <th className="w-10 px-4 py-2 border-b border-border-color" />
                    {COLUMNS.map((col) => (
                      <th
                        key={col.key}
                        className="text-left px-4 py-2 border-b border-border-color font-medium"
                      >
                        <button
                          type="button"
                          onClick={() => handleSortClick(col.key)}
                          className="flex items-center gap-1 text-text-secondary hover:text-text-primary transition-colors"
                        >
                          {col.label}
                          {sortKey === col.key &&
                            (sortDir === "asc" ? (
                              <ArrowUp size={12} />
                            ) : (
                              <ArrowDown size={12} />
                            ))}
                        </button>
                      </th>
                    ))}
                    <th className="text-left px-4 py-2 border-b border-border-color font-medium text-text-secondary">
                      Sidecars
                    </th>
                    <th className="text-left px-4 py-2 border-b border-border-color font-medium text-text-secondary">
                      Status
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((item) => {
                    const chosen = item.candidates[0];
                    return (
                      <tr
                        key={item.source_path}
                        className={clsx(
                          "hover:bg-card-active/50 transition-colors",
                          item.excluded && "opacity-50",
                        )}
                      >
                        <td className="px-4 py-2 border-b border-border-color">
                          <input
                            type="checkbox"
                            checked={!item.excluded}
                            onChange={() => onToggleExcluded(item.source_path)}
                            aria-label={
                              item.excluded
                                ? "Include in import"
                                : "Exclude from import"
                            }
                            data-tooltip={
                              item.excluded
                                ? "Include in import"
                                : "Exclude from import"
                            }
                          />
                        </td>
                        <td className="px-4 py-2 border-b border-border-color max-w-xs">
                          <button
                            type="button"
                            onClick={() =>
                              revealItemInDir(item.source_path).catch(
                                console.error,
                              )
                            }
                            data-tooltip={`Show ${item.source_path} in folder`}
                            className="truncate hover:text-accent hover:underline transition-colors text-left w-full"
                          >
                            {relativeTo(item.source_path, sourceRoot)}
                          </button>
                        </td>
                        <td
                          className="px-4 py-2 border-b border-border-color max-w-xs truncate text-text-secondary"
                          data-tooltip={item.destination_path ?? undefined}
                        >
                          {item.destination_path
                            ? relativeTo(item.destination_path, destinationRoot)
                            : "(unresolved)"}
                        </td>
                        <td className="px-4 py-2 border-b border-border-color whitespace-nowrap text-text-secondary">
                          {chosen
                            ? `${chosen.date.replace("T", " ")} · ${chosen.source}`
                            : "—"}
                        </td>
                        <td className="px-4 py-2 border-b border-border-color text-text-secondary">
                          {chosen
                            ? `${(chosen.confidence * 100).toFixed(0)}%`
                            : "—"}
                        </td>
                        <td className="px-4 py-2 border-b border-border-color text-text-secondary whitespace-nowrap">
                          {item.sidecar_extensions.length
                            ? item.sidecar_extensions
                                .map((ext) => `.${ext}`)
                                .join(", ")
                            : "—"}
                        </td>
                        <td className="px-4 py-2 border-b border-border-color">
                          <div className="flex gap-1.5 flex-wrap">
                            {badgesFor(
                              item,
                              duplicateGroupsByPath.get(item.source_path),
                              sourceRoot,
                            ).map((b) => (
                              <Badge
                                key={b.text}
                                tone={b.tone}
                                tooltip={b.tooltip}
                              >
                                {b.text}
                              </Badge>
                            ))}
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  );
}
