//! Read-side safety: the ExposureGate (working draft §2.3, RECONCILIATION R1).
//!
//! The PreWriteSafetyGate (`ingest_guard`) lives in `altevra-secrets` because it
//! needs the secret detectors; the ExposureGate lives here because it only needs
//! envelope + sensitivity comparison (no detectors), so it stays in core where
//! every reader (packet compiler, CLI, MCP) can call it without a dep cycle.

pub mod exposure_gate;

pub use exposure_gate::{DenyReason, ExposureDecision, ExposureGate, ExposureRequest};
