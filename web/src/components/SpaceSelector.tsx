import { useState } from "react";
import { postAction } from "../api/client";
import type { ApiSpace, StateResponse } from "../api/types";
import { activeSpaceId } from "../utils/sidebar";

/**
 * One dot per space, at the top of the sidebar.
 *
 * A space is a separate set of projects, agents, tasks and roots; switching
 * changes what the sidebar below is about. The daemon owns which one is
 * showing — one per profile — so a click asks and the next state poll brings
 * the answer back, which is also how a switch made on the desktop reaches here.
 *
 * Spaces are added, renamed and deleted on the desktop. This client shows them
 * and moves between them.
 */
export function SpaceSelector({ workspace }: { workspace: StateResponse | null }) {
  const [busy, setBusy] = useState(false);
  const spaces = workspace?.spaces ?? [];
  // One space is no choice, and a daemon from before spaces sends none.
  if (spaces.length < 2) return null;

  const active = activeSpaceId(workspace);

  const switchTo = async (space: ApiSpace) => {
    if (busy || space.id === active) return;
    setBusy(true);
    try {
      await postAction({ action: "space_activate", space_id: space.id });
    } catch {
      // The next poll shows what actually happened; a failed switch simply
      // leaves the highlight where it was.
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex items-center gap-1.5 border-b border-[var(--ok-border)] px-3 py-2">
      {spaces.map((space) => {
        const isActive = space.id === active;
        return (
          <button
            key={space.id}
            type="button"
            title={space.name}
            aria-label={space.name}
            aria-current={isActive ? "true" : undefined}
            onClick={() => void switchTo(space)}
            className="flex h-4 w-4 items-center justify-center"
          >
            <span
              className={[
                "rounded-full",
                isActive ? "h-[9px] w-[9px]" : "h-[7px] w-[7px]",
                space.agent_waiting
                  ? "bg-[var(--ok-yellow)]"
                  : isActive
                    ? "bg-[var(--ok-text)]"
                    : "bg-[var(--ok-text-muted)]",
                isActive ? "ring-1 ring-[var(--ok-blue)]" : "",
              ].join(" ")}
            />
          </button>
        );
      })}
      <span className="ml-auto truncate text-[10px] text-[var(--ok-text-muted)]">
        {spaces.find((s) => s.id === active)?.name ?? ""}
      </span>
    </div>
  );
}
