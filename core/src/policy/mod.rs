//! `couple_core::policy` — the pure interaction-decision policies (PLAN-CORE-SHELL-BOUNDARY
//! Phase E). Each submodule is a single, cross-platform home for one decision that turns a
//! gesture-reduced set of INPUTS into an OUTCOME, with no UI, no IO, and no ctx. These were
//! previously implemented Swift-only in `mac/Sources/CoupleKit` (MatchQuality, RepairDrop,
//! RowMerge, MediaRemoval, AnchorProposal, ShadowTelemetry); this module is their relocation —
//! a RELOCATION, not a behavior change. Each `decide`/`plan`/`propose`/`rows` is byte-faithful
//! to the Swift body it replaces, and the Swift policies become thin wrappers that round-trip
//! through the FFI (`cpl_policy_*`). User-facing strings/localization stay shell-side; core
//! returns structured decisions only. `export_include` is the one NEW decision (PLAN-COMPACT-MODE
//! §3, UX-DECISIONS V2.12), born here rather than relocated.

pub mod activity;
pub mod anchor;
pub mod export_include;
pub mod match_quality;
pub mod media_removal;
pub mod monitor;
pub mod repair_drop;
pub mod row_merge;
pub mod shadow;
pub mod sync_seed;

pub use activity::{ImportPhase, LineKind, MatchPhase, Stages};
pub use anchor::{AnchorCandidate, AnchorTuning};
pub use export_include::{IncludeDecision, IncludeInput};
pub use match_quality::{MatchQuality, SelectionProvenance};
pub use media_removal::{ClearSessionCounts, CoupleRef, MediaRemovalPlan};
pub use monitor::{MonitorAction, MonitorPlan, MonitorRequest};
pub use repair_drop::RepairDropOutcome;
pub use row_merge::RowMergePlan;
pub use shadow::ShadowEvidenceRow;
pub use sync_seed::{SeedPlan, SeedSource};
