pub mod capture;
pub mod detector;
pub mod ingest_guard;
pub mod pii;
pub mod redactor;
pub mod store;

pub use capture::{auto_capture, derive_key_name, fingerprint8, CaptureResult};
pub use detector::{detect_secrets, SecretKind, SecretMatch};
pub use ingest_guard::{
    content_is_high_water, guard_text, ingest_guard, Guarded, GuardedText, SecretSighting,
};
pub use pii::{detect_pii, PiiKind, PiiMatch};
pub use redactor::{redact, redact_with};
pub use store::{SecretBackend, SecretStore};
