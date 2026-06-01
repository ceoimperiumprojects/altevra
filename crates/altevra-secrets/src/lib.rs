pub mod capture;
pub mod detector;
pub mod ingest_guard;
pub mod redactor;
pub mod store;

pub use capture::{auto_capture, derive_key_name, fingerprint8, CaptureResult};
pub use detector::{detect_secrets, SecretKind, SecretMatch};
pub use ingest_guard::{ingest_guard, Guarded, SecretSighting};
pub use redactor::{redact, redact_with};
pub use store::{SecretBackend, SecretStore};
