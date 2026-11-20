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
//!
//! A window that is sized to its content turns this around: the engine
//! decides the size, and the surface follows. See `ContentSync`.

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
    /// Give the window this geometry, and wait for the compositor to
    /// configure it. Only for a window sized to its content.
    Resize(SurfaceGeometry),
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
    /// The frame is not shown: the surface has to get its size first. Wake
    /// the platform thread for that. Only for a window sized to its
    /// content.
    Resize,
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

    /// Whether a frame of this size would be shown if it were presented now.
    /// Called on the raster thread, before the engine draws a frame that
    /// can not be dropped once it is drawn (one drawn straight into the
    /// window's framebuffer).
    ///
    /// The answer holds for as long as a request is waited for: a frame
    /// that is admitted for the request in flight gives the request that
    /// time again.
    pub(crate) fn admit(&mut self, size: PhysicalSize<u32>, now: Instant) -> bool {
        if let Some(request) = self.in_flight.as_mut() {
            if request.geometry.size == size {
                request.deadline = Some(now + REQUEST_TIMEOUT);
                return true;
            }
        }

        self.committed.is_some_and(|geometry| geometry.size == size)
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

/// The surface state of a window that is sized to its content: the engine
/// draws at the size it lays the content out at, within the constraints it
/// is given, and the window takes the size of its frames.
///
/// A frame of a size the window does not have is not shown: the window is
/// given that size first (`PlatformAction::Resize`), and once the
/// compositor has configured it for it, the engine is asked for a frame
/// again (`PlatformAction::Request`, which makes it draw one). That frame
/// is committed with the acknowledgement of the configure.
///
/// The size in a configure is not imposed on the content: the window keeps
/// the size of its content. A configure that only moves the window is
/// acknowledged without a new frame.
#[derive(Debug)]
pub(crate) struct ContentSync {
    /// The buffer scale frames are drawn at.
    scale: u32,
    /// The size the window was last given, in buffer pixels.
    placed: PhysicalSize<u32>,
    /// The size of the engine's last frame.
    content: Option<PhysicalSize<u32>>,
    /// The last configure that has not been acknowledged. Acknowledging it
    /// answers the ones before it.
    unacknowledged: Option<u32>,
    /// Whether the engine has to be told again what to draw: the scale or
    /// the constraints changed.
    needs_frame: bool,
    /// When the frame asked for may be asked for again, while it has not
    /// come.
    deadline: Option<Instant>,
    /// The geometry of the buffer that was committed last.
    committed: Option<SurfaceGeometry>,
}

impl ContentSync {
    /// A window that has been given `size` (in buffer pixels) to start
    /// with: the engine's first frame decides the size it gets next.
    pub(crate) fn new(size: PhysicalSize<u32>, scale: u32) -> Self {
        Self {
            scale,
            placed: size,
            content: None,
            unacknowledged: None,
            needs_frame: true,
            deadline: None,
            committed: None,
        }
    }

    fn geometry(&self, size: PhysicalSize<u32>) -> SurfaceGeometry {
        SurfaceGeometry {
            size,
            scale: self.scale,
        }
    }

    /// The compositor configured the window. Called on the platform thread.
    pub(crate) fn configure(&mut self, serial: u32) {
        self.unacknowledged = Some(serial);
    }

    /// The buffer scale changed. Called on the platform thread.
    pub(crate) fn set_scale(&mut self, scale: u32) {
        if self.scale != scale {
            self.scale = scale;
            self.needs_frame = true;
        }
    }

    /// The engine has to be told what to draw again, as its constraints
    /// changed. Called on the platform thread.
    pub(crate) fn request_frame(&mut self) {
        self.needs_frame = true;
    }

    /// The scale the engine draws at, with the size it drew at last.
    pub(crate) fn engine_geometry(&self) -> SurfaceGeometry {
        self.geometry(self.content.unwrap_or(self.placed))
    }

    /// See `SurfaceSync::poll`.
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        may_request: bool,
    ) -> (PlatformAction, Option<Instant>) {
        // The engine drew at a size the window does not have.
        if let Some(content) = self.content.filter(|&content| content != self.placed) {
            self.placed = content;
            // The frame is asked for again once the compositor configured
            // the window for its size.
            self.deadline = None;
            return (PlatformAction::Resize(self.geometry(content)), None);
        }

        // Only moved, or configured again for what it shows.
        if let Some(serial) = self.unacknowledged {
            if self.committed == Some(self.geometry(self.placed)) {
                self.unacknowledged = None;
                return (PlatformAction::Acknowledge(serial), None);
            }
        }

        // A frame is wanted: for a configure, or for a new scale.
        let wants_frame =
            self.needs_frame || (self.unacknowledged.is_some() && self.deadline.is_none());
        let late = self.deadline.is_some_and(|deadline| now >= deadline);
        if !(wants_frame || late) {
            return (PlatformAction::None, self.deadline);
        }
        if !may_request {
            return (PlatformAction::None, None);
        }

        self.needs_frame = false;
        let deadline = now + REQUEST_TIMEOUT;
        // Asked again, if it does not come, for as long as it is wanted.
        self.deadline = self.unacknowledged.is_some().then_some(deadline);
        (
            PlatformAction::Request(self.engine_geometry()),
            self.deadline,
        )
    }

    /// Whether a frame of this size would be shown: see `SurfaceSync`.
    pub(crate) fn admit(&mut self, size: PhysicalSize<u32>) -> bool {
        size == self.placed
    }

    /// See `SurfaceSync::frame`.
    pub(crate) fn frame(&mut self, size: Option<PhysicalSize<u32>>) -> FrameAction {
        let Some(size) = size else {
            // A frame without content has no size: it fits what is shown.
            return match self.committed {
                Some(_) => FrameAction::Present,
                None => FrameAction::Drop,
            };
        };

        let resized = self.content != Some(size);
        self.content = Some(size);
        if size != self.placed {
            // `poll` gives the window the size. The frame is asked for
            // again once the compositor configured the window for it.
            return match resized {
                true => FrameAction::Resize,
                false => FrameAction::Drop,
            };
        }

        let geometry = self.geometry(size);
        if let Some(serial) = self.unacknowledged.take() {
            self.deadline = None;
            self.committed = Some(geometry);
            return FrameAction::Commit {
                geometry,
                acknowledge: Some(serial),
            };
        }
        if self.committed == Some(geometry) {
            return FrameAction::Present;
        }

        // The window has the size, but the compositor has not configured
        // it for it yet: the frame is asked for again once it has.
        FrameAction::Drop
    }
}

/// The surface state of a window: sized by the compositor, or sized to its
/// content. Closed once the window's role is gone: nothing may be
/// committed to its surface then.
#[derive(Debug)]
pub(crate) enum WindowSync {
    Sized(SurfaceSync),
    Content(ContentSync),
    Closed,
}

impl WindowSync {
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        may_request: bool,
    ) -> (PlatformAction, Option<Instant>) {
        match self {
            Self::Sized(sync) => sync.poll(now, may_request),
            Self::Content(sync) => sync.poll(now, may_request),
            Self::Closed => (PlatformAction::None, None),
        }
    }

    pub(crate) fn admit(&mut self, size: PhysicalSize<u32>, now: Instant) -> bool {
        match self {
            Self::Sized(sync) => sync.admit(size, now),
            Self::Content(sync) => sync.admit(size),
            Self::Closed => false,
        }
    }

    pub(crate) fn frame(&mut self, size: Option<PhysicalSize<u32>>) -> FrameAction {
        match self {
            Self::Sized(sync) => sync.frame(size),
            Self::Content(sync) => sync.frame(size),
            Self::Closed => FrameAction::Drop,
        }
    }

    /// The geometry the engine draws for at the moment.
    pub(crate) fn engine_geometry(&self) -> Option<SurfaceGeometry> {
        match self {
            Self::Sized(sync) => sync.engine_geometry(),
            Self::Content(sync) => Some(sync.engine_geometry()),
            Self::Closed => None,
        }
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
    fn admitted_frame_is_not_overtaken() {
        let now = Instant::now();
        let mut sync = settled(now);
        let size = |size: (u32, u32)| PhysicalSize::new(size.0, size.1);

        assert!(sync.admit(size(A), now));
        assert!(!sync.admit(size(B), now));

        sync.configure(2, g(B));
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(B)));
        sync.configure(3, g(C));
        assert!(sync.admit(size(A), now));
        assert!(!sync.admit(size(C), now));

        // The frame for B is being drawn when the request would have run
        // out: C waits for it.
        let late = now + REQUEST_TIMEOUT - Duration::from_millis(1);
        assert!(sync.admit(size(B), late));
        assert_eq!(
            sync.poll(now + REQUEST_TIMEOUT, true),
            (PlatformAction::None, Some(late + REQUEST_TIMEOUT))
        );
        assert_eq!(
            sync.frame(s(B)),
            FrameAction::Commit {
                geometry: g(B),
                acknowledge: Some(2)
            }
        );
        assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(C)));
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

    mod content {
        use super::*;

        fn content_sync() -> ContentSync {
            ContentSync::new(PhysicalSize::new(1, 1), 1)
        }

        /// A window that shows its content, of size A, for configure 2.
        fn shown(now: Instant) -> ContentSync {
            let mut sync = content_sync();
            sync.configure(1);
            assert_eq!(
                sync.poll(now, true).0,
                PlatformAction::Request(geometry(1, 1))
            );
            assert_eq!(sync.frame(s(A)), FrameAction::Resize);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Resize(g(A)));
            sync.configure(2);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(A)));
            assert_eq!(
                sync.frame(s(A)),
                FrameAction::Commit {
                    geometry: g(A),
                    acknowledge: Some(2)
                }
            );
            assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
            sync
        }

        #[test]
        fn takes_the_size_of_its_first_frame() {
            shown(Instant::now());
        }

        #[test]
        fn nothing_is_shown_before_a_configure() {
            let now = Instant::now();
            let mut sync = content_sync();
            assert_eq!(
                sync.poll(now, true).0,
                PlatformAction::Request(geometry(1, 1))
            );
            assert_eq!(sync.frame(s(A)), FrameAction::Resize);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Resize(g(A)));
            // Placed, but not configured yet.
            assert_eq!(sync.frame(s(A)), FrameAction::Drop);
            assert_eq!(sync.frame(None), FrameAction::Drop);
            // Not asked for again before the window is configured.
            assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
            sync.configure(1);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(A)));
            assert_eq!(
                sync.frame(s(A)),
                FrameAction::Commit {
                    geometry: g(A),
                    acknowledge: Some(1)
                }
            );
        }

        #[test]
        fn later_frames_of_its_size_are_presented() {
            let now = Instant::now();
            let mut sync = shown(now);
            assert_eq!(sync.frame(s(A)), FrameAction::Present);
            assert_eq!(sync.frame(None), FrameAction::Present);
            assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
        }

        #[test]
        fn follows_its_content() {
            let now = Instant::now();
            let mut sync = shown(now);

            assert_eq!(sync.frame(s(B)), FrameAction::Resize);
            // The same size again only waits.
            assert_eq!(sync.frame(s(B)), FrameAction::Drop);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Resize(g(B)));
            assert_eq!(sync.poll(now, true).0, PlatformAction::None);
            sync.configure(3);
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
        fn a_move_needs_no_frame() {
            let now = Instant::now();
            let mut sync = shown(now);
            sync.configure(3);
            assert_eq!(sync.poll(now, true), (PlatformAction::Acknowledge(3), None));
            assert_eq!(sync.poll(now, true), (PlatformAction::None, None));
        }

        #[test]
        fn a_frame_that_does_not_come_is_asked_for_again() {
            let now = Instant::now();
            let mut sync = shown(now);
            assert_eq!(sync.frame(s(B)), FrameAction::Resize);
            assert_eq!(sync.poll(now, true).0, PlatformAction::Resize(g(B)));
            sync.configure(3);
            let deadline = now + REQUEST_TIMEOUT;
            assert_eq!(
                sync.poll(now, true),
                (PlatformAction::Request(g(B)), Some(deadline))
            );
            assert_eq!(sync.poll(now, true), (PlatformAction::None, Some(deadline)));
            assert_eq!(sync.poll(deadline, true).0, PlatformAction::Request(g(B)));
        }

        #[test]
        fn a_new_scale_is_drawn_for() {
            let now = Instant::now();
            let mut sync = shown(now);
            sync.set_scale(2);
            let scaled = |size: (u32, u32)| SurfaceGeometry {
                size: PhysicalSize::new(size.0, size.1),
                scale: 2,
            };
            assert_eq!(sync.poll(now, true).0, PlatformAction::Request(scaled(A)));
            // The content at the new scale is twice as large.
            assert_eq!(sync.frame(size(1280, 960)), FrameAction::Resize);
            assert_eq!(
                sync.poll(now, true).0,
                PlatformAction::Resize(scaled((1280, 960)))
            );
            sync.configure(3);
            assert_eq!(
                sync.poll(now, true).0,
                PlatformAction::Request(scaled((1280, 960)))
            );
            assert_eq!(
                sync.frame(size(1280, 960)),
                FrameAction::Commit {
                    geometry: scaled((1280, 960)),
                    acknowledge: Some(3)
                }
            );
        }

        #[test]
        fn requests_wait_for_a_good_moment() {
            let now = Instant::now();
            let mut sync = shown(now);
            sync.request_frame();
            assert_eq!(sync.poll(now, false), (PlatformAction::None, None));
            assert_eq!(sync.poll(now, true).0, PlatformAction::Request(g(A)));
        }
    }
}
