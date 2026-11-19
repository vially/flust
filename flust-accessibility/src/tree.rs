//! The semantics tree of one Flutter view, kept as the engine last described
//! it, and translated to an AccessKit tree.

use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionData, ActionRequest, Affine, CustomAction, Live, Node, NodeId, Rect, Role,
    TextDirection, Toggled, TreeId, TreeInfo, TreeUpdate,
};
use flust_engine::{
    codec::{MessageCodec, Value, STANDARD_CODEC},
    semantics::{
        FlutterCheckState, FlutterSemanticsAction, FlutterSemanticsCustomAction,
        FlutterSemanticsNode, FlutterSemanticsUpdate, FlutterTextDirection, FlutterTransformation,
        FlutterTristate,
    },
};
use tracing::debug;

/// The id Flutter gives to the root node of a view.
const FLUTTER_ROOT_ID: i32 = 0;

/// The node that stands for the window. It is the root of the AccessKit
/// tree, and has Flutter's root node as its only child.
///
/// Flutter's ids are 32 bit, so the ids of the nodes that are not Flutter's
/// start above them.
pub const WINDOW_NODE_ID: NodeId = NodeId(1 << 32);

/// The actions that are the same thing on both sides.
///
/// Flutter names a scroll action after the direction the finger moves in,
/// AccessKit after the direction of the content that comes into view.
const ACTIONS: &[(FlutterSemanticsAction, Action)] = &[
    (FlutterSemanticsAction::Tap, Action::Click),
    (FlutterSemanticsAction::Focus, Action::Focus),
    (FlutterSemanticsAction::ShowOnScreen, Action::ScrollIntoView),
    (FlutterSemanticsAction::Increase, Action::Increment),
    (FlutterSemanticsAction::Decrease, Action::Decrement),
    (FlutterSemanticsAction::Expand, Action::Expand),
    (FlutterSemanticsAction::Collapse, Action::Collapse),
    (FlutterSemanticsAction::ScrollUp, Action::ScrollDown),
    (FlutterSemanticsAction::ScrollDown, Action::ScrollUp),
    (FlutterSemanticsAction::ScrollLeft, Action::ScrollRight),
    (FlutterSemanticsAction::ScrollRight, Action::ScrollLeft),
    (FlutterSemanticsAction::CustomAction, Action::CustomAction),
];

fn node_id(id: i32) -> NodeId {
    NodeId(id as u32 as u64)
}

/// The Flutter id of a node, unless it is one that Flutter does not know.
fn flutter_id(id: NodeId) -> Option<i32> {
    i32::try_from(id.0).ok()
}

/// An action for the engine to pass on to a semantics node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticsActionRequest {
    pub node_id: i32,
    pub action: FlutterSemanticsAction,
    /// The arguments of the action, encoded for the engine.
    pub data: Vec<u8>,
}

/// The semantics tree of a view.
///
/// The engine only reports what changes. AccessKit has to be told which
/// node has the focus with every change, and is at times asked for the tree
/// as a whole, so all of it is kept here.
#[derive(Debug, Default)]
pub struct SemanticsTree {
    title: String,
    nodes: HashMap<i32, FlutterSemanticsNode>,
    custom_actions: HashMap<i32, FlutterSemanticsCustomAction>,
    /// The node that has the input focus, if it is one of Flutter's.
    focus: Option<i32>,
    /// Nodes that name a child the engine has not described. They are sent
    /// without it, and again with every update until it is there.
    incomplete: HashSet<i32>,
}

impl SemanticsTree {
    /// `title` is what the window is called.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    /// Whether the engine has not described anything yet.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Takes in a change from the engine. Returns what it changes in the
    /// AccessKit tree.
    pub fn apply(&mut self, update: FlutterSemanticsUpdate) -> TreeUpdate {
        let had_root = self.nodes.contains_key(&FLUTTER_ROOT_ID);

        let mut changed: HashSet<i32> = std::mem::take(&mut self.incomplete);
        for node in update.nodes {
            changed.insert(node.id);
            self.nodes.insert(node.id, node);
        }
        for action in update.custom_actions {
            self.custom_actions.insert(action.id, action);
        }

        self.remove_unreachable_nodes();

        let mut nodes: Vec<(NodeId, Node)> = Vec::with_capacity(changed.len() + 1);
        if had_root != self.nodes.contains_key(&FLUTTER_ROOT_ID) {
            nodes.push((WINDOW_NODE_ID, self.build_window_node()));
        }
        for id in changed {
            if let Some(node) = self.nodes.get(&id) {
                let (built, is_complete) = self.build_node(node);
                if !is_complete {
                    self.incomplete.insert(id);
                }
                nodes.push((node_id(id), built));
            }
        }

        self.tree_update(nodes)
    }

    /// Forgets what the engine has described. Returns what is left of the
    /// AccessKit tree: the window.
    pub fn clear(&mut self) -> TreeUpdate {
        self.nodes.clear();
        self.custom_actions.clear();
        self.incomplete.clear();
        self.focus = None;

        self.full_update()
    }

    /// The AccessKit tree as a whole.
    pub fn full_update(&self) -> TreeUpdate {
        let mut nodes: Vec<(NodeId, Node)> = Vec::with_capacity(self.nodes.len() + 1);
        nodes.push((WINDOW_NODE_ID, self.build_window_node()));
        for (id, node) in &self.nodes {
            nodes.push((node_id(*id), self.build_node(node).0));
        }

        self.tree_update(nodes)
    }

    /// What the engine has to be asked to do for an action that an assistive
    /// technology requests, if the node it is for can do it.
    pub fn translate_action(&self, request: &ActionRequest) -> Option<SemanticsActionRequest> {
        let id = flutter_id(request.target_node)?;
        let node = self.nodes.get(&id)?;

        let (action, _) = ACTIONS
            .iter()
            .find(|(_, action)| *action == request.action)?;
        if !node.actions.contains(*action) {
            return None;
        }

        let data = match (&request.action, &request.data) {
            (Action::CustomAction, Some(ActionData::CustomAction(custom_action_id))) => {
                if !node.custom_accessibility_actions.contains(custom_action_id) {
                    return None;
                }
                STANDARD_CODEC.encode_message(&Value::I32(*custom_action_id))
            }
            (Action::CustomAction, _) => return None,
            _ => Vec::new(),
        };

        Some(SemanticsActionRequest {
            node_id: id,
            action: *action,
            data,
        })
    }

    fn tree_update(&self, nodes: Vec<(NodeId, Node)>) -> TreeUpdate {
        let mut tree = TreeInfo::new(WINDOW_NODE_ID);
        tree.toolkit_name = Some("Flutter".into());

        TreeUpdate {
            nodes,
            tree: Some(tree),
            tree_id: TreeId::ROOT,
            focus: self.focus.map(node_id).unwrap_or(WINDOW_NODE_ID),
        }
    }

    /// Drops the nodes that are no longer a descendant of the root, and
    /// finds the one that has the focus.
    ///
    /// The engine does not say that a node is gone. It is, once no other
    /// node has it as a child.
    fn remove_unreachable_nodes(&mut self) {
        let mut reachable: HashSet<i32> = HashSet::with_capacity(self.nodes.len());
        let mut focus = None;

        let mut pending = vec![FLUTTER_ROOT_ID];
        while let Some(id) = pending.pop() {
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            if !reachable.insert(id) {
                continue;
            }
            if node.flags.is_focused == FlutterTristate::True {
                focus = Some(id);
            }
            pending.extend(&node.children_in_traversal_order);
        }

        self.nodes.retain(|id, _| reachable.contains(id));
        self.focus = focus;
    }

    fn build_window_node(&self) -> Node {
        let mut window = Node::new(Role::Window);
        window.set_label(self.title.as_str());
        if self.nodes.contains_key(&FLUTTER_ROOT_ID) {
            window.set_children([node_id(FLUTTER_ROOT_ID)]);
        }
        window
    }

    /// Returns the AccessKit node, and whether it has all the children the
    /// engine named for it.
    fn build_node(&self, node: &FlutterSemanticsNode) -> (Node, bool) {
        let role = role_of(node);
        let mut built = Node::new(role);

        let children: Vec<NodeId> = node
            .children_in_traversal_order
            .iter()
            .filter(|id| self.nodes.contains_key(id))
            .map(|id| node_id(*id))
            .collect();
        let is_complete = children.len() == node.children_in_traversal_order.len();
        if !is_complete {
            debug!(
                "semantics node {} names children that have not been described",
                node.id
            );
        }
        built.set_children(children);

        built.set_bounds(Rect::new(
            node.rect.left,
            node.rect.top,
            node.rect.right,
            node.rect.bottom,
        ));
        if node.transform != FlutterTransformation::IDENTITY {
            // Any perspective a transform has is left out: assistive
            // technologies work with flat rectangles.
            built.set_transform(Affine::new([
                node.transform.scale_x,
                node.transform.skew_y,
                node.transform.skew_x,
                node.transform.scale_y,
                node.transform.trans_x,
                node.transform.trans_y,
            ]));
        }

        // A label is what AccessKit calls a piece of text: its text is its
        // value.
        if role == Role::Label {
            let text = if node.label.is_empty() {
                &node.value
            } else {
                &node.label
            };
            built.set_value(text.as_str());
        } else {
            if !node.label.is_empty() {
                built.set_label(node.label.as_str());
            }
            if !node.value.is_empty() {
                built.set_value(node.value.as_str());
            }
        }
        if !node.hint.is_empty() {
            built.set_description(node.hint.as_str());
        }
        if !node.tooltip.is_empty() {
            built.set_tooltip(node.tooltip.as_str());
        }
        if !node.identifier.is_empty() {
            built.set_author_id(node.identifier.as_str());
        }
        match node.text_direction {
            FlutterTextDirection::Ltr => built.set_text_direction(TextDirection::LeftToRight),
            FlutterTextDirection::Rtl => built.set_text_direction(TextDirection::RightToLeft),
            FlutterTextDirection::Unknown => {}
        }

        let flags = &node.flags;
        match (flags.is_checked, flags.is_toggled) {
            (FlutterCheckState::True, _) | (_, FlutterTristate::True) => {
                built.set_toggled(Toggled::True)
            }
            (FlutterCheckState::Mixed, _) => built.set_toggled(Toggled::Mixed),
            (FlutterCheckState::False, _) | (_, FlutterTristate::False) => {
                built.set_toggled(Toggled::False)
            }
            (FlutterCheckState::None, FlutterTristate::None) => {}
        }
        if flags.is_selected.is_some() {
            built.set_selected(flags.is_selected.is_true());
        }
        if flags.is_expanded.is_some() {
            built.set_expanded(flags.is_expanded.is_true());
        }
        if flags.is_enabled == FlutterTristate::False {
            built.set_disabled();
        }
        if flags.is_required.is_true() {
            built.set_required();
        }
        if flags.is_read_only {
            built.set_read_only();
        }
        if flags.is_live_region {
            built.set_live(Live::Polite);
        }
        if role == Role::Heading && node.heading_level > 0 {
            built.set_level(node.heading_level as usize);
        }
        // `is_hidden` is not passed on. Flutter sets it on what is scrolled
        // out of view, which an assistive technology still has to be able to
        // move to. AccessKit would leave such a node out altogether.

        let scrolls_vertically = node.actions.contains(FlutterSemanticsAction::ScrollUp)
            || node.actions.contains(FlutterSemanticsAction::ScrollDown);
        let scrolls_horizontally = node.actions.contains(FlutterSemanticsAction::ScrollLeft)
            || node.actions.contains(FlutterSemanticsAction::ScrollRight);
        if scrolls_vertically {
            built.set_scroll_y(node.scroll_position);
            // A list that builds its children as they scroll into view does
            // not know where it ends.
            if node.scroll_extent_min.is_finite() {
                built.set_scroll_y_min(node.scroll_extent_min);
            }
            if node.scroll_extent_max.is_finite() {
                built.set_scroll_y_max(node.scroll_extent_max);
            }
        } else if scrolls_horizontally {
            built.set_scroll_x(node.scroll_position);
            if node.scroll_extent_min.is_finite() {
                built.set_scroll_x_min(node.scroll_extent_min);
            }
            if node.scroll_extent_max.is_finite() {
                built.set_scroll_x_max(node.scroll_extent_max);
            }
        }

        for (flutter_action, action) in ACTIONS {
            if *flutter_action != FlutterSemanticsAction::CustomAction
                && node.actions.contains(*flutter_action)
            {
                built.add_action(*action);
            }
        }
        if node.actions.contains(FlutterSemanticsAction::CustomAction) {
            let custom_actions: Vec<CustomAction> = node
                .custom_accessibility_actions
                .iter()
                .filter_map(|id| self.custom_actions.get(id))
                // The ones that stand in for a standard action only give it
                // another label.
                .filter(|action| action.override_action.is_none())
                .map(|action| CustomAction {
                    id: action.id,
                    description: action.label.clone(),
                })
                .collect();
            if !custom_actions.is_empty() {
                built.add_action(Action::CustomAction);
                built.set_custom_actions(custom_actions);
            }
        }

        (built, is_complete)
    }
}

/// What kind of thing a node is.
///
/// The engine does not pass on the role a widget gives itself, only flags
/// and the actions it has. This goes from the most specific of them to the
/// least.
fn role_of(node: &FlutterSemanticsNode) -> Role {
    let flags = &node.flags;
    let actions = node.actions;

    if flags.is_text_field {
        return if flags.is_obscured {
            Role::PasswordInput
        } else if flags.is_multiline {
            Role::MultilineTextInput
        } else {
            Role::TextInput
        };
    }
    if flags.is_slider {
        return Role::Slider;
    }
    if flags.is_checked.is_some() {
        return if flags.is_in_mutually_exclusive_group {
            Role::RadioButton
        } else {
            Role::CheckBox
        };
    }
    if flags.is_toggled.is_some() {
        return Role::Switch;
    }
    if flags.is_link {
        return Role::Link;
    }
    if flags.is_button || flags.is_keyboard_key {
        return Role::Button;
    }
    if flags.is_header || node.heading_level > 0 {
        return Role::Heading;
    }
    if flags.is_image {
        return Role::Image;
    }

    let scrolls = [
        FlutterSemanticsAction::ScrollUp,
        FlutterSemanticsAction::ScrollDown,
        FlutterSemanticsAction::ScrollLeft,
        FlutterSemanticsAction::ScrollRight,
    ]
    .into_iter()
    .any(|action| actions.contains(action));
    if scrolls || flags.has_implicit_scrolling {
        return Role::ScrollView;
    }

    let has_text = !node.label.is_empty() || !node.value.is_empty();
    let has_one_text = node.label.is_empty() || node.value.is_empty();
    if has_text
        && has_one_text
        && node.children_in_traversal_order.is_empty()
        && !actions.contains(FlutterSemanticsAction::Tap)
    {
        return Role::Label;
    }

    Role::GenericContainer
}

#[cfg(test)]
mod tests {
    use flust_engine::semantics::{FlutterRect, FlutterSemanticsActions, FlutterSemanticsFlags};

    use super::*;

    fn node(id: i32, children: &[i32]) -> FlutterSemanticsNode {
        FlutterSemanticsNode {
            id,
            children_in_traversal_order: children.to_vec(),
            children_in_hit_test_order: children.to_vec(),
            ..Default::default()
        }
    }

    fn labelled(id: i32, label: &str) -> FlutterSemanticsNode {
        FlutterSemanticsNode {
            label: label.into(),
            ..node(id, &[])
        }
    }

    fn button(id: i32, label: &str) -> FlutterSemanticsNode {
        FlutterSemanticsNode {
            flags: FlutterSemanticsFlags {
                is_button: true,
                is_enabled: FlutterTristate::True,
                is_focused: FlutterTristate::False,
                ..Default::default()
            },
            actions: [FlutterSemanticsAction::Tap, FlutterSemanticsAction::Focus]
                .into_iter()
                .collect(),
            ..labelled(id, label)
        }
    }

    fn update(nodes: Vec<FlutterSemanticsNode>) -> FlutterSemanticsUpdate {
        FlutterSemanticsUpdate {
            view_id: 0,
            nodes,
            custom_actions: vec![],
        }
    }

    fn find(update: &TreeUpdate, id: NodeId) -> &Node {
        update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .map(|(_, node)| node)
            .unwrap_or_else(|| panic!("no node {id:?} in the update"))
    }

    fn ids(update: &TreeUpdate) -> Vec<u64> {
        let mut ids: Vec<u64> = update.nodes.iter().map(|(id, _)| id.0).collect();
        ids.sort();
        ids
    }

    fn request(action: Action, target: NodeId, data: Option<ActionData>) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: target,
            data,
        }
    }

    #[test]
    fn a_tree_without_semantics_is_the_window() {
        let tree = SemanticsTree::new("Probe");

        let update = tree.full_update();

        assert_eq!(ids(&update), [WINDOW_NODE_ID.0]);
        let window = find(&update, WINDOW_NODE_ID);
        assert_eq!(window.role(), Role::Window);
        assert_eq!(window.label(), Some("Probe"));
        assert!(window.children().is_empty());
        assert_eq!(update.tree.as_ref().unwrap().root, WINDOW_NODE_ID);
        assert_eq!(update.focus, WINDOW_NODE_ID);
    }

    #[test]
    fn the_first_update_hangs_the_root_under_the_window() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));

        assert_eq!(ids(&update), [0, 1, WINDOW_NODE_ID.0]);
        assert_eq!(find(&update, WINDOW_NODE_ID).children(), [NodeId(0)]);
        assert_eq!(find(&update, NodeId(0)).children(), [NodeId(1)]);
    }

    #[test]
    fn a_later_update_only_has_what_changed() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1, 2]),
            button(1, "Send"),
            labelled(2, "Draft"),
        ]));

        let update = tree.apply(update(vec![labelled(2, "Sent")]));

        assert_eq!(ids(&update), [2]);
        assert_eq!(find(&update, NodeId(2)).value(), Some("Sent"));
    }

    #[test]
    fn the_full_tree_has_every_node() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));
        tree.apply(update(vec![button(1, "Sent")]));

        let update = tree.full_update();

        assert_eq!(ids(&update), [0, 1, WINDOW_NODE_ID.0]);
        assert_eq!(find(&update, NodeId(1)).label(), Some("Sent"));
    }

    #[test]
    fn a_node_that_is_no_longer_a_child_is_gone() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1, 2]),
            node(1, &[3]),
            labelled(2, "Stays"),
            labelled(3, "Goes with its parent"),
        ]));

        let update = tree.apply(update(vec![node(0, &[2])]));

        assert_eq!(ids(&update), [0]);
        assert_eq!(ids(&tree.full_update()), [0, 2, WINDOW_NODE_ID.0]);
    }

    #[test]
    fn a_node_can_move_to_another_parent() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1, 2]),
            node(1, &[3]),
            node(2, &[]),
            labelled(3, "Moves"),
        ]));

        tree.apply(update(vec![node(1, &[]), node(2, &[3])]));

        let full = tree.full_update();
        assert_eq!(ids(&full), [0, 1, 2, 3, WINDOW_NODE_ID.0]);
        assert_eq!(find(&full, NodeId(2)).children(), [NodeId(3)]);
    }

    #[test]
    fn a_child_that_is_not_described_is_left_out_until_it_is() {
        let mut tree = SemanticsTree::new("Probe");

        let first = tree.apply(update(vec![node(0, &[1])]));
        assert!(find(&first, NodeId(0)).children().is_empty());

        let second = tree.apply(update(vec![labelled(1, "Late")]));
        assert_eq!(ids(&second), [0, 1]);
        assert_eq!(find(&second, NodeId(0)).children(), [NodeId(1)]);

        let third = tree.apply(update(vec![labelled(1, "Later")]));
        assert_eq!(ids(&third), [1]);
    }

    #[test]
    fn clearing_leaves_the_window() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));

        let cleared = tree.clear();

        assert!(tree.is_empty());
        assert_eq!(ids(&cleared), [WINDOW_NODE_ID.0]);
        assert!(find(&cleared, WINDOW_NODE_ID).children().is_empty());

        let again = tree.apply(update(vec![node(0, &[])]));
        assert_eq!(ids(&again), [0, WINDOW_NODE_ID.0]);
    }

    #[test]
    fn the_focus_follows_the_focused_node() {
        let mut tree = SemanticsTree::new("Probe");
        let mut send = button(1, "Send");
        tree.apply(update(vec![node(0, &[1]), send.clone()]));
        assert_eq!(tree.full_update().focus, WINDOW_NODE_ID);

        send.flags.is_focused = FlutterTristate::True;
        assert_eq!(tree.apply(update(vec![send.clone()])).focus, NodeId(1));
        // An update for another node says so again.
        assert_eq!(tree.apply(update(vec![node(0, &[1])])).focus, NodeId(1));

        send.flags.is_focused = FlutterTristate::False;
        assert_eq!(tree.apply(update(vec![send])).focus, WINDOW_NODE_ID);
    }

    #[test]
    fn the_focus_goes_with_a_removed_node() {
        let mut tree = SemanticsTree::new("Probe");
        let mut send = button(1, "Send");
        send.flags.is_focused = FlutterTristate::True;
        tree.apply(update(vec![node(0, &[1]), send]));

        let update = tree.apply(update(vec![node(0, &[])]));

        assert_eq!(update.focus, WINDOW_NODE_ID);
    }

    #[test]
    fn roles() {
        let flags = |set: fn(&mut FlutterSemanticsFlags)| {
            let mut node = labelled(1, "Label");
            set(&mut node.flags);
            role_of(&node)
        };

        assert_eq!(flags(|f| f.is_button = true), Role::Button);
        assert_eq!(flags(|f| f.is_link = true), Role::Link);
        assert_eq!(flags(|f| f.is_image = true), Role::Image);
        assert_eq!(flags(|f| f.is_header = true), Role::Heading);
        assert_eq!(flags(|f| f.is_slider = true), Role::Slider);
        assert_eq!(flags(|f| f.is_text_field = true), Role::TextInput);
        assert_eq!(
            flags(|f| {
                f.is_text_field = true;
                f.is_multiline = true;
            }),
            Role::MultilineTextInput
        );
        assert_eq!(
            flags(|f| {
                f.is_text_field = true;
                f.is_obscured = true;
            }),
            Role::PasswordInput
        );
        assert_eq!(
            flags(|f| f.is_checked = FlutterCheckState::False),
            Role::CheckBox
        );
        assert_eq!(
            flags(|f| {
                f.is_checked = FlutterCheckState::True;
                f.is_in_mutually_exclusive_group = true;
            }),
            Role::RadioButton
        );
        assert_eq!(
            flags(|f| f.is_toggled = FlutterTristate::False),
            Role::Switch
        );
        // A switch is also a button to Flutter.
        assert_eq!(
            flags(|f| {
                f.is_toggled = FlutterTristate::True;
                f.is_button = true;
            }),
            Role::Switch
        );
        assert_eq!(flags(|f| f.has_implicit_scrolling = true), Role::ScrollView);
        assert_eq!(flags(|_| {}), Role::Label);

        assert_eq!(role_of(&node(1, &[])), Role::GenericContainer);
        assert_eq!(
            role_of(&FlutterSemanticsNode {
                label: "Group".into(),
                ..node(1, &[2])
            }),
            Role::GenericContainer
        );
        assert_eq!(
            role_of(&FlutterSemanticsNode {
                heading_level: 2,
                ..labelled(1, "Title")
            }),
            Role::Heading
        );
        assert_eq!(
            role_of(&FlutterSemanticsNode {
                actions: FlutterSemanticsAction::ScrollUp.into(),
                ..node(1, &[2])
            }),
            Role::ScrollView
        );
    }

    #[test]
    fn text_and_state() {
        let mut tree = SemanticsTree::new("Probe");
        let checkbox = FlutterSemanticsNode {
            flags: FlutterSemanticsFlags {
                is_checked: FlutterCheckState::Mixed,
                is_enabled: FlutterTristate::False,
                is_selected: FlutterTristate::False,
                is_expanded: FlutterTristate::True,
                is_required: FlutterTristate::True,
                is_live_region: true,
                is_hidden: true,
                ..Default::default()
            },
            value: "Some".into(),
            hint: "Selects all of them".into(),
            tooltip: "All".into(),
            identifier: "select-all".into(),
            text_direction: FlutterTextDirection::Rtl,
            ..labelled(1, "Select all")
        };
        let heading = FlutterSemanticsNode {
            heading_level: 2,
            ..labelled(2, "Inbox")
        };

        let update = tree.apply(update(vec![node(0, &[1, 2]), checkbox, heading]));

        let checkbox = find(&update, NodeId(1));
        assert_eq!(checkbox.role(), Role::CheckBox);
        assert_eq!(checkbox.label(), Some("Select all"));
        assert_eq!(checkbox.value(), Some("Some"));
        assert_eq!(checkbox.description(), Some("Selects all of them"));
        assert_eq!(checkbox.tooltip(), Some("All"));
        assert_eq!(checkbox.author_id(), Some("select-all"));
        assert_eq!(checkbox.text_direction(), Some(TextDirection::RightToLeft));
        assert_eq!(checkbox.toggled(), Some(Toggled::Mixed));
        assert_eq!(checkbox.is_selected(), Some(false));
        assert_eq!(checkbox.is_expanded(), Some(true));
        assert!(checkbox.is_disabled());
        assert!(checkbox.is_required());
        assert_eq!(checkbox.live(), Some(Live::Polite));
        assert!(!checkbox.is_hidden());

        let heading = find(&update, NodeId(2));
        assert_eq!(heading.role(), Role::Heading);
        assert_eq!(heading.level(), Some(2));

        let root = find(&update, NodeId(0));
        assert_eq!(root.label(), None);
        assert_eq!(root.toggled(), None);
        assert_eq!(root.is_selected(), None);
        assert!(!root.is_disabled());
    }

    #[test]
    fn the_text_of_a_label_is_its_value() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![node(0, &[1]), labelled(1, "Hello")]));

        let label = find(&update, NodeId(1));
        assert_eq!(label.role(), Role::Label);
        assert_eq!(label.value(), Some("Hello"));
        assert_eq!(label.label(), None);
    }

    #[test]
    fn bounds_and_transform() {
        let mut tree = SemanticsTree::new("Probe");
        let moved = FlutterSemanticsNode {
            rect: FlutterRect {
                left: 1.0,
                top: 2.0,
                right: 30.0,
                bottom: 40.0,
            },
            transform: FlutterTransformation {
                scale_x: 2.0,
                skew_x: 0.25,
                trans_x: 10.0,
                skew_y: 0.5,
                scale_y: 3.0,
                trans_y: 20.0,
                ..FlutterTransformation::IDENTITY
            },
            ..node(1, &[])
        };

        let update = tree.apply(update(vec![node(0, &[1]), moved]));

        let moved = find(&update, NodeId(1));
        assert_eq!(moved.bounds(), Some(Rect::new(1.0, 2.0, 30.0, 40.0)));
        // x' = 2x + 0.25y + 10, y' = 0.5x + 3y + 20
        assert_eq!(
            moved.transform(),
            Some(&Affine::new([2.0, 0.5, 0.25, 3.0, 10.0, 20.0]))
        );
        assert_eq!(find(&update, NodeId(0)).transform(), None);
    }

    #[test]
    fn scroll_position() {
        let mut tree = SemanticsTree::new("Probe");
        let list = FlutterSemanticsNode {
            actions: [
                FlutterSemanticsAction::ScrollUp,
                FlutterSemanticsAction::ScrollDown,
            ]
            .into_iter()
            .collect(),
            scroll_position: 120.0,
            scroll_extent_min: 0.0,
            scroll_extent_max: f64::INFINITY,
            ..node(1, &[])
        };

        let update = tree.apply(update(vec![node(0, &[1]), list]));

        let list = find(&update, NodeId(1));
        assert_eq!(list.role(), Role::ScrollView);
        assert_eq!(list.scroll_y(), Some(120.0));
        assert_eq!(list.scroll_y_min(), Some(0.0));
        assert_eq!(list.scroll_y_max(), None);
        assert_eq!(list.scroll_x(), None);
        // Moving the finger up brings what is below into view.
        assert!(list.supports_action(Action::ScrollDown));
        assert!(list.supports_action(Action::ScrollUp));
    }

    #[test]
    fn actions_go_both_ways() {
        let mut tree = SemanticsTree::new("Probe");
        let update = tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));

        let send = find(&update, NodeId(1));
        assert!(send.supports_action(Action::Click));
        assert!(send.supports_action(Action::Focus));
        assert!(!send.supports_action(Action::Expand));

        assert_eq!(
            tree.translate_action(&request(Action::Click, NodeId(1), None)),
            Some(SemanticsActionRequest {
                node_id: 1,
                action: FlutterSemanticsAction::Tap,
                data: vec![],
            })
        );
        assert_eq!(
            tree.translate_action(&request(Action::Focus, NodeId(1), None))
                .map(|request| request.action),
            Some(FlutterSemanticsAction::Focus)
        );
    }

    #[test]
    fn actions_a_node_can_not_do_are_dropped() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));

        // Not an action of the node.
        assert_eq!(
            tree.translate_action(&request(Action::Expand, NodeId(1), None)),
            None
        );
        // Not an action Flutter has.
        assert_eq!(
            tree.translate_action(&request(Action::ShowContextMenu, NodeId(1), None)),
            None
        );
        // Not a node Flutter has.
        assert_eq!(
            tree.translate_action(&request(Action::Click, WINDOW_NODE_ID, None)),
            None
        );
        assert_eq!(
            tree.translate_action(&request(Action::Click, NodeId(7), None)),
            None
        );
    }

    #[test]
    fn scroll_actions_are_named_the_other_way_around() {
        let mut tree = SemanticsTree::new("Probe");
        let list = FlutterSemanticsNode {
            actions: FlutterSemanticsAction::ScrollUp.into(),
            ..node(1, &[])
        };
        tree.apply(update(vec![node(0, &[1]), list]));

        assert_eq!(
            tree.translate_action(&request(Action::ScrollDown, NodeId(1), None))
                .map(|request| request.action),
            Some(FlutterSemanticsAction::ScrollUp)
        );
        assert_eq!(
            tree.translate_action(&request(Action::ScrollUp, NodeId(1), None)),
            None
        );
    }

    #[test]
    fn custom_actions() {
        let mut tree = SemanticsTree::new("Probe");
        let message = FlutterSemanticsNode {
            actions: [
                FlutterSemanticsAction::CustomAction,
                FlutterSemanticsAction::Tap,
            ]
            .into_iter()
            .collect::<FlutterSemanticsActions>(),
            custom_accessibility_actions: vec![7, 8],
            ..labelled(1, "Message")
        };

        let update = tree.apply(FlutterSemanticsUpdate {
            view_id: 0,
            nodes: vec![node(0, &[1]), message],
            custom_actions: vec![
                FlutterSemanticsCustomAction {
                    id: 7,
                    override_action: None,
                    label: "Archive".into(),
                    hint: String::new(),
                },
                FlutterSemanticsCustomAction {
                    id: 8,
                    override_action: Some(FlutterSemanticsAction::Tap),
                    label: String::new(),
                    hint: "open".into(),
                },
            ],
        });

        let message = find(&update, NodeId(1));
        assert!(message.supports_action(Action::CustomAction));
        assert_eq!(
            message.custom_actions(),
            [CustomAction {
                id: 7,
                description: "Archive".into(),
            }]
        );

        let archive = tree
            .translate_action(&request(
                Action::CustomAction,
                NodeId(1),
                Some(ActionData::CustomAction(7)),
            ))
            .unwrap();
        assert_eq!(archive.action, FlutterSemanticsAction::CustomAction);
        // The standard message codec's int32: its type, then the value.
        assert_eq!(archive.data, [3, 7, 0, 0, 0]);

        assert_eq!(
            tree.translate_action(&request(
                Action::CustomAction,
                NodeId(1),
                Some(ActionData::CustomAction(9)),
            )),
            None
        );
        assert_eq!(
            tree.translate_action(&request(Action::CustomAction, NodeId(1), None)),
            None
        );
    }
}
