//! Keeps what a window's surface shows in step with what the compositor asked
//! for.
//!
//! The compositor configures the window on the platform thread. The frame
//! drawn for that configure is presented by the raster thread, some time
//! later. `SurfaceSync` is the state the two share, and it upholds one rule:
//! a configure is acknowledged in a `wl_surface.commit` whose buffer was
//! drawn for it, and never before.
//!
//! To get there, only one size is asked of the engine at a time (the
//! *request in flight*). Configures that arrive in the meantime replace each
//! other, and the last one is sent when the frame for the request in flight
//! has been committed. Without that, the engine throws away every frame
//! whose size is not the last one it was told about, and a window that is
//! being resized continuously never gets to draw.
//!
//! Configures are acknowledged one by one, each with a commit of its own,
//! and not only the last of them, although xdg-shell lets a client do that
//! (acknowledging a configure answers the ones before it as well). Sway
//! holds a resize back until a commit acknowledges exactly the configure it
//! sent for it, or 200 ms have passed, and it shows the window's old content
//! in the meantime. Only configures for a size that was replaced before it
//! could be drawn are skipped.
//!
//! Background: <https://forge.lab.vially.dev/vially/flust/issues/1>

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use dpi::PhysicalSize;

/// How long the frame for a request is waited for before a newer size may
/// replace it. The engine drops frames without telling anyone, so a request
/// can not be waited on forever.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(100);

/// How many configures are remembered while none of them can be
/// acknowledged. More than that only come in if the engine has stopped
/// drawing, and then the oldest are of no use any more.
const MAX_UNACKNOWLEDGED: usize = 64;

/// What a buffer is drawn for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceGeometry {
    /// The size of the buffer.
    pub(crate) size: PhysicalSize<u32>,
    /// The buffer scale: `size` is the surface's size times this.
    pub(crate) scale: u32,
}

/// What the platform thread has to do, from `SurfaceSync::poll`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlatformAction {
    None,
    /// Ask the engine for frames of this geometry (send window metrics).
    Request(SurfaceGeometry),
    /// Acknowledge this configure and commit the surface as it is: what it
    /// shows is already what the configure asked for.
    Acknowledge(u32),
}

/// What the raster thread has to do with a frame, from `SurfaceSync::frame`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameAction {
    /// The frame answers the request in flight. Give the surface this
    /// geometry, acknowledge the configure if there is one, and commit all
    /// of that together with the frame.
    Commit {
        geometry: SurfaceGeometry,
        acknowledge: Option<u32>,
    },
    /// The frame is for the geometry the surface already has. Commit it.
    Present,
    /// The frame is for a geometry the surface does not have and was not
    /// asked to get. Do not show it.
    Drop,
}

#[derive(Clone, Copy, Debug)]
struct Request {
    geometry: SurfaceGeometry,
    /// When a newer geometry may replace this request. `None` once that
    /// time has passed without a newer one.
    deadline: Option<Instant>,
}

#[derive(Debug, Default)]
pub(crate) struct SurfaceSync {
    /// The geometry the surface should end up with.
    wanted: Option<SurfaceGeometry>,
    /// The configures that have not been acknowledged, oldest first, each
    /// with the geometry it asked for.
    unacknowledged: VecDeque<(u32, SurfaceGeometry)>,
    /// The geometry the engine was last asked for, until a frame of it is
    /// committed.
    in_flight: Option<Request>,
    /// The geometry of the buffer that was committed last.
    committed: Option<SurfaceGeometry>,
}

impl SurfaceSync {
    /// The compositor configured the window. Called on the platform thread.
    pub(crate) fn configure(&mut self, serial: u32, geometry: SurfaceGeometry) {
        self.wanted = Some(geometry);

        if self.unacknowledged.len() == MAX_UNACKNOWLEDGED {
            self.unacknowledged.pop_front();
        }
        self.unacknowledged.push_back((serial, geometry));
    }

    /// Takes a configure that asked for `geometry` off the list, together
    /// with the ones before it: acknowledging it answers those too.
    ///
    /// Of several in a row for that geometry it is the first, so that the
    /// others still get a commit of their own. Earlier ones, from before the
    /// window was asked for something else in between, are passed over.
    fn take_configure_for(&mut self, geometry: SurfaceGeometry) -> Option<u32> {
        let asks_for = |(_, configured): &(u32, SurfaceGeometry)| *configured == geometry;

        let last = self.unacknowledged.iter().rposition(asks_for)?;
        let before_run = self
            .unacknowledged
            .iter()
            .take(last)
            .rposition(|c| !asks_for(c));
        let first = before_run.map_or(0, |index| index + 1);

        self.unacknowledged
            .drain(..=first)
            .next_back()
            .map(|(serial, _)| serial)
    }

    /// The geometry changed for another reason than a configure (the scale
    /// did). Called on the platform thread.
    pub(crate) fn set_geometry(&mut self, geometry: SurfaceGeometry) {
        self.wanted = Some(geometry);
    }

    /// The geometry the engine draws for at the moment: the last one it was
    /// asked for.
    pub(crate) fn engine_geometry(&self) -> Option<SurfaceGeometry> {
        self.in_flight
            .map(|request| request.geometry)
            .or(self.committed)
    }

    /// Returns what to do next, and when to call this again if nothing else
    /// happens before. Called on the platform thread: after a configure,
    /// after a frame was committed, and when the returned time has come.
    /// After any action but `PlatformAction::None`, call it again.
    ///
    /// `may_request` says whether this is a good moment to ask the engine
    /// for another size: it throws away the frame it is working on when the
    /// size changes under it. When it is not, call this again when it is.
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        may_request: bool,
    ) -> (PlatformAction, Option<Instant>) {
        // The oldest configure asks for what the surface already shows.
        if let Some(&(serial, geometry)) = self.unacknowledged.front() {
            if self.committed == Some(geometry) {
                self.unacknowledged.pop_front();
                return (PlatformAction::Acknowledge(serial), None);
            }
        }

        if let Some(request) = self.in_flight.as_mut() {
            if request.deadline.is_some_and(|deadline| now < deadline) {
                return (PlatformAction::None, request.deadline);
            }

            // The frame is late. Keep waiting for it, unless there is
            // something newer to ask for.
            request.deadline = None;
            if self.wanted == Some(request.geometry) {
                return (PlatformAction::None, None);
            }
        }

        let Some(wanted) = self.wanted else {
            return (PlatformAction::None, None);
        };

        if self.committed == Some(wanted) {
            // The surface shows what is wanted. Configures for sizes it
            // passed over on the way there need no answer of their own.
            self.in_flight = None;
            return match self.take_configure_for(wanted) {
                Some(serial) => (PlatformAction::Acknowledge(serial), None),
                None => (PlatformAction::None, None),
            };
        }

        if !may_request {
            return (PlatformAction::None, None);
        }

        let deadline = now + REQUEST_TIMEOUT;
        self.in_flight = Some(Request {
            geometry: wanted,
            deadline: Some(deadline),
        });
        (PlatformAction::Request(wanted), Some(deadline))
    }

    /// The engine has a frame of this size to present (`None` for a frame
    /// without content, which has no size). Called on the raster thread,
    /// which then does what is returned before it lets go of the lock this
    /// state is under.
    pub(crate) fn frame(&mut self, size: Option<PhysicalSize<u32>>) -> FrameAction {
        let fits = |geometry: SurfaceGeometry| size.is_none_or(|size| size == geometry.size);

        if let Some(request) = self.in_flight.filter(|request| fits(request.geometry)) {
            let acknowledge = self.take_configure_for(request.geometry);

            self.in_flight = None;
            self.committed = Some(request.geometry);

            return FrameAction::Commit {
                geometry: request.geometry,
                acknowledge,
            };
        }

        if self.committed.is_some_and(fits) {
            return FrameAction::Present;
        }

        FrameAction::Drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(width: u32, height: u32) -> SurfaceGeometry {
        SurfaceGeometry {
            size: PhysicalSize::new(width, height),
            scale: 1,
        }
    }

    fn size(width: u32, height: u32) -> Option<PhysicalSize<u32>> {
        Some(PhysicalSize::new(width, height))
    }

    const A: (u32, u32) = (640, 480);
    const B: (u32, u32) = (700, 500);
    const C: (u32, u32) = (800, 600);

    fn g(size: (u32, u32)) -> SurfaceGeometry {
        geometry(size.0, size.1)
    }

    fn s(size: (u32, u32)) -> Option<PhysicalSize<u32>> {
        self::size(size.0, size.1)
    }

    /// A window that shows a frame of size A, for configure 1.
    fn settled(now: Instant) -> SurfaceSync {
        let mut sync = SurfaceSync::default();
        sync.configure(1, g(A));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(A)));
        assert_eq!(
            sync.frame(s(A)),
            FrameAction::Commit {
                geometry: g(A),
                acknowledge: Some(1)
            }
        );
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
        sync
    }

    #[test]
    fn nothing_is_shown_before_the_first_configure() {
        let now = Instant::now();
        let mut sync = SurfaceSync::default();
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
        assert_eq!(sync.frame(s(A)), FrameAction::Drop);
        assert_eq!(sync.frame(None), FrameAction::Drop);
        assert_eq!(sync.engine_geometry(), None);
    }

    #[test]
    fn first_configure_is_acknowledged_with_its_frame() {
        let now = Instant::now();
        let mut sync = SurfaceSync::default();
        sync.configure(1, g(A));
        assert_eq!(
            sync.poll(now, true),
            (PlatformAction::Request(g(A)), Some(now + REQUEST_TIMEOUT))
        );
        assert_eq!(sync.engine_geometry(), Some(g(A)));

        // A frame of another size is not shown.
        assert_eq!(sync.frame(s(B)), FrameAction::Drop);
        assert_eq!(
            sync.frame(s(A)),
            FrameAction::Commit {
                geometry: g(A),
                acknowledge: Some(1)
            }
        );

        // Later frames of that size are presented as they come.
        assert_eq!(sync.frame(s(A)), FrameAction::Present);
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
    }

    #[test]
    fn configures_during_a_request_replace_each_other() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));

        sync.configure(3, g(C));
        sync.configure(4, g(A));
        sync.configure(5, g(C));
        assert_eq!(sync.poll(now, true).0, PlatformAction::None);

        // The frame for B answers configure 2, not the later ones.
        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );

        // Only the last of the others is asked for.
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(C)));
        assert_eq!(
            sync.frame(s(C)),
            FrameAction::Commit {
                geometry: g(C),
                acknowledge: Some(5)
            }
        );
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
    }

    #[test]
    fn frames_of_the_committed_size_are_presented_during_a_request() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));

        // The engine had this one under way already. It does not answer
        // the configure.
        assert_eq!(sync.frame(s(A)), FrameAction::Present);
        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );
        // And this one is too late.
        assert_eq!(sync.frame(s(A)), FrameAction::Drop);
    }

    #[test]
    fn configure_for_the_committed_size_needs_no_frame() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(A));
        assert_eq!(sync.poll(now, true), (PlatformAction::Acknowledge(2), None));
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
    }

    #[test]
    fn every_configure_for_a_size_is_acknowledged_in_turn() {
        // What sway sends when an interactive resize ends: the last size
        // again, three times, with other window states.
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        sync.configure(3, g(B));
        sync.configure(4, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::None);

        // The frame goes with the first of them, which is the one a
        // compositor that waits for a particular configure waits for.
        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );
        assert_eq!(sync.poll(now, true), (PlatformAction::Acknowledge(3), None));
        assert_eq!(sync.poll(now, true), (PlatformAction::Acknowledge(4), None));
        assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
    }

    #[test]
    fn configure_for_the_committed_size_is_acknowledged_during_a_request() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(A));
        sync.configure(3, g(B));
        assert_eq!(sync.poll(now, true), (PlatformAction::Acknowledge(2), None));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(3)
            }
        );
    }

    #[test]
    fn unanswerable_configures_are_forgotten() {
        let now = Instant::now();
        let mut sync = settled(now);

        for serial in 2..(2 * MAX_UNACKNOWLEDGED as u32) {
            sync.configure(serial, g(B));
        }
        assert_eq!(sync.unacknowledged.len(), MAX_UNACKNOWLEDGED);
    }

    #[test]
    fn late_frame_gives_way_to_a_newer_size() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        sync.configure(3, g(C));

        let before = now + REQUEST_TIMEOUT - Duration::from_millis(1);
        assert_eq!(
            sync.poll(before, true),
            (PlatformAction::None, Some(now + REQUEST_TIMEOUT))
        );

        let after = now + REQUEST_TIMEOUT;
        assert_eq!(
            sync.poll(after, true),
            (PlatformAction::Request(g(C)), Some(after + REQUEST_TIMEOUT))
        );

        // The frame for B, should it still come, is not what was asked for
        // any more.
        assert_eq!(sync.frame(s(B)), FrameAction::Drop);
        assert_eq!(
            sync.frame(s(C)),
            FrameAction::Commit {
                geometry: g(C),
                acknowledge: Some(3)
            }
        );
    }

    #[test]
    fn late_frame_is_waited_for_when_nothing_is_newer() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));

        let later = now + REQUEST_TIMEOUT * 3;
        assert_eq!(sync.poll(later, true), (PlatformAction::None, None));

        // A newer size does not have to wait any longer.
        sync.configure(3, g(C));
        assert_eq!(sync.poll(later, true).0, PlatformAction::Request(g(C)));
    }

    #[test]
    fn resize_back_during_a_request() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        sync.configure(3, g(A));

        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(A)));
        assert_eq!(
            sync.frame(s(A)),
            FrameAction::Commit {
                geometry: g(A),
                acknowledge: Some(3)
            }
        );
    }

    #[test]
    fn resize_back_after_a_late_frame_is_acknowledged_without_one() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        sync.configure(3, g(A));

        let after = now + REQUEST_TIMEOUT;
        assert_eq!(
            sync.poll(after, true),
            (PlatformAction::Acknowledge(3), None)
        );
        assert_eq!(sync.frame(s(B)), FrameAction::Drop);
        assert_eq!(sync.frame(s(A)), FrameAction::Present);
    }

    #[test]
    fn request_waits_for_a_good_moment() {
        let now = Instant::now();
        let mut sync = settled(now);

        sync.configure(2, g(A));
        sync.configure(3, g(B));

        // Acknowledging does not have to wait.
        assert_eq!(
            sync.poll(now, false),
            (PlatformAction::Acknowledge(2), None)
        );
        assert_eq!(sync.poll(now, false), (PlatformAction::None, None));
        assert_eq!(sync.engine_geometry(), Some(g(A)));

        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
    }

    #[test]
    fn scale_change_is_committed_with_its_frame() {
        let now = Instant::now();
        let mut sync = settled(now);

        let scaled = SurfaceGeometry {
            size: PhysicalSize::new(1280, 960),
            scale: 2,
        };
        sync.set_geometry(scaled);
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(scaled));
        assert_eq!(sync.frame(s(A)), FrameAction::Present);
        assert_eq!(
            sync.frame(size(1280, 960)),
            FrameAction::Commit {
                geometry: scaled,
                acknowledge: None
            }
        );
    }

    #[test]
    fn empty_frame_answers_the_request() {
        let now = Instant::now();
        let mut sync = settled(now);
        assert_eq!(sync.frame(None), FrameAction::Present);

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        assert_eq!(
            sync.frame(None),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );
    }
}
