// CompactLayout.tsx — the window's two arrangements as ONE tree (UX §V2.11;
// PLAN-COMPACT-MODE Phase 4 + 6; mac CompactLayout.swift / RootView.swift twins).
//
// Compact (about 360 × 700): four blocks top to bottom — SessionLine (identity,
// counts, "+ Add", "⤢ Full"), PairPreview (fixed: 16:9 player, PairStrip,
// Camera · Synced · External, offset · fit, or the inline candidate picker / "No
// audio" line), CoupleList (flex; only the rows scroll) and ExportPanel (pinned at
// the bottom). No loupes, overview, nudges, anchors or media-pool list. Empty
// session: the drop target fills the preview area, the list keeps its empty Ready
// section, the export panel stays pinned.
//
// Full: three columns — left rail = SessionLine + CoupleList + media pool; center =
// PairPreview (greedy hero) + the editor tools (loupes, overview, sync tools);
// right rail = ExportPanel + Files.
//
// The SAME elements render both: the column wrappers are `display: contents` in
// compact and CSS `order` stacks the four blocks, so a switch is a class change —
// nothing remounts (the <video> keeps playing) and layoutFlip.ts can FLIP the four
// `[data-blk]` boxes. Full-only surfaces are mounted only in full and fade in.
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { CoupleList } from './CoupleList';
import { EditorTools } from './CenterColumn';
import { ExportPanel } from './ExportPanel';
import { FilesList } from './ExportRail';
import { PairPreview } from './PairPreview';
import { PoolSection, useRailLayout } from './QueueRail';
import { SessionLine } from './SessionLine';

export function Arrangement(): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  const compact = model.layoutMode === 'compact';
  const { couplesRef, listRef, poolMax } = useRailLayout(model.layoutMode);
  const empty = !model.hasContent;

  return (
    <div className={`arrangement ${compact ? 'compact' : 'full'}${empty ? ' empty' : ''}`}>
      <div className="col col-rail">
        <div className="blk blk-hd" data-blk="hd">
          <SessionLine compact={compact} />
        </div>
        <div className="blk blk-ls" data-blk="ls">
          <CoupleList scrollRef={couplesRef} scrolls={!(compact && empty)} />
        </div>
        {!compact && (
          <div className="full-only pool-slot">
            <PoolSection listRef={listRef} poolMax={poolMax} />
          </div>
        )}
      </div>
      <div className="col col-center">
        <div className="blk blk-pv" data-blk="pv">
          <PairPreview compact={compact} />
        </div>
        {!compact && (
          <div className="full-only tools-slot">
            <EditorTools />
          </div>
        )}
      </div>
      <div className="col col-export">
        <div className="blk blk-ex" data-blk="ex">
          <ExportPanel />
        </div>
        {!compact && (
          <div className="full-only files-slot">
            <FilesList />
          </div>
        )}
      </div>
    </div>
  );
}
