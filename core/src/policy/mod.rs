//! `couple_core::policy` — the pure interaction-decision policies (PLAN-CORE-SHELL-BOUNDARY
//! Phase E). Each submodule is a single, cross-platform home for one decision that turns a
//! gesture-reduced set of INPUTS into an OUTCOME, with no UI, no IO, and no ctx. These were
//! previously implemented Swift-only in `mac/Sources/CoupleKit` (MatchQuality, RepairDrop,
//! RowMerge, MediaRemoval, AnchorProposal, ShadowTelemetry); this module is their relocation —
//! a RELOCATION, not a behavior change. Each `decide`/`plan`/`propose`/`rows` is byte-faithful
//! to the Swift body it replaces, and the Swift policies become thin wrappers that round-trip
//! through the FFI (`cpl_policy_*`). User-facing strings/localization stay shell-side; core
//! returns structured decisions only.

pub mod activity;
pub mod anchor;
pub mod match_quality;
pub mod media_removal;
pub mod repair_drop;
pub mod row_merge;
pub mod shadow;

pub use activity::{ImportPhase, LineKind, MatchPhase, Stages};
pub use anchor::{AnchorCandidate, AnchorTuning};
pub use match_quality::{MatchQuality, SelectionProvenance};
pub use media_removal::{ClearSessionCounts, CoupleRef, MediaRemovalPlan};
pub use repair_drop::RepairDropOutcome;
pub use row_merge::RowMergePlan;
pub use shadow::ShadowEvidenceRow;
