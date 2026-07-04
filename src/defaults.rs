pub(crate) const STORE_VERSION: u8 = 2;
pub(crate) const DEFAULT_RECENT_HOURS: u64 = 72;
pub(crate) const DEFAULT_TRANSCRIPT_QUIET_SECONDS: u64 = 30;
pub(crate) const DEFAULT_DEBOUNCE_SECONDS: u64 = 300;
pub(crate) const DEFAULT_TARGET_MODEL: &str = "claude-fable-5";
pub(crate) const COMPACT_WAIT_TIMEOUT_MS: u64 = 180_000;
/// How long a queued fast-path compact stays pending before the session
/// falls back to the ordinary idle-path remediation.
pub(crate) const PENDING_COMPACT_EXPIRY_SECONDS: u64 = 1800;
pub(crate) const COMPACT_COMMAND: &str = "/compact Plain handoff: summarize the current goal, repo/session state, exact next action, and any risks. Keep it factual and compact.";
