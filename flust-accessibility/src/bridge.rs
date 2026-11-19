//! Connects the semantics of a Flutter engine to the accessibility API of
//! the platform.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use accesskit_unix::Adapter;
use flust_engine::{
    ffi::FlutterViewId,
    semantics::{FlutterSemanticsHandler, FlutterSemanticsUpdate},
    FlutterEngine, FlutterEngineWeakRef,
};
use tracing::{debug, info, warn};

use crate::tree::SemanticsTree;

/// Makes the views of a Flutter engine available to assistive technologies,
/// over AT-SPI.
///
/// Set it as the semantics handler of the engine, tell it about the engine's
/// views with [`add_view`](Self::add_view), and call
/// [`engine_started`](Self::engine_started) once the engine runs.
///
/// Keeping the semantics tree up to date costs the framework time on every
/// frame, so the engine is only asked for it while an assistive technology
/// is running.
///
/// AccessKit calls back on a thread of its own. Everything that involves the
/// engine is passed on to the platform thread from there.
#[derive(Clone, Default)]
pub struct Accessibility {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// Set once the engine runs. It can not be asked for anything earlier.
    engine: Mutex<Option<FlutterEngineWeakRef>>,
    /// Not locked on AccessKit's thread: it calls the handlers below while
    /// it holds the lock of an adapter, which the platform thread takes
    /// while it holds this one.
    views: Mutex<HashMap<FlutterViewId, View>>,
    /// The views an assistive technology is looking at.
    active_views: Mutex<HashSet<FlutterViewId>>,
    /// Whether the engine has been asked for semantics. Only used on the
    /// platform thread.
    is_semantics_enabled: Mutex<bool>,
}

struct View {
    tree: Arc<Mutex<SemanticsTree>>,
    adapter: Adapter,
}

impl Accessibility {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes a view known to assistive technologies. `title` is what its
    /// window is called.
    pub fn add_view(&self, view_id: FlutterViewId, title: &str) {
        let tree = Arc::new(Mutex::new(SemanticsTree::new(title)));
        let shared = Arc::downgrade(&self.shared);

        let adapter = Adapter::new(
            Activation {
                view_id,
                tree: tree.clone(),
                shared: shared.clone(),
            },
            Actions {
                view_id,
                tree: tree.clone(),
                shared: shared.clone(),
            },
            Deactivation { view_id, shared },
        );

        self.shared
            .views
            .lock()
            .unwrap()
            .insert(view_id, View { tree, adapter });
    }

    pub fn remove_view(&self, view_id: FlutterViewId) {
        self.shared.views.lock().unwrap().remove(&view_id);
        self.shared.active_views.lock().unwrap().remove(&view_id);
        self.shared
            .run_on_platform_thread(|shared, engine| shared.sync_semantics_enabled(engine));
    }

    /// Whether the window of a view is the one the user is working in.
    pub fn set_view_focused(&self, view_id: FlutterViewId, is_focused: bool) {
        if let Some(view) = self.shared.views.lock().unwrap().get_mut(&view_id) {
            view.adapter.update_window_focus_state(is_focused);
        }
    }

    /// Has to be called on the platform thread, once the engine runs.
    pub fn engine_started(&self, engine: &FlutterEngine) {
        *self.shared.engine.lock().unwrap() = Some(engine.downgrade());
        self.shared.sync_semantics_enabled(engine);
    }
}

impl FlutterSemanticsHandler for Accessibility {
    fn update_semantics(&self, update: FlutterSemanticsUpdate) {
        let mut views = self.shared.views.lock().unwrap();
        let Some(view) = views.get_mut(&update.view_id) else {
            debug!("semantics update for unknown view {}", update.view_id);
            return;
        };

        // The lock of the tree is not held while the adapter takes its own:
        // AccessKit's thread takes them the other way around.
        let tree_update = view.tree.lock().unwrap().apply(update);
        view.adapter.update_if_active(|| tree_update);
    }
}

impl Shared {
    fn engine(&self) -> Option<FlutterEngine> {
        self.engine.lock().unwrap().as_ref()?.upgrade()
    }

    /// Runs `f` on the platform thread, unless the engine is not running.
    fn run_on_platform_thread(
        self: &Arc<Self>,
        f: impl FnOnce(&Arc<Shared>, &FlutterEngine) + Send + 'static,
    ) {
        let Some(engine) = self.engine() else {
            return;
        };
        let shared = self.clone();
        engine.run_on_platform_thread(move |engine| f(&shared, engine));
    }

    /// Asks the engine for semantics if an assistive technology is looking
    /// at a view, and tells it to stop if none is anymore. Has to be called
    /// on the platform thread.
    fn sync_semantics_enabled(&self, engine: &FlutterEngine) {
        let enable = !self.active_views.lock().unwrap().is_empty();

        let mut is_enabled = self.is_semantics_enabled.lock().unwrap();
        if *is_enabled == enable {
            return;
        }
        info!("semantics enabled: {enable}");
        if let Err(err) = engine.update_semantics_enabled(enable) {
            warn!("failed to set semantics enabled to {enable}: {err}");
            return;
        }
        *is_enabled = enable;
        drop(is_enabled);

        if !enable {
            // The engine describes the trees from scratch when it is asked
            // for semantics again.
            for view in self.views.lock().unwrap().values_mut() {
                let tree_update = view.tree.lock().unwrap().clear();
                view.adapter.update_if_active(|| tree_update);
            }
        }
    }
}

struct Activation {
    view_id: FlutterViewId,
    tree: Arc<Mutex<SemanticsTree>>,
    shared: Weak<Shared>,
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        if let Some(shared) = self.shared.upgrade() {
            shared.active_views.lock().unwrap().insert(self.view_id);
            shared.run_on_platform_thread(|shared, engine| shared.sync_semantics_enabled(engine));
        }

        // Until the engine has described the view, this is only its window.
        // The rest follows as a change to it.
        Some(self.tree.lock().unwrap().full_update())
    }
}

struct Deactivation {
    view_id: FlutterViewId,
    shared: Weak<Shared>,
}

impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.active_views.lock().unwrap().remove(&self.view_id);
            shared.run_on_platform_thread(|shared, engine| shared.sync_semantics_enabled(engine));
        }
    }
}

struct Actions {
    view_id: FlutterViewId,
    tree: Arc<Mutex<SemanticsTree>>,
    shared: Weak<Shared>,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let view_id = self.view_id;
        let tree = self.tree.clone();

        // Translated on the platform thread, with the tree as it is by
        // then.
        shared.run_on_platform_thread(move |_, engine| {
            let Some(action) = tree.lock().unwrap().translate_action(&request) else {
                debug!("ignoring accessibility action: {request:?}");
                return;
            };
            if let Err(err) =
                engine.send_semantics_action(view_id, action.node_id, action.action, &action.data)
            {
                warn!("failed to send semantics action {action:?}: {err}");
            }
        });
    }
}
