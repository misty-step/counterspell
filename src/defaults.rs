pub(crate) const STORE_VERSION: u8 = 2;
pub(crate) const DEFAULT_RECENT_HOURS: u64 = 72;
pub(crate) const DEFAULT_TRANSCRIPT_QUIET_SECONDS: u64 = 30;
pub(crate) const DEFAULT_DEBOUNCE_SECONDS: u64 = 300;
pub(crate) const DEFAULT_TARGET_MODEL: &str = "claude-fable-5";
pub(crate) const COMPACT_WAIT_TIMEOUT_MS: u64 = 180_000;
/// Best-effort pause for Escape to end the current turn so the queued
/// compact executes immediately instead of at the end of a resumed turn.
/// Failure is ignored — the chain is queue-safe without it.
pub(crate) const INTERRUPT_WAIT_TIMEOUT_MS: u64 = 15_000;
/// Margin added on top of a `herdr wait --timeout` when sizing the
/// subprocess kill, so counterspell never truncates its own wait.
pub(crate) const HERDR_WAIT_MARGIN_MS: u64 = 5_000;
/// How long the in-flight marker blocks a second remediation chain. The
/// queued chain normally lands within a compact's duration; if drift still
/// shows after this, the chain is presumed lost and the fast path re-fires.
pub(crate) const PENDING_COMPACT_EXPIRY_SECONDS: u64 = 300;
pub(crate) const COMPACT_COMMAND: &str = "/compact Plain handoff: summarize the current goal, repo/session state, exact next action, and any risks. Keep it factual and compact.";
