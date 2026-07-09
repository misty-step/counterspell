pub(crate) const STORE_VERSION: u8 = 3;
pub(crate) const DEFAULT_RECENT_HOURS: u64 = 72;
pub(crate) const DEFAULT_TRANSCRIPT_QUIET_SECONDS: u64 = 30;
pub(crate) const DEFAULT_DEBOUNCE_SECONDS: u64 = 300;
pub(crate) const DEFAULT_TARGET_MODEL: &str = "claude-fable-5";
pub(crate) const MODEL_SWITCH_CONFIRM_DELAY_MS: u64 = 200;
/// Best-effort pause for Escape to end the current turn before sending the
/// plain-handoff compact command. Failure is ignored: the durable chain state
/// prevents duplicate compacts even if Herdr status lags the interrupt.
pub(crate) const INTERRUPT_WAIT_TIMEOUT_MS: u64 = 15_000;
/// Margin added on top of a `herdr wait --timeout` when sizing the
/// subprocess kill, so counterspell never truncates its own wait.
pub(crate) const HERDR_WAIT_MARGIN_MS: u64 = 5_000;
/// How long a sent remediation step blocks duplicate sends before the next
/// armed pass may attempt an explicit recovery.
pub(crate) const REMEDIATION_CHAIN_TIMEOUT_SECONDS: u64 = 300;
pub(crate) const COMPACT_COMMAND: &str = "/compact Plain handoff: summarize the current goal, repo/session state, exact next action, and any risks. Keep it factual and compact.";
pub(crate) const CONTINUE_COMMAND: &str = "continue";
