//! What `build.rs` recorded about this build.

/// Short commit hash, `+changes` when the tree had uncommitted changes,
/// `unknown` outside a git checkout.
pub const COMMIT: &str = env!("DOCTOR_TOOLKIT_COMMIT");
pub const TARGET: &str = env!("DOCTOR_TOOLKIT_TARGET");
/// Cargo profile (`debug` or `release`; the `dist` profile reports `release`).
pub const PROFILE: &str = env!("DOCTOR_TOOLKIT_PROFILE");
pub const RUSTC: &str = env!("DOCTOR_TOOLKIT_RUSTC");

/// `x86_64-unknown-linux-gnu, release, rustc 1.97.1 (…)`.
pub fn summary() -> String {
    format!("{TARGET}, {PROFILE}, {RUSTC}")
}
