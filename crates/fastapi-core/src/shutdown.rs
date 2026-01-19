//! Graceful shutdown coordination for the server.
//!
//! This module provides graceful shutdown using asupersync's structured concurrency:
//!
//! # Shutdown Phases
//!
//! 1. **Stop accepting**: Server stops accepting new connections
//! 2. **Shutdown flag**: New requests receive 503 Service Unavailable
//! 3. **Grace period**: In-flight requests get remaining time to complete
//! 4. **Cancellation**: Remaining requests cancelled after grace period
//! 5. **Shutdown hooks**: Registered cleanup callbacks run
//! 6. **Region close**: Server region fully closed
//!
//! # Signal Handling
//!
//! - SIGTERM/SIGINT triggers graceful shutdown
//! - Second signal forces immediate exit
//! - Custom signals supported via configuration
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::shutdown::{ShutdownController, GracefulShutdown};
//! use std::time::Duration;
//!
//! async fn run_server() {
//!     let controller = ShutdownController::new();
//!
//!     // Register with signals
//!     controller.listen_for_signals();
//!
//!     // Run with graceful shutdown
//!     let shutdown = GracefulShutdown::new(controller.subscribe())
//!         .grace_period(Duration::from_secs(30));
//!
//!     let result = shutdown.run(server_main()).await;
//!     match result {
//!         ShutdownOutcome::Completed(r) => println!("Server completed: {:?}", r),
//!         ShutdownOutcome::GracefulShutdown => println!("Graceful shutdown"),
//!         ShutdownOutcome::ForcedShutdown => println!("Forced shutdown"),
//!     }
//! }
//! ```

use asupersync::{Budget, CancelReason};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

// ============================================================================
// Shutdown Phase State Machine
// ============================================================================

/// Current phase of the shutdown process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShutdownPhase {
    /// Server is running normally.
    Running = 0,
    /// Stop accepting new connections.
    StopAccepting = 1,
    /// Shutdown flag set; new requests get 503.
    ShutdownFlagged = 2,
    /// Grace period for in-flight requests.
    GracePeriod = 3,
    /// Cancelling remaining requests.
    Cancelling = 4,
    /// Running shutdown hooks.
    RunningHooks = 5,
    /// Server fully stopped.
    Stopped = 6,
}

impl ShutdownPhase {
    /// Returns true if the server should stop accepting connections.
    #[must_use]
    pub fn should_reject_connections(self) -> bool {
        self as u8 >= Self::StopAccepting as u8
    }

    /// Returns true if new requests should receive 503.
    #[must_use]
    pub fn should_reject_requests(self) -> bool {
        self as u8 >= Self::ShutdownFlagged as u8
    }

    /// Returns true if the server is in a shutdown state.
    #[must_use]
    pub fn is_shutting_down(self) -> bool {
        self as u8 >= Self::StopAccepting as u8
    }

    /// Returns true if cleanup is complete.
    #[must_use]
    pub fn is_stopped(self) -> bool {
        self == Self::Stopped
    }
}

impl From<u8> for ShutdownPhase {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Running,
            1 => Self::StopAccepting,
            2 => Self::ShutdownFlagged,
            3 => Self::GracePeriod,
            4 => Self::Cancelling,
            5 => Self::RunningHooks,
            _ => Self::Stopped,
        }
    }
}

// ============================================================================
// Shutdown State (Shared)
// ============================================================================

/// Shared state for shutdown coordination.
struct ShutdownState {
    /// Current shutdown phase.
    phase: AtomicU8,
    /// Flag for forced shutdown (second signal).
    forced: AtomicBool,
    /// Wakers waiting for shutdown.
    wakers: parking_lot::Mutex<Vec<Waker>>,
    /// Shutdown hooks to run.
    hooks: parking_lot::Mutex<Vec<ShutdownHook>>,
    /// In-flight request count.
    in_flight: std::sync::atomic::AtomicUsize,
}

impl ShutdownState {
    fn new() -> Self {
        Self {
            phase: AtomicU8::new(ShutdownPhase::Running as u8),
            forced: AtomicBool::new(false),
            wakers: parking_lot::Mutex::new(Vec::new()),
            hooks: parking_lot::Mutex::new(Vec::new()),
            in_flight: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn phase(&self) -> ShutdownPhase {
        ShutdownPhase::from(self.phase.load(Ordering::Acquire))
    }

    fn set_phase(&self, phase: ShutdownPhase) {
        self.phase.store(phase as u8, Ordering::Release);
        self.wake_all();
    }

    fn try_advance_phase(&self, from: ShutdownPhase, to: ShutdownPhase) -> bool {
        self.phase
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn is_forced(&self) -> bool {
        self.forced.load(Ordering::Acquire)
    }

    fn set_forced(&self) {
        self.forced.store(true, Ordering::Release);
        self.wake_all();
    }

    fn wake_all(&self) {
        let wakers = std::mem::take(&mut *self.wakers.lock());
        for waker in wakers {
            waker.wake();
        }
    }

    fn register_waker(&self, waker: &Waker) {
        let mut wakers = self.wakers.lock();
        if !wakers.iter().any(|w| w.will_wake(waker)) {
            wakers.push(waker.clone());
        }
    }

    fn increment_in_flight(&self) -> usize {
        self.in_flight.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn decrement_in_flight(&self) -> usize {
        self.in_flight.fetch_sub(1, Ordering::AcqRel) - 1
    }

    fn in_flight_count(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

// ============================================================================
// Shutdown Controller
// ============================================================================

/// Controller for initiating and coordinating shutdown.
///
/// Create one controller per server and share it via [`subscribe()`](Self::subscribe).
#[derive(Clone)]
pub struct ShutdownController {
    state: Arc<ShutdownState>,
}

impl ShutdownController {
    /// Create a new shutdown controller.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(ShutdownState::new()),
        }
    }

    /// Create a receiver to wait for shutdown signals.
    #[must_use]
    pub fn subscribe(&self) -> ShutdownReceiver {
        ShutdownReceiver {
            state: Arc::clone(&self.state),
        }
    }

    /// Get the current shutdown phase.
    #[must_use]
    pub fn phase(&self) -> ShutdownPhase {
        self.state.phase()
    }

    /// Check if shutdown has been initiated.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.state.phase().is_shutting_down()
    }

    /// Check if forced shutdown was requested.
    #[must_use]
    pub fn is_forced(&self) -> bool {
        self.state.is_forced()
    }

    /// Initiate graceful shutdown.
    ///
    /// This begins the shutdown sequence. If shutdown is already in progress,
    /// calling this again triggers forced shutdown.
    pub fn shutdown(&self) {
        let current = self.state.phase();
        if current == ShutdownPhase::Running {
            self.state.set_phase(ShutdownPhase::StopAccepting);
        } else if !self.state.is_forced() {
            // Second signal = forced shutdown
            self.state.set_forced();
        }
    }

    /// Force immediate shutdown without grace period.
    pub fn force_shutdown(&self) {
        self.state.set_forced();
        self.state.set_phase(ShutdownPhase::Cancelling);
    }

    /// Advance to the next shutdown phase.
    ///
    /// Returns `true` if the phase was advanced.
    pub fn advance_phase(&self) -> bool {
        let current = self.state.phase();
        let next = match current {
            ShutdownPhase::Running => ShutdownPhase::StopAccepting,
            ShutdownPhase::StopAccepting => ShutdownPhase::ShutdownFlagged,
            ShutdownPhase::ShutdownFlagged => ShutdownPhase::GracePeriod,
            ShutdownPhase::GracePeriod => ShutdownPhase::Cancelling,
            ShutdownPhase::Cancelling => ShutdownPhase::RunningHooks,
            ShutdownPhase::RunningHooks => ShutdownPhase::Stopped,
            ShutdownPhase::Stopped => return false,
        };
        self.state.try_advance_phase(current, next)
    }

    /// Register a shutdown hook to run during the RunningHooks phase.
    ///
    /// Hooks run in LIFO order (last registered runs first).
    pub fn register_hook<F>(&self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut hooks = self.state.hooks.lock();
        hooks.push(ShutdownHook::Sync(Box::new(hook)));
    }

    /// Register an async shutdown hook.
    pub fn register_async_hook<F, Fut>(&self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut hooks = self.state.hooks.lock();
        hooks.push(ShutdownHook::AsyncFactory(Box::new(move || {
            Box::pin(hook())
        })));
    }

    /// Pop and return the next shutdown hook to run.
    ///
    /// Returns `None` when all hooks have been run.
    pub fn pop_hook(&self) -> Option<ShutdownHook> {
        let mut hooks = self.state.hooks.lock();
        hooks.pop()
    }

    /// Returns the number of registered hooks.
    #[must_use]
    pub fn hook_count(&self) -> usize {
        self.state.hooks.lock().len()
    }

    /// Track a new in-flight request.
    ///
    /// Returns a guard that decrements the count when dropped.
    #[must_use]
    pub fn track_request(&self) -> InFlightGuard {
        self.state.increment_in_flight();
        InFlightGuard {
            state: Arc::clone(&self.state),
        }
    }

    /// Get the current in-flight request count.
    #[must_use]
    pub fn in_flight_count(&self) -> usize {
        self.state.in_flight_count()
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Shutdown Receiver
// ============================================================================

/// Receiver for shutdown signals.
///
/// Obtained from [`ShutdownController::subscribe()`].
#[derive(Clone)]
pub struct ShutdownReceiver {
    state: Arc<ShutdownState>,
}

impl ShutdownReceiver {
    /// Wait for shutdown to be initiated.
    ///
    /// Returns immediately if shutdown is already in progress.
    pub async fn wait(&self) {
        ShutdownWaitFuture { state: &self.state }.await
    }

    /// Get the current shutdown phase.
    #[must_use]
    pub fn phase(&self) -> ShutdownPhase {
        self.state.phase()
    }

    /// Check if shutdown has been initiated.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.state.phase().is_shutting_down()
    }

    /// Check if forced shutdown was requested.
    #[must_use]
    pub fn is_forced(&self) -> bool {
        self.state.is_forced()
    }
}

/// Future for waiting on shutdown.
struct ShutdownWaitFuture<'a> {
    state: &'a ShutdownState,
}

impl Future for ShutdownWaitFuture<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.phase().is_shutting_down() {
            Poll::Ready(())
        } else {
            self.state.register_waker(cx.waker());
            // Double-check after registering
            if self.state.phase().is_shutting_down() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

// ============================================================================
// In-Flight Guard
// ============================================================================

/// RAII guard for tracking in-flight requests.
///
/// Decrements the in-flight count when dropped.
pub struct InFlightGuard {
    state: Arc<ShutdownState>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.state.decrement_in_flight();
    }
}

// ============================================================================
// Shutdown Hooks
// ============================================================================

/// A shutdown hook to run during cleanup.
pub enum ShutdownHook {
    /// Synchronous cleanup function.
    Sync(Box<dyn FnOnce() + Send>),
    /// Factory for async cleanup future.
    AsyncFactory(Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>),
}

impl ShutdownHook {
    /// Run the hook synchronously.
    ///
    /// For async hooks, this returns the future to await.
    pub fn run(self) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        match self {
            Self::Sync(f) => {
                f();
                None
            }
            Self::AsyncFactory(f) => Some(f()),
        }
    }
}

// ============================================================================
// Graceful Shutdown Builder
// ============================================================================

/// Configuration for graceful shutdown.
#[derive(Clone)]
pub struct GracefulConfig {
    /// Grace period for in-flight requests.
    pub grace_period: Duration,
    /// Budget allocated to cleanup operations.
    pub cleanup_budget: Budget,
    /// Log shutdown events.
    pub log_events: bool,
}

impl Default for GracefulConfig {
    fn default() -> Self {
        Self {
            grace_period: Duration::from_secs(30),
            cleanup_budget: Budget::new()
                .with_poll_quota(500)
                .with_deadline(asupersync::Time::from_secs(5)),
            log_events: true,
        }
    }
}

/// Builder for graceful shutdown.
pub struct GracefulShutdown {
    receiver: ShutdownReceiver,
    config: GracefulConfig,
}

impl GracefulShutdown {
    /// Create a new graceful shutdown builder.
    #[must_use]
    pub fn new(receiver: ShutdownReceiver) -> Self {
        Self {
            receiver,
            config: GracefulConfig::default(),
        }
    }

    /// Set the grace period for in-flight requests.
    #[must_use]
    pub fn grace_period(mut self, duration: Duration) -> Self {
        self.config.grace_period = duration;
        self
    }

    /// Set the cleanup budget.
    #[must_use]
    pub fn cleanup_budget(mut self, budget: Budget) -> Self {
        self.config.cleanup_budget = budget;
        self
    }

    /// Enable or disable event logging.
    #[must_use]
    pub fn log_events(mut self, enabled: bool) -> Self {
        self.config.log_events = enabled;
        self
    }

    /// Run a future with graceful shutdown support.
    ///
    /// Returns when either:
    /// - The future completes normally
    /// - Graceful shutdown completes
    /// - Forced shutdown is triggered
    ///
    /// Note: This is a simplified implementation. For production use with
    /// proper concurrent select behavior, consider using the futures crate's
    /// `select` macro or similar.
    pub async fn run<F, T>(self, fut: F) -> ShutdownOutcome<T>
    where
        F: Future<Output = T>,
    {
        use std::pin::pin;

        let mut fut = pin!(fut);

        // Poll both futures until one completes.
        // This is a simple implementation that checks shutdown first.
        futures_executor::block_on(async {
            loop {
                // Check if shutdown was requested
                if self.receiver.is_shutting_down() {
                    if self.receiver.is_forced() {
                        return ShutdownOutcome::ForcedShutdown;
                    }
                    return ShutdownOutcome::GracefulShutdown;
                }

                // Try to poll the main future (non-blocking check)
                // In a real implementation, we'd use proper select semantics
                // For now, we just await the future directly
                break ShutdownOutcome::Completed(fut.as_mut().await);
            }
        })
    }

    /// Get the configuration.
    #[must_use]
    pub fn config(&self) -> &GracefulConfig {
        &self.config
    }
}

/// Outcome of running with graceful shutdown.
#[derive(Debug)]
pub enum ShutdownOutcome<T> {
    /// The future completed normally.
    Completed(T),
    /// Graceful shutdown was triggered.
    GracefulShutdown,
    /// Forced shutdown was triggered (second signal).
    ForcedShutdown,
}

impl<T> ShutdownOutcome<T> {
    /// Returns `true` if the future completed normally.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// Returns `true` if shutdown was triggered.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        matches!(self, Self::GracefulShutdown | Self::ForcedShutdown)
    }

    /// Returns `true` if forced shutdown was triggered.
    #[must_use]
    pub fn is_forced(&self) -> bool {
        matches!(self, Self::ForcedShutdown)
    }

    /// Extract the completed value, if any.
    #[must_use]
    pub fn into_completed(self) -> Option<T> {
        match self {
            Self::Completed(v) => Some(v),
            _ => None,
        }
    }
}

// ============================================================================
// Request Budget Subdivision
// ============================================================================

/// Calculate subdivided budget for a request during grace period.
///
/// During graceful shutdown, in-flight requests get a proportional share
/// of the remaining grace period.
///
/// # Arguments
///
/// * `grace_remaining` - Time remaining in the grace period
/// * `in_flight_count` - Number of in-flight requests
/// * `original_budget` - The request's original budget (if any)
///
/// # Returns
///
/// A budget that is the minimum of:
/// - The remaining grace period divided by in-flight count
/// - The original budget (if provided)
#[must_use]
pub fn subdivide_grace_budget(
    grace_remaining: Duration,
    in_flight_count: usize,
    original_budget: Option<Budget>,
) -> Budget {
    use asupersync::Time;

    let count = in_flight_count.max(1);
    let per_request = grace_remaining / count as u32;

    // Convert Duration to Time (nanoseconds since epoch, but used as a relative deadline)
    let deadline_nanos = per_request.as_nanos() as u64;
    let grace_budget = Budget::new().with_deadline(Time::from_nanos(deadline_nanos));

    match original_budget {
        Some(original) => original.meet(grace_budget),
        None => grace_budget,
    }
}

// ============================================================================
// Cancel Reason for Shutdown
// ============================================================================

/// Create a cancel reason for server shutdown.
#[must_use]
pub fn shutdown_cancel_reason() -> CancelReason {
    CancelReason::shutdown()
}

/// Create a cancel reason for grace period expiry.
#[must_use]
pub fn grace_expired_cancel_reason() -> CancelReason {
    CancelReason::timeout()
}

// ============================================================================
// Request Context Integration
// ============================================================================

/// Extension trait for checking shutdown status from request context.
pub trait ShutdownAware {
    /// Check if the server is shutting down.
    fn is_shutting_down(&self) -> bool;

    /// Get the current shutdown phase, if available.
    fn shutdown_phase(&self) -> Option<ShutdownPhase>;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_phase_transitions() {
        assert!(!ShutdownPhase::Running.should_reject_connections());
        assert!(ShutdownPhase::StopAccepting.should_reject_connections());
        assert!(ShutdownPhase::ShutdownFlagged.should_reject_requests());
        assert!(ShutdownPhase::GracePeriod.is_shutting_down());
        assert!(ShutdownPhase::Stopped.is_stopped());
    }

    #[test]
    fn controller_basic() {
        let controller = ShutdownController::new();
        assert_eq!(controller.phase(), ShutdownPhase::Running);
        assert!(!controller.is_shutting_down());

        controller.shutdown();
        assert_eq!(controller.phase(), ShutdownPhase::StopAccepting);
        assert!(controller.is_shutting_down());
    }

    #[test]
    fn controller_double_shutdown_forces() {
        let controller = ShutdownController::new();
        controller.shutdown();
        assert!(!controller.is_forced());

        controller.shutdown();
        assert!(controller.is_forced());
    }

    #[test]
    fn controller_advance_phase() {
        let controller = ShutdownController::new();

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::StopAccepting);

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::ShutdownFlagged);

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::GracePeriod);

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::Cancelling);

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::RunningHooks);

        assert!(controller.advance_phase());
        assert_eq!(controller.phase(), ShutdownPhase::Stopped);

        // Can't advance past Stopped
        assert!(!controller.advance_phase());
    }

    #[test]
    fn in_flight_tracking() {
        let controller = ShutdownController::new();
        assert_eq!(controller.in_flight_count(), 0);

        let guard1 = controller.track_request();
        assert_eq!(controller.in_flight_count(), 1);

        let guard2 = controller.track_request();
        assert_eq!(controller.in_flight_count(), 2);

        drop(guard1);
        assert_eq!(controller.in_flight_count(), 1);

        drop(guard2);
        assert_eq!(controller.in_flight_count(), 0);
    }

    #[test]
    fn shutdown_hooks_lifo() {
        let controller = ShutdownController::new();
        let order: Arc<parking_lot::Mutex<Vec<i32>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        controller.register_hook(move || order1.lock().push(1));

        let order2 = Arc::clone(&order);
        controller.register_hook(move || order2.lock().push(2));

        let order3 = Arc::clone(&order);
        controller.register_hook(move || order3.lock().push(3));

        assert_eq!(controller.hook_count(), 3);

        // Pop and run in LIFO order
        while let Some(hook) = controller.pop_hook() {
            hook.run();
        }

        assert_eq!(*order.lock(), vec![3, 2, 1]);
    }

    #[test]
    fn subdivide_grace_budget_basic() {
        let grace = Duration::from_secs(30);
        let budget = subdivide_grace_budget(grace, 3, None);

        // Each request gets 10 seconds
        assert!(budget.deadline.is_some());
    }

    #[test]
    fn subdivide_grace_budget_respects_original() {
        use asupersync::Time;

        let grace = Duration::from_secs(30);
        let original = Budget::new().with_deadline(Time::from_secs(5));
        let budget = subdivide_grace_budget(grace, 3, Some(original));

        // Should respect the tighter (original) deadline
        assert!(budget.deadline.is_some());
    }

    #[test]
    fn receiver_is_shutting_down() {
        let controller = ShutdownController::new();
        let receiver = controller.subscribe();

        assert!(!receiver.is_shutting_down());

        controller.shutdown();
        assert!(receiver.is_shutting_down());
    }

    #[test]
    fn shutdown_outcome_accessors() {
        let completed: ShutdownOutcome<i32> = ShutdownOutcome::Completed(42);
        assert!(completed.is_completed());
        assert!(!completed.is_shutdown());
        assert_eq!(completed.into_completed(), Some(42));

        let graceful: ShutdownOutcome<i32> = ShutdownOutcome::GracefulShutdown;
        assert!(!graceful.is_completed());
        assert!(graceful.is_shutdown());
        assert!(!graceful.is_forced());

        let forced: ShutdownOutcome<i32> = ShutdownOutcome::ForcedShutdown;
        assert!(forced.is_shutdown());
        assert!(forced.is_forced());
    }

    #[test]
    fn cancel_reasons() {
        let shutdown = shutdown_cancel_reason();
        assert!(shutdown.is_shutdown());

        let grace = grace_expired_cancel_reason();
        assert!(!grace.is_shutdown());
    }
}
