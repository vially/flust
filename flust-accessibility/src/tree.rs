//! The semantics tree of one Flutter view, kept as the engine last described
//! it, and translated to an AccessKit tree.

use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionData, ActionRequest, Affine, CustomAction, Live, Node, NodeId, Rect, Role,
    TextDirection, TextPosition, TextSelection, Toggled, TreeId, TreeInfo, TreeUpdate,
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

use crate::text::{text_run_id, text_run_of, TextLayout};

/// The id Flutter gives to the root node of a view.
const FLUTTER_ROOT_ID: i32 = 0;

/// The node that stands for the window. It is the root of the AccessKit
/// tree, and has Flutter's root node as its only child.
///
/// Flutter's ids are 32 bit, so the ids of the nodes that are not Flutter's
/// start above them.
pub const WINDOW_NODE_ID: NodeId = NodeId(1 << 32);

/// A node that is only in the tree for the moment it takes to announce a
/// message. See [`SemanticsTree::announce`].
pub const ANNOUNCEMENT_NODE_ID: NodeId = NodeId((1 << 32) + 1);

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
                if !self.build_nodes(node, &mut nodes) {
                    self.incomplete.insert(id);
                }
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
        for node in self.nodes.values() {
            self.build_nodes(node, &mut nodes);
        }

        self.tree_update(nodes)
    }

    /// What the engine has to be asked to do for an action that an assistive
    /// technology requests, if the node it is for can do it.
    pub fn translate_action(&self, request: &ActionRequest) -> Option<SemanticsActionRequest> {
        let id = flutter_id(request.target_node)?;
        let node = self.nodes.get(&id)?;

        match (&request.action, &request.data) {
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection))) => {
                return translate_set_selection(node, selection);
            }
            (Action::SetValue, Some(ActionData::Value(text))) => {
                return translate_set_text(node, text);
            }
            (Action::SetValue, Some(ActionData::NumericValue(value))) => {
                return translate_set_numeric_value(node, *value);
            }
            _ => {}
        }

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

    /// Has an assistive technology say a message that is not part of what
    /// is on screen, interrupting what it is saying if `interrupt` is set.
    ///
    /// There is no way to ask for that as such. What gets announced is a
    /// node that is marked as one to announce, when it appears. So the
    /// first update that is returned adds such a node to the window, and the
    /// second one takes it out again: the same message can then be announced
    /// again, and nothing is left behind for the user to come across.
    pub fn announce(&self, message: &str, interrupt: bool) -> [TreeUpdate; 2] {
        let mut announcement = Node::new(Role::Label);
        announcement.set_value(message);
        announcement.set_live(if interrupt {
            Live::Assertive
        } else {
            Live::Polite
        });

        let mut window = self.build_window_node();
        window.push_child(ANNOUNCEMENT_NODE_ID);

        [
            self.tree_update(vec![
                (WINDOW_NODE_ID, window),
                (ANNOUNCEMENT_NODE_ID, announcement),
            ]),
            self.tree_update(vec![(WINDOW_NODE_ID, self.build_window_node())]),
        ]
    }

    /// Adds the AccessKit node for a semantics node to `nodes`, and the
    /// nodes that AccessKit wants for it but Flutter does not have. Returns
    /// whether it has all the children the engine named for it.
    fn build_nodes(&self, node: &FlutterSemanticsNode, nodes: &mut Vec<(NodeId, Node)>) -> bool {
        // The text of a text field is a list of child nodes to AccessKit.
        let text = node
            .flags
            .is_text_field
            .then(|| TextLayout::new(&node.value));

        let (built, is_complete) = self.build_node(node, text.as_ref());
        nodes.push((node_id(node.id), built));

        if let Some(text) = text {
            let continued: Vec<bool> = text.runs[1..]
                .iter()
                .map(|next| next.continues_line)
                .chain([false])
                .collect();
            for (index, run) in text.runs.into_iter().enumerate() {
                let mut built = Node::new(Role::TextRun);
                if continued[index] {
                    built.set_next_on_line(text_run_id(node.id, index + 1));
                }
                built.set_value(run.text);
                built.set_character_lengths(run.character_lengths);
                built.set_word_starts(run.word_starts);
                if run.continues_line {
                    built.set_previous_on_line(text_run_id(node.id, index - 1));
                }
                nodes.push((text_run_id(node.id, index), built));
            }
        }

        is_complete
    }

    /// Returns the AccessKit node, and whether it has all the children the
    /// engine named for it.
    fn build_node(&self, node: &FlutterSemanticsNode, text: Option<&TextLayout>) -> (Node, bool) {
        let role = role_of(node);
        let mut built = Node::new(role);

        let flutter_children: Vec<NodeId> = node
            .children_in_traversal_order
            .iter()
            .filter(|id| self.nodes.contains_key(id))
            .map(|id| node_id(*id))
            .collect();
        let is_complete = flutter_children.len() == node.children_in_traversal_order.len();
        let text_runs = text.map_or(0, |text| text.runs.len());
        let children: Vec<NodeId> = (0..text_runs)
            .map(|run| text_run_id(node.id, run))
            .chain(flutter_children)
            .collect();
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
            // The value of a text field is the text of its runs.
            if !node.value.is_empty() && text.is_none() {
                built.set_value(node.value.as_str());
            }
        }
        if let Some(text) = text {
            let position = |utf16_offset: i32| {
                let (run, character_index) = text.position(utf16_offset as usize);
                TextPosition {
                    node: text_run_id(node.id, run),
                    character_index,
                }
            };
            // Without a selection, both ends of it are `-1`.
            if node.text_selection_base >= 0 && node.text_selection_extent >= 0 {
                built.set_text_selection(TextSelection {
                    anchor: position(node.text_selection_base),
                    focus: position(node.text_selection_extent),
                });
            }
            if node.actions.contains(FlutterSemanticsAction::SetSelection) {
                built.add_action(Action::SetTextSelection);
            }
            if node.actions.contains(FlutterSemanticsAction::SetText) {
                built.add_action(Action::SetValue);
            }
        }
        if let Some(value) = adjustable_value(node) {
            built.set_numeric_value(value.current);
            if let Some((min, max)) = value.range {
                built.set_min_numeric_value(min);
                built.set_max_numeric_value(max);
            }
            if let Some(step) = value.step {
                built.set_numeric_value_step(step);
            }
            built.add_action(Action::SetValue);
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

/// The value of a node that can be increased and decreased, as a number.
struct AdjustableValue {
    current: f64,
    range: Option<(f64, f64)>,
    step: Option<f64>,
}

/// Flutter describes the value of a slider as text, such as `50%`. The
/// accessibility API of the platform wants a number, which is there if that
/// text is one.
fn adjustable_value(node: &FlutterSemanticsNode) -> Option<AdjustableValue> {
    let is_adjustable = node.actions.contains(FlutterSemanticsAction::Increase)
        || node.actions.contains(FlutterSemanticsAction::Decrease);
    if !is_adjustable {
        return None;
    }

    let number = |text: &str| -> Option<f64> {
        let text = text.trim();
        text.strip_suffix('%').unwrap_or(text).trim().parse().ok()
    };

    let current = number(&node.value)?;
    let neighbour = number(&node.increased_value).or_else(|| number(&node.decreased_value));
    Some(AdjustableValue {
        current,
        range: node.value.trim().ends_with('%').then_some((0.0, 100.0)),
        step: neighbour
            .map(|neighbour| (neighbour - current).abs())
            .filter(|step| *step > 0.0),
    })
}

fn translate_set_selection(
    node: &FlutterSemanticsNode,
    selection: &TextSelection,
) -> Option<SemanticsActionRequest> {
    if !node.flags.is_text_field || !node.actions.contains(FlutterSemanticsAction::SetSelection) {
        return None;
    }

    let text = TextLayout::new(&node.value);
    let utf16_offset = |position: &TextPosition| -> Option<i32> {
        let (field, run) = text_run_of(position.node)?;
        if field != node.id {
            return None;
        }
        i32::try_from(text.utf16_offset(run, position.character_index)?).ok()
    };

    let arguments = Value::Map(HashMap::from([
        (
            "base".to_owned(),
            Value::I32(utf16_offset(&selection.anchor)?),
        ),
        (
            "extent".to_owned(),
            Value::I32(utf16_offset(&selection.focus)?),
        ),
    ]));
    Some(SemanticsActionRequest {
        node_id: node.id,
        action: FlutterSemanticsAction::SetSelection,
        data: STANDARD_CODEC.encode_message(&arguments),
    })
}

fn translate_set_text(node: &FlutterSemanticsNode, text: &str) -> Option<SemanticsActionRequest> {
    if !node.actions.contains(FlutterSemanticsAction::SetText) {
        return None;
    }

    Some(SemanticsActionRequest {
        node_id: node.id,
        action: FlutterSemanticsAction::SetText,
        data: STANDARD_CODEC.encode_message(&Value::String(text.to_owned())),
    })
}

/// Flutter can not be asked to set a slider to a value, only to move it a
/// step. So this moves it one step towards the value.
fn translate_set_numeric_value(
    node: &FlutterSemanticsNode,
    value: f64,
) -> Option<SemanticsActionRequest> {
    let current = adjustable_value(node)?.current;
    let action = if value > current {
        FlutterSemanticsAction::Increase
    } else if value < current {
        FlutterSemanticsAction::Decrease
    } else {
        return None;
    };
    if !node.actions.contains(action) {
        return None;
    }

    Some(SemanticsActionRequest {
        node_id: node.id,
        action,
        data: Vec::new(),
    })
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

    fn text_field(id: i32, value: &str, selection: (i32, i32)) -> FlutterSemanticsNode {
        FlutterSemanticsNode {
            flags: FlutterSemanticsFlags {
                is_text_field: true,
                is_multiline: true,
                is_focused: FlutterTristate::True,
                ..Default::default()
            },
            actions: [
                FlutterSemanticsAction::Tap,
                FlutterSemanticsAction::SetSelection,
                FlutterSemanticsAction::SetText,
            ]
            .into_iter()
            .collect(),
            value: value.into(),
            text_selection_base: selection.0,
            text_selection_extent: selection.1,
            ..labelled(id, "Message")
        }
    }

    fn decode(data: &[u8]) -> Value {
        STANDARD_CODEC.decode_message(data).unwrap()
    }

    #[test]
    fn the_text_of_a_text_field_is_in_runs() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![
            node(0, &[1]),
            text_field(1, "Hi there\nBye", (3, 10)),
        ]));

        let first = text_run_id(1, 0);
        let second = text_run_id(1, 1);
        assert_eq!(ids(&update), [0, 1, WINDOW_NODE_ID.0, first.0, second.0]);

        let field = find(&update, NodeId(1));
        assert_eq!(field.role(), Role::MultilineTextInput);
        assert_eq!(field.label(), Some("Message"));
        assert_eq!(field.value(), None);
        assert_eq!(field.children(), [first, second]);
        assert_eq!(
            field.text_selection(),
            Some(&TextSelection {
                anchor: TextPosition {
                    node: first,
                    character_index: 3,
                },
                focus: TextPosition {
                    node: second,
                    character_index: 1,
                },
            })
        );
        assert!(field.supports_action(Action::SetTextSelection));
        assert!(field.supports_action(Action::SetValue));

        let run = find(&update, first);
        assert_eq!(run.role(), Role::TextRun);
        assert_eq!(run.value(), Some("Hi there\n"));
        assert_eq!(run.character_lengths(), [1; 9]);
        assert_eq!(run.word_starts(), [3]);
        assert_eq!(run.next_on_line(), None);
        assert_eq!(find(&update, second).value(), Some("Bye"));
        assert_eq!(find(&update, second).previous_on_line(), None);

        assert_eq!(update.focus, NodeId(1));
    }

    #[test]
    fn a_text_field_without_text_or_selection() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![node(0, &[1]), text_field(1, "", (-1, -1))]));

        let field = find(&update, NodeId(1));
        assert_eq!(field.children(), [text_run_id(1, 0)]);
        assert_eq!(field.text_selection(), None);
        assert_eq!(find(&update, text_run_id(1, 0)).value(), Some(""));
    }

    #[test]
    fn a_text_field_keeps_its_own_children() {
        let mut tree = SemanticsTree::new("Probe");
        let mut field = text_field(1, "Hi", (0, 0));
        field.children_in_traversal_order = vec![2];

        let update = tree.apply(update(vec![node(0, &[1]), field, button(2, "Clear")]));

        assert_eq!(
            find(&update, NodeId(1)).children(),
            [text_run_id(1, 0), NodeId(2)]
        );
    }

    #[test]
    fn runs_that_are_no_longer_there_are_not_children_anymore() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1]),
            text_field(1, "one\ntwo", (0, 0)),
        ]));

        let update = tree.apply(update(vec![text_field(1, "one", (0, 0))]));

        assert_eq!(ids(&update), [1, text_run_id(1, 0).0]);
        assert_eq!(find(&update, NodeId(1)).children(), [text_run_id(1, 0)]);
    }

    #[test]
    fn long_lines_are_linked_runs() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![
            node(0, &[1]),
            text_field(1, &"a".repeat(300), (-1, -1)),
        ]));

        let first = find(&update, text_run_id(1, 0));
        let second = find(&update, text_run_id(1, 1));
        assert_eq!(first.next_on_line(), Some(text_run_id(1, 1)));
        assert_eq!(second.previous_on_line(), Some(text_run_id(1, 0)));
        assert_eq!(second.next_on_line(), None);
    }

    #[test]
    fn setting_the_selection_of_a_text_field() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1, 2]),
            text_field(1, "a😀b\ncd", (0, 0)),
            text_field(2, "other", (0, 0)),
        ]));
        let select = |anchor: TextPosition, focus: TextPosition| {
            tree.translate_action(&request(
                Action::SetTextSelection,
                NodeId(1),
                Some(ActionData::SetTextSelection(TextSelection {
                    anchor,
                    focus,
                })),
            ))
        };
        let position = |field: i32, run: usize, character_index: usize| TextPosition {
            node: text_run_id(field, run),
            character_index,
        };

        let selected = select(position(1, 0, 2), position(1, 1, 1)).unwrap();
        assert_eq!(selected.node_id, 1);
        assert_eq!(selected.action, FlutterSemanticsAction::SetSelection);
        // In UTF-16 code units: the emoji is two of them.
        assert_eq!(
            decode(&selected.data),
            Value::Map(HashMap::from([
                ("base".to_owned(), Value::I32(3)),
                ("extent".to_owned(), Value::I32(6)),
            ]))
        );

        // A position in another text field, or one that is not there.
        assert_eq!(select(position(1, 0, 0), position(2, 0, 1)), None);
        assert_eq!(select(position(1, 0, 0), position(1, 0, 9)), None);
        assert_eq!(select(position(1, 0, 0), position(1, 5, 0)), None);
    }

    #[test]
    fn setting_the_text_of_a_text_field() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![
            node(0, &[1, 2]),
            text_field(1, "old", (0, 0)),
            button(2, "Send"),
        ]));
        let set = |target: u64| {
            tree.translate_action(&request(
                Action::SetValue,
                NodeId(target),
                Some(ActionData::Value("new".into())),
            ))
        };

        let request = set(1).unwrap();
        assert_eq!(request.action, FlutterSemanticsAction::SetText);
        assert_eq!(decode(&request.data), Value::String("new".into()));

        assert_eq!(set(2), None);
    }

    fn slider(id: i32, value: &str, increased: &str, decreased: &str) -> FlutterSemanticsNode {
        FlutterSemanticsNode {
            flags: FlutterSemanticsFlags {
                is_slider: true,
                ..Default::default()
            },
            actions: [
                FlutterSemanticsAction::Increase,
                FlutterSemanticsAction::Decrease,
            ]
            .into_iter()
            .collect(),
            value: value.into(),
            increased_value: increased.into(),
            decreased_value: decreased.into(),
            ..labelled(id, "Volume")
        }
    }

    #[test]
    fn a_slider_has_a_number() {
        let mut tree = SemanticsTree::new("Probe");

        let update = tree.apply(update(vec![
            node(0, &[1, 2]),
            slider(1, "50%", "55%", "45%"),
            slider(2, "Loud", "Louder", "Quieter"),
        ]));

        let volume = find(&update, NodeId(1));
        assert_eq!(volume.role(), Role::Slider);
        assert_eq!(volume.value(), Some("50%"));
        assert_eq!(volume.numeric_value(), Some(50.0));
        assert_eq!(volume.min_numeric_value(), Some(0.0));
        assert_eq!(volume.max_numeric_value(), Some(100.0));
        assert_eq!(volume.numeric_value_step(), Some(5.0));
        assert!(volume.supports_action(Action::SetValue));
        assert!(volume.supports_action(Action::Increment));

        // A value that is not a number is only text.
        let named = find(&update, NodeId(2));
        assert_eq!(named.value(), Some("Loud"));
        assert_eq!(named.numeric_value(), None);
        assert!(!named.supports_action(Action::SetValue));
    }

    #[test]
    fn setting_the_number_of_a_slider_moves_it_a_step() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![node(0, &[1]), slider(1, "50%", "55%", "45%")]));
        let set = |value: f64| {
            tree.translate_action(&request(
                Action::SetValue,
                NodeId(1),
                Some(ActionData::NumericValue(value)),
            ))
            .map(|request| request.action)
        };

        assert_eq!(set(80.0), Some(FlutterSemanticsAction::Increase));
        assert_eq!(set(10.0), Some(FlutterSemanticsAction::Decrease));
        assert_eq!(set(50.0), None);
    }

    #[test]
    fn an_announcement_is_a_node_that_comes_and_goes() {
        let mut tree = SemanticsTree::new("Probe");
        tree.apply(update(vec![node(0, &[1]), button(1, "Send")]));

        let [added, removed] = tree.announce("Sent", false);

        assert_eq!(ids(&added), [WINDOW_NODE_ID.0, ANNOUNCEMENT_NODE_ID.0]);
        assert_eq!(
            find(&added, WINDOW_NODE_ID).children(),
            [NodeId(0), ANNOUNCEMENT_NODE_ID]
        );
        let announcement = find(&added, ANNOUNCEMENT_NODE_ID);
        assert_eq!(announcement.role(), Role::Label);
        assert_eq!(announcement.value(), Some("Sent"));
        assert_eq!(announcement.live(), Some(Live::Polite));

        assert_eq!(ids(&removed), [WINDOW_NODE_ID.0]);
        assert_eq!(find(&removed, WINDOW_NODE_ID).children(), [NodeId(0)]);

        let [urgent, _] = tree.announce("Failed", true);
        assert_eq!(
            find(&urgent, ANNOUNCEMENT_NODE_ID).live(),
            Some(Live::Assertive)
        );
        // It is not a text run, and not one of Flutter's nodes either.
        assert_eq!(text_run_of(ANNOUNCEMENT_NODE_ID), None);
        assert_eq!(
            tree.translate_action(&request(Action::Click, ANNOUNCEMENT_NODE_ID, None)),
            None
        );
    }
}
