//! Decides when the engine's vsync baton is returned.
//!
//! The engine asks for a vsync whenever it wants to start a frame. The
//! answer comes from the compositor when it has one to give: the
//! `wl_surface.frame` callback requested by the commit of the previous frame.
//! That callback is requested by the raster thread together with the buffer
//! (see `SctkFlutterWindowInner::present_frame`), never by a commit of its
//! own, so asking for a vsync never changes what the compositor shows.
//!
//! When no such callback is on its way (nothing has been committed since the
//! last one, or ever), or it does not arrive (the compositor sends none for a
//! surface that is not visible), the baton is returned from a timer instead.
//!
//! A frame callback is also not taken at its word when it comes much too
//! soon after the previous vsync. Sway sends one right after every commit
//! of a window whose resize it is holding back, which would have the engine
//! draw hundreds of frames a second.

use std::sync::Arc;

use parking_lot::Mutex;
use smithay_client_toolkit::reexports::calloop::LoopSignal;

/// How long a requested frame callback is waited for before the timer takes
/// over. A surface that is not shown gets no callbacks, so this is also the
/// pace of a hidden window.
const FRAME_CALLBACK_TIMEOUT_NANOS: u64 = 100_000_000;

/// Times are in nanoseconds, on the engine's clock
/// (`FlutterEngineGetCurrentTime`).
#[derive(Debug, Default)]
pub(crate) struct FrameClockState {
    /// The engine's pending vsync request.
    baton: Option<isize>,
    /// When the frame callback that has not fired yet was requested.
    callback_requested_at: Option<u64>,
    /// When the last baton was returned or the last frame callback fired.
    last_vsync: Option<u64>,
    /// A frame callback fired and `poll` has not run since.
    refreshed: bool,
}

impl FrameClockState {
    pub(crate) fn request_vsync(&mut self, baton: isize) {
        self.baton = Some(baton);
    }

    /// Whether the engine is waiting for a vsync.
    pub(crate) fn is_vsync_requested(&self) -> bool {
        self.baton.is_some()
    }

    /// A `wl_surface.frame` callback was requested, for the commit that
    /// follows.
    pub(crate) fn frame_callback_requested(&mut self, now: u64) {
        self.callback_requested_at = Some(now);
    }

    /// A frame callback fired. `poll` returns the baton, if the engine is
    /// waiting for one: the caller runs it once it has handled everything
    /// else the compositor sent along with the callback (a configure, for
    /// one, should reach the engine before it starts its next frame).
    pub(crate) fn frame_callback_done(&mut self, now: u64, frame_interval: u64) {
        self.callback_requested_at = None;

        // Too soon to be the output's next refresh: `poll` hands the baton
        // back when that is due.
        let earliest = self
            .last_vsync
            .map_or(0, |last| last + frame_interval - frame_interval / 4);
        if now < earliest {
            return;
        }

        self.last_vsync = Some(now);
        self.refreshed = true;
    }

    /// Returns the baton to hand back now, if any, and otherwise when to
    /// call this again.
    pub(crate) fn poll(&mut self, now: u64, frame_interval: u64) -> (Option<isize>, Option<u64>) {
        let refreshed = std::mem::take(&mut self.refreshed);

        if self.baton.is_none() {
            return (None, None);
        }

        if refreshed {
            return (self.baton.take(), None);
        }

        if let Some(requested_at) = self.callback_requested_at {
            let deadline = requested_at + FRAME_CALLBACK_TIMEOUT_NANOS;
            if now < deadline {
                return (None, Some(deadline));
            }
        }

        let due = self.last_vsync.map_or(now, |last| last + frame_interval);
        if now < due {
            return (None, Some(due));
        }

        self.last_vsync = Some(now);
        (self.baton.take(), None)
    }
}

/// The frame clock of a window, shared by the platform thread (which also
/// runs Dart, and asks for vsyncs) and the raster thread.
#[derive(Clone)]
pub(crate) struct FrameClock {
    state: Arc<Mutex<FrameClockState>>,
    signal: LoopSignal,
}

impl FrameClock {
    pub(crate) fn new(signal: LoopSignal) -> Self {
        Self {
            state: Default::default(),
            signal,
        }
    }

    /// Called on the UI thread, which is the platform thread, possibly from
    /// within a call into the engine: the lock is not held anywhere else
    /// on this thread while calling into the engine.
    pub(crate) fn request_vsync(&self, baton: isize) {
        self.state.lock().request_vsync(baton);
        self.wake_platform_thread();
    }

    /// Called on the platform thread.
    pub(crate) fn is_vsync_requested(&self) -> bool {
        self.state.lock().is_vsync_requested()
    }

    /// Called on the raster thread.
    pub(crate) fn frame_callback_requested(&self, now: u64) {
        self.state.lock().frame_callback_requested(now);
    }

    /// Called on the platform thread.
    pub(crate) fn frame_callback_done(&self, now: u64, frame_interval: u64) {
        self.state.lock().frame_callback_done(now, frame_interval);
    }

    /// Called on the platform thread.
    pub(crate) fn poll(&self, now: u64, frame_interval: u64) -> (Option<isize>, Option<u64>) {
        self.state.lock().poll(now, frame_interval)
    }

    /// Makes the platform thread's event loop run another iteration, which
    /// polls this clock and the windows.
    pub(crate) fn wake_platform_thread(&self) {
        self.signal.wakeup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    const INTERVAL: u64 = 16 * MS;

    #[test]
    fn nothing_to_do_without_a_request() {
        let mut clock = FrameClockState::default();
        assert_eq!(clock.poll(0, INTERVAL), (None, None));
        clock.frame_callback_done(5 * MS, INTERVAL);
        assert_eq!(clock.poll(5 * MS, INTERVAL), (None, None));

        // That refresh has passed: a request made later waits for the next.
        clock.request_vsync(1);
        assert_eq!(clock.poll(6 * MS, INTERVAL), (None, Some(21 * MS)));
    }

    #[test]
    fn first_request_is_answered_at_once() {
        // Nothing has been committed yet, so no frame callback can come.
        let mut clock = FrameClockState::default();
        clock.request_vsync(1);
        assert_eq!(clock.poll(3 * MS, INTERVAL), (Some(1), None));
        assert_eq!(clock.poll(4 * MS, INTERVAL), (None, None));
    }

    #[test]
    fn request_waits_for_the_frame_callback() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_requested(0);
        clock.request_vsync(1);
        assert_eq!(
            clock.poll(MS, INTERVAL),
            (None, Some(FRAME_CALLBACK_TIMEOUT_NANOS))
        );
        clock.frame_callback_done(10 * MS, INTERVAL);
        assert_eq!(clock.poll(10 * MS, INTERVAL), (Some(1), None));
        assert_eq!(clock.poll(11 * MS, INTERVAL), (None, None));
    }

    #[test]
    fn request_without_a_callback_waits_for_the_next_interval() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_requested(0);
        clock.frame_callback_done(10 * MS, INTERVAL);
        assert_eq!(clock.poll(10 * MS, INTERVAL).0, None);

        // The engine asks for the next frame before the current one has
        // been committed.
        clock.request_vsync(1);
        assert_eq!(clock.poll(12 * MS, INTERVAL), (None, Some(26 * MS)));
        assert_eq!(clock.poll(26 * MS, INTERVAL), (Some(1), None));
    }

    #[test]
    fn a_commit_before_the_timer_is_due_hands_over_to_the_callback() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_done(10 * MS, INTERVAL);
        assert_eq!(clock.poll(10 * MS, INTERVAL).0, None);
        clock.request_vsync(1);
        assert_eq!(clock.poll(12 * MS, INTERVAL), (None, Some(26 * MS)));

        clock.frame_callback_requested(18 * MS);
        assert_eq!(
            clock.poll(26 * MS, INTERVAL),
            (None, Some(18 * MS + FRAME_CALLBACK_TIMEOUT_NANOS))
        );
        clock.frame_callback_done(27 * MS, INTERVAL);
        assert_eq!(clock.poll(27 * MS, INTERVAL), (Some(1), None));
    }

    #[test]
    fn early_frame_callbacks_do_not_speed_the_clock_up() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_done(100 * MS, INTERVAL);
        assert_eq!(clock.poll(100 * MS, INTERVAL).0, None);

        // A callback right after every commit.
        clock.request_vsync(1);
        clock.frame_callback_requested(101 * MS);
        clock.frame_callback_done(102 * MS, INTERVAL);
        assert_eq!(clock.poll(102 * MS, INTERVAL).0, None);
        assert_eq!(clock.poll(102 * MS, INTERVAL), (None, Some(116 * MS)));
        assert_eq!(clock.poll(116 * MS, INTERVAL), (Some(1), None));

        clock.request_vsync(2);
        clock.frame_callback_requested(117 * MS);
        clock.frame_callback_done(118 * MS, INTERVAL);
        assert_eq!(clock.poll(118 * MS, INTERVAL).0, None);
        assert_eq!(clock.poll(118 * MS, INTERVAL), (None, Some(132 * MS)));

        // One that is a little early is the real thing.
        clock.frame_callback_requested(119 * MS);
        clock.frame_callback_done(130 * MS, INTERVAL);
        assert_eq!(clock.poll(130 * MS, INTERVAL), (Some(2), None));
    }

    #[test]
    fn idle_request_is_answered_at_once() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_done(10 * MS, INTERVAL);
        assert_eq!(clock.poll(10 * MS, INTERVAL).0, None);
        clock.request_vsync(1);
        assert_eq!(clock.poll(500 * MS, INTERVAL), (Some(1), None));
    }

    #[test]
    fn hidden_surface_falls_back_to_the_timer() {
        let mut clock = FrameClockState::default();
        clock.frame_callback_requested(0);
        clock.request_vsync(1);
        assert_eq!(clock.poll(50 * MS, INTERVAL), (None, Some(100 * MS)));
        assert_eq!(clock.poll(100 * MS, INTERVAL), (Some(1), None));

        // The frame it drew asks for another callback, which does not come
        // either.
        clock.frame_callback_requested(105 * MS);
        clock.request_vsync(2);
        assert_eq!(clock.poll(110 * MS, INTERVAL), (None, Some(205 * MS)));
        assert_eq!(clock.poll(205 * MS, INTERVAL), (Some(2), None));

        // Shown again: the callbacks fire.
        clock.frame_callback_requested(210 * MS);
        clock.request_vsync(3);
        clock.frame_callback_done(225 * MS, INTERVAL);
        assert_eq!(clock.poll(225 * MS, INTERVAL), (Some(3), None));
    }
}
