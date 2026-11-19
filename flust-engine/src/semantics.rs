//! The semantics tree of a Flutter view, as the engine reports it to the
//! embedder, and what the embedder can send back.
//!
//! The types in here own their data. The engine's own structs only live for
//! the duration of the callback they are passed to.

use std::{
    ffi::{c_char, CStr},
    mem::{offset_of, size_of},
    ptr::addr_of,
    slice,
};

use crate::ffi::{FlutterViewId, IMPLICIT_VIEW_ID};

/// Receives the semantics updates of the engine's views.
///
/// Updates are only sent while semantics is enabled, see
/// [`FlutterEngine::update_semantics_enabled`](crate::FlutterEngine::update_semantics_enabled).
pub trait FlutterSemanticsHandler {
    /// Called on the platform thread.
    fn update_semantics(&self, update: FlutterSemanticsUpdate);
}

/// Defines an enum of single bits, and a set of them.
macro_rules! bit_set {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident, set $set:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $bit:expr,)+
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u32)]
        pub enum $name {
            $($(#[$variant_meta])* $variant = 1 << $bit,)+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];

            #[inline]
            pub const fn bits(self) -> u32 {
                self as u32
            }
        }

        #[doc = concat!("A set of [`", stringify!($name), "`] values.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $set(u32);

        impl $set {
            #[inline]
            pub const fn empty() -> Self {
                Self(0)
            }

            /// Bits that are not a known value are kept: the engine may be
            /// newer than this crate.
            #[inline]
            pub const fn from_bits(bits: u32) -> Self {
                Self(bits)
            }

            #[inline]
            pub const fn bits(self) -> u32 {
                self.0
            }

            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            #[inline]
            pub const fn contains(self, value: $name) -> bool {
                self.0 & value.bits() != 0
            }

            #[inline]
            pub fn insert(&mut self, value: $name) {
                self.0 |= value.bits();
            }

            #[inline]
            pub fn remove(&mut self, value: $name) {
                self.0 &= !value.bits();
            }

            pub fn iter(self) -> impl Iterator<Item = $name> {
                $name::ALL.iter().copied().filter(move |value| self.contains(*value))
            }
        }

        impl From<$name> for $set {
            fn from(value: $name) -> Self {
                Self(value.bits())
            }
        }

        impl FromIterator<$name> for $set {
            fn from_iter<I: IntoIterator<Item = $name>>(iter: I) -> Self {
                let mut set = Self::empty();
                for value in iter {
                    set.insert(value);
                }
                set
            }
        }

        impl std::fmt::Debug for $set {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_set().entries(self.iter()).finish()
            }
        }
    };
}

bit_set! {
    /// Something a semantics node can be asked to do.
    ///
    /// Matches `SemanticsAction` in Flutter's `semantics.dart`.
    pub enum FlutterSemanticsAction, set FlutterSemanticsActions {
        Tap = 0,
        LongPress = 1,
        ScrollLeft = 2,
        ScrollRight = 3,
        ScrollUp = 4,
        ScrollDown = 5,
        Increase = 6,
        Decrease = 7,
        ShowOnScreen = 8,
        MoveCursorForwardByCharacter = 9,
        MoveCursorBackwardByCharacter = 10,
        SetSelection = 11,
        Copy = 12,
        Cut = 13,
        Paste = 14,
        DidGainAccessibilityFocus = 15,
        DidLoseAccessibilityFocus = 16,
        CustomAction = 17,
        Dismiss = 18,
        MoveCursorForwardByWord = 19,
        MoveCursorBackwardByWord = 20,
        SetText = 21,
        Focus = 22,
        ScrollToOffset = 23,
        Expand = 24,
        Collapse = 25,
    }
}

impl FlutterSemanticsAction {
    fn from_bits(bits: u32) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.bits() == bits)
    }
}

impl From<FlutterSemanticsAction> for flust_engine_sys::FlutterSemanticsAction {
    fn from(action: FlutterSemanticsAction) -> Self {
        Self(action.bits())
    }
}

bit_set! {
    /// An accessibility setting of the platform that the framework adapts
    /// to.
    ///
    /// Matches `AccessibilityFeatures` in Flutter's `window.dart`.
    pub enum FlutterAccessibilityFeature, set FlutterAccessibilityFeatures {
        /// An assistive technology, such as a screen reader, is in use.
        AccessibleNavigation = 0,
        InvertColors = 1,
        DisableAnimations = 2,
        BoldText = 3,
        ReduceMotion = 4,
        HighContrast = 5,
        OnOffSwitchLabels = 6,
        NoAnnounce = 7,
        NoAutoPlayAnimatedImages = 8,
        NoAutoPlayVideos = 9,
        DeterministicCursor = 10,
    }
}

impl From<FlutterAccessibilityFeatures> for flust_engine_sys::FlutterAccessibilityFeature {
    fn from(features: FlutterAccessibilityFeatures) -> Self {
        Self(features.bits())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlutterTristate {
    /// The node does not have this property.
    #[default]
    None,
    True,
    False,
}

impl FlutterTristate {
    fn from_ffi(value: u32) -> Self {
        const TRUE: u32 = flust_engine_sys::FlutterTristate::kFlutterTristateTrue as u32;
        const FALSE: u32 = flust_engine_sys::FlutterTristate::kFlutterTristateFalse as u32;

        match value {
            TRUE => Self::True,
            FALSE => Self::False,
            _ => Self::None,
        }
    }

    /// Whether the node has this property at all.
    pub fn is_some(self) -> bool {
        self != Self::None
    }

    pub fn is_true(self) -> bool {
        self == Self::True
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlutterCheckState {
    /// The node can not be checked.
    #[default]
    None,
    True,
    False,
    Mixed,
}

impl FlutterCheckState {
    fn from_ffi(value: u32) -> Self {
        const TRUE: u32 = flust_engine_sys::FlutterCheckState::kFlutterCheckStateTrue as u32;
        const FALSE: u32 = flust_engine_sys::FlutterCheckState::kFlutterCheckStateFalse as u32;
        const MIXED: u32 = flust_engine_sys::FlutterCheckState::kFlutterCheckStateMixed as u32;

        match value {
            TRUE => Self::True,
            FALSE => Self::False,
            MIXED => Self::Mixed,
            _ => Self::None,
        }
    }

    /// Whether the node can be checked at all.
    pub fn is_some(self) -> bool {
        self != Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlutterTextDirection {
    #[default]
    Unknown,
    Rtl,
    Ltr,
}

impl FlutterTextDirection {
    fn from_ffi(value: u32) -> Self {
        const RTL: u32 = flust_engine_sys::FlutterTextDirection::kFlutterTextDirectionRTL as u32;
        const LTR: u32 = flust_engine_sys::FlutterTextDirection::kFlutterTextDirectionLTR as u32;

        match value {
            RTL => Self::Rtl,
            LTR => Self::Ltr,
            _ => Self::Unknown,
        }
    }
}

/// What a semantics node is, and the state it is in.
///
/// Matches `SemanticsFlags` in Flutter's `semantics.dart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FlutterSemanticsFlags {
    pub is_checked: FlutterCheckState,
    pub is_selected: FlutterTristate,
    pub is_enabled: FlutterTristate,
    pub is_toggled: FlutterTristate,
    pub is_expanded: FlutterTristate,
    pub is_required: FlutterTristate,
    pub is_focused: FlutterTristate,
    pub is_button: bool,
    pub is_text_field: bool,
    pub is_in_mutually_exclusive_group: bool,
    pub is_header: bool,
    pub is_obscured: bool,
    pub scopes_route: bool,
    pub names_route: bool,
    pub is_hidden: bool,
    pub is_image: bool,
    pub is_live_region: bool,
    pub has_implicit_scrolling: bool,
    pub is_multiline: bool,
    pub is_read_only: bool,
    pub is_link: bool,
    pub is_slider: bool,
    pub is_keyboard_key: bool,
    pub is_accessibility_focus_blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlutterRect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl From<flust_engine_sys::FlutterRect> for FlutterRect {
    fn from(rect: flust_engine_sys::FlutterRect) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

/// A 3x3 transformation matrix, in row-major order:
///
/// ```text
/// | scale_x  skew_x   trans_x |
/// | skew_y   scale_y  trans_y |
/// | pers0    pers1    pers2   |
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlutterTransformation {
    pub scale_x: f64,
    pub skew_x: f64,
    pub trans_x: f64,
    pub skew_y: f64,
    pub scale_y: f64,
    pub trans_y: f64,
    pub pers0: f64,
    pub pers1: f64,
    pub pers2: f64,
}

impl FlutterTransformation {
    pub const IDENTITY: Self = Self {
        scale_x: 1.0,
        skew_x: 0.0,
        trans_x: 0.0,
        skew_y: 0.0,
        scale_y: 1.0,
        trans_y: 0.0,
        pers0: 0.0,
        pers1: 0.0,
        pers2: 1.0,
    };
}

impl Default for FlutterTransformation {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<flust_engine_sys::FlutterTransformation> for FlutterTransformation {
    fn from(transform: flust_engine_sys::FlutterTransformation) -> Self {
        Self {
            scale_x: transform.scaleX,
            skew_x: transform.skewX,
            trans_x: transform.transX,
            skew_y: transform.skewY,
            scale_y: transform.scaleY,
            trans_y: transform.transY,
            pers0: transform.pers0,
            pers1: transform.pers1,
            pers2: transform.pers2,
        }
    }
}

/// How an assistive technology should announce a part of a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlutterStringAttribute {
    /// Where the part starts in the string.
    pub start: usize,
    /// The position after the last one of the part.
    pub end: usize,
    pub kind: FlutterStringAttributeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FlutterStringAttributeKind {
    /// Announce the part character by character.
    SpellOut,
    /// Announce the part in this locale.
    Locale(String),
}

/// A node of a view's semantics tree.
///
/// See <https://api.flutter.dev/flutter/semantics/SemanticsNode-class.html>.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlutterSemanticsNode {
    /// Unique within the view. The root node has the id `0`.
    pub id: i32,
    pub flags: FlutterSemanticsFlags,
    /// What the node can be asked to do.
    pub actions: FlutterSemanticsActions,
    /// Where the text selection starts, or `-1` if there is none.
    pub text_selection_base: i32,
    /// Where the text selection ends, or `-1` if there is none.
    pub text_selection_extent: i32,
    /// How many scrollable children contribute to semantics.
    pub scroll_child_count: i32,
    /// The index of the first visible semantic child of a scrollable node.
    pub scroll_index: i32,
    /// The scroll position in logical pixels, if the node is scrollable.
    pub scroll_position: f64,
    pub scroll_extent_max: f64,
    pub scroll_extent_min: f64,
    pub elevation: f64,
    pub thickness: f64,
    /// A description of the node.
    pub label: String,
    /// A short description of what performing an action on the node does.
    pub hint: String,
    /// A description of the current value of the node.
    pub value: String,
    /// What `value` will be after [`FlutterSemanticsAction::Increase`].
    pub increased_value: String,
    /// What `value` will be after [`FlutterSemanticsAction::Decrease`].
    pub decreased_value: String,
    pub tooltip: String,
    /// The reading direction of the strings of this node.
    pub text_direction: FlutterTextDirection,
    /// The bounding box of the node in its own coordinate system.
    pub rect: FlutterRect,
    /// From the coordinate system of the node to that of its parent.
    pub transform: FlutterTransformation,
    pub children_in_traversal_order: Vec<i32>,
    pub children_in_hit_test_order: Vec<i32>,
    /// The ids of the [`FlutterSemanticsCustomAction`]s of this node.
    pub custom_accessibility_actions: Vec<i32>,
    /// The platform view this node stands for, or `-1`.
    pub platform_view_id: i64,
    pub label_attributes: Vec<FlutterStringAttribute>,
    pub hint_attributes: Vec<FlutterStringAttribute>,
    pub value_attributes: Vec<FlutterStringAttribute>,
    pub increased_value_attributes: Vec<FlutterStringAttribute>,
    pub decreased_value_attributes: Vec<FlutterStringAttribute>,
    /// `0` if the node is not a heading. `1` is the highest level.
    pub heading_level: i32,
    /// Identifies the node in the platform's accessibility tree, for tools
    /// that test through it. Not meant to be shown to users.
    pub identifier: String,
}

/// An action an application defines for a node, or the label it gives to
/// one of the standard actions.
///
/// See <https://api.flutter.dev/flutter/semantics/CustomSemanticsAction-class.html>.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlutterSemanticsCustomAction {
    pub id: i32,
    /// The standard action this one stands in for, if any.
    pub override_action: Option<FlutterSemanticsAction>,
    pub label: String,
    pub hint: String,
}

/// A change to the semantics tree of a view.
///
/// Lists the nodes that are new or have changed, each of them in full. A
/// node that is no longer a child of any other node has been removed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlutterSemanticsUpdate {
    pub view_id: FlutterViewId,
    pub nodes: Vec<FlutterSemanticsNode>,
    pub custom_actions: Vec<FlutterSemanticsCustomAction>,
}

/// Reads a field of a struct the engine passed in, or `None` if the engine's
/// version of the struct ends before it.
///
/// The engine is a shared library, and may be older than the header these
/// bindings were generated from. It says how much of a struct it filled in
/// through `struct_size`. No reference to the struct is made, as it may be
/// shorter than its type.
macro_rules! read_field {
    ($ptr:expr, $ty:ty, $field:ident) => {
        read_field!($ptr, $ty, $field, (*$ptr).struct_size)
    };
    ($ptr:expr, $ty:ty, $field:ident, $struct_size:expr) => {{
        let ptr: *const $ty = $ptr;
        let field = addr_of!((*ptr).$field);
        if $struct_size >= offset_of!($ty, $field) + size_of_pointee(field) {
            Some(field.read())
        } else {
            None
        }
    }};
}

/// Like [`read_field`], for a field that is a C enum. It is read as a
/// number, since a newer engine may send a value that the Rust enum does not
/// have.
macro_rules! read_enum_field {
    ($ptr:expr, $ty:ty, $field:ident) => {
        read_enum_field!($ptr, $ty, $field, (*$ptr).struct_size)
    };
    ($ptr:expr, $ty:ty, $field:ident, $struct_size:expr) => {{
        let ptr: *const $ty = $ptr;
        let field = addr_of!((*ptr).$field);
        if $struct_size >= offset_of!($ty, $field) + size_of_pointee(field) {
            Some(field.cast::<u32>().read())
        } else {
            None
        }
    }};
}

fn size_of_pointee<T>(_: *const T) -> usize {
    size_of::<T>()
}

unsafe fn string_from_ffi(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

unsafe fn slice_from_ffi<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
    slice::from_raw_parts(ptr, len)
}

impl FlutterSemanticsFlags {
    unsafe fn from_ffi(raw: *const flust_engine_sys::FlutterSemanticsFlags) -> Self {
        type Raw = flust_engine_sys::FlutterSemanticsFlags;

        if raw.is_null() {
            return Self::default();
        }

        // The engine leaves the size of this struct at zero (as of Flutter
        // 3.50), so there is no telling how much of it an engine has. It is
        // taken to have all of it.
        let struct_size = match (*raw).struct_size {
            0 => size_of::<Raw>(),
            struct_size => struct_size,
        };

        macro_rules! tristate {
            ($field:ident) => {
                read_enum_field!(raw, Raw, $field, struct_size)
                    .map(FlutterTristate::from_ffi)
                    .unwrap_or_default()
            };
        }
        macro_rules! flag {
            ($field:ident) => {
                read_field!(raw, Raw, $field, struct_size).unwrap_or_default()
            };
        }

        Self {
            is_checked: read_enum_field!(raw, Raw, is_checked, struct_size)
                .map(FlutterCheckState::from_ffi)
                .unwrap_or_default(),
            is_selected: tristate!(is_selected),
            is_enabled: tristate!(is_enabled),
            is_toggled: tristate!(is_toggled),
            is_expanded: tristate!(is_expanded),
            is_required: tristate!(is_required),
            is_focused: tristate!(is_focused),
            is_button: flag!(is_button),
            is_text_field: flag!(is_text_field),
            is_in_mutually_exclusive_group: flag!(is_in_mutually_exclusive_group),
            is_header: flag!(is_header),
            is_obscured: flag!(is_obscured),
            scopes_route: flag!(scopes_route),
            names_route: flag!(names_route),
            is_hidden: flag!(is_hidden),
            is_image: flag!(is_image),
            is_live_region: flag!(is_live_region),
            has_implicit_scrolling: flag!(has_implicit_scrolling),
            is_multiline: flag!(is_multiline),
            is_read_only: flag!(is_read_only),
            is_link: flag!(is_link),
            is_slider: flag!(is_slider),
            is_keyboard_key: flag!(is_keyboard_key),
            is_accessibility_focus_blocked: flag!(is_accessibility_focus_blocked),
        }
    }
}

impl FlutterStringAttribute {
    unsafe fn from_ffi(raw: *const flust_engine_sys::FlutterStringAttribute) -> Option<Self> {
        type Raw = flust_engine_sys::FlutterStringAttribute;
        const LOCALE: u32 = flust_engine_sys::FlutterStringAttributeType::kLocale as u32;
        const SPELL_OUT: u32 = flust_engine_sys::FlutterStringAttributeType::kSpellOut as u32;

        if raw.is_null() {
            return None;
        }

        let kind = match read_enum_field!(raw, Raw, type_)? {
            SPELL_OUT => FlutterStringAttributeKind::SpellOut,
            LOCALE => {
                let locale = (*raw).__bindgen_anon_1.locale;
                if locale.is_null() {
                    return None;
                }
                FlutterStringAttributeKind::Locale(string_from_ffi((*locale).locale))
            }
            _ => return None,
        };

        Some(Self {
            start: (*raw).start,
            end: (*raw).end,
            kind,
        })
    }

    unsafe fn vec_from_ffi(
        attributes: *const *const flust_engine_sys::FlutterStringAttribute,
        count: usize,
    ) -> Vec<Self> {
        slice_from_ffi(attributes, count)
            .iter()
            .filter_map(|attribute| Self::from_ffi(*attribute))
            .collect()
    }
}

impl FlutterSemanticsNode {
    /// # Safety
    ///
    /// `raw` has to point to a node the engine passed to the semantics
    /// update callback, during that callback.
    unsafe fn from_ffi(raw: *const flust_engine_sys::FlutterSemanticsNode2) -> Self {
        type Raw = flust_engine_sys::FlutterSemanticsNode2;

        // The fields up to `decreased_value_attributes` have been there
        // since the engine first had this struct, and are read as they are.
        // The ones after it were added later. `node` is a place, not a
        // reference: the engine's struct may be shorter than `Raw`.
        macro_rules! node {
            ($field:ident) => {
                (*raw).$field
            };
        }

        let flags = read_field!(raw, Raw, flags2)
            .map(|flags| FlutterSemanticsFlags::from_ffi(flags))
            .unwrap_or_default();

        Self {
            id: node!(id),
            flags,
            actions: FlutterSemanticsActions::from_bits(node!(actions).0),
            text_selection_base: node!(text_selection_base),
            text_selection_extent: node!(text_selection_extent),
            scroll_child_count: node!(scroll_child_count),
            scroll_index: node!(scroll_index),
            scroll_position: node!(scroll_position),
            scroll_extent_max: node!(scroll_extent_max),
            scroll_extent_min: node!(scroll_extent_min),
            elevation: node!(elevation),
            thickness: node!(thickness),
            label: string_from_ffi(node!(label)),
            hint: string_from_ffi(node!(hint)),
            value: string_from_ffi(node!(value)),
            increased_value: string_from_ffi(node!(increased_value)),
            decreased_value: string_from_ffi(node!(decreased_value)),
            tooltip: string_from_ffi(node!(tooltip)),
            text_direction: read_enum_field!(raw, Raw, text_direction)
                .map(FlutterTextDirection::from_ffi)
                .unwrap_or_default(),
            rect: node!(rect).into(),
            transform: node!(transform).into(),
            children_in_traversal_order: slice_from_ffi(
                node!(children_in_traversal_order),
                node!(child_count),
            )
            .to_vec(),
            children_in_hit_test_order: slice_from_ffi(
                node!(children_in_hit_test_order),
                node!(child_count),
            )
            .to_vec(),
            custom_accessibility_actions: slice_from_ffi(
                node!(custom_accessibility_actions),
                node!(custom_accessibility_actions_count),
            )
            .to_vec(),
            platform_view_id: node!(platform_view_id),
            label_attributes: FlutterStringAttribute::vec_from_ffi(
                node!(label_attributes),
                node!(label_attribute_count),
            ),
            hint_attributes: FlutterStringAttribute::vec_from_ffi(
                node!(hint_attributes),
                node!(hint_attribute_count),
            ),
            value_attributes: FlutterStringAttribute::vec_from_ffi(
                node!(value_attributes),
                node!(value_attribute_count),
            ),
            increased_value_attributes: FlutterStringAttribute::vec_from_ffi(
                node!(increased_value_attributes),
                node!(increased_value_attribute_count),
            ),
            decreased_value_attributes: FlutterStringAttribute::vec_from_ffi(
                node!(decreased_value_attributes),
                node!(decreased_value_attribute_count),
            ),
            heading_level: read_field!(raw, Raw, heading_level).unwrap_or_default(),
            identifier: read_field!(raw, Raw, identifier)
                .map(|identifier| string_from_ffi(identifier))
                .unwrap_or_default(),
        }
    }
}

impl FlutterSemanticsCustomAction {
    /// # Safety
    ///
    /// As for [`FlutterSemanticsNode::from_ffi`].
    unsafe fn from_ffi(raw: *const flust_engine_sys::FlutterSemanticsCustomAction2) -> Self {
        Self {
            id: (*raw).id,
            override_action: FlutterSemanticsAction::from_bits((*raw).override_action.0),
            label: string_from_ffi((*raw).label),
            hint: string_from_ffi((*raw).hint),
        }
    }
}

impl FlutterSemanticsUpdate {
    /// # Safety
    ///
    /// `raw` has to be the update the engine passed to the semantics update
    /// callback, during that callback.
    pub(crate) unsafe fn from_ffi(raw: *const flust_engine_sys::FlutterSemanticsUpdate2) -> Self {
        type Raw = flust_engine_sys::FlutterSemanticsUpdate2;

        let nodes = slice_from_ffi((*raw).nodes, (*raw).node_count)
            .iter()
            .filter(|node| !node.is_null())
            .map(|node| FlutterSemanticsNode::from_ffi(*node))
            .collect();

        let custom_actions = slice_from_ffi((*raw).custom_actions, (*raw).custom_action_count)
            .iter()
            .filter(|action| !action.is_null())
            .map(|action| FlutterSemanticsCustomAction::from_ffi(*action))
            .collect();

        Self {
            // Engines from before there were multiple views do not say
            // which view an update is for.
            view_id: read_field!(raw, Raw, view_id).unwrap_or(IMPLICIT_VIEW_ID),
            nodes,
            custom_actions,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::CString, mem::zeroed, ptr};

    use flust_engine_sys as sys;

    use super::*;

    fn raw_node(id: i32) -> sys::FlutterSemanticsNode2 {
        let mut node: sys::FlutterSemanticsNode2 = unsafe { zeroed() };
        node.struct_size = size_of::<sys::FlutterSemanticsNode2>();
        node.id = id;
        node.text_selection_base = -1;
        node.text_selection_extent = -1;
        node.platform_view_id = -1;
        node
    }

    fn raw_flags() -> sys::FlutterSemanticsFlags {
        let mut flags: sys::FlutterSemanticsFlags = unsafe { zeroed() };
        flags.struct_size = size_of::<sys::FlutterSemanticsFlags>();
        flags
    }

    #[test]
    fn action_set() {
        let actions = FlutterSemanticsActions::from_bits(
            (sys::FlutterSemanticsAction::kFlutterSemanticsActionTap
                | sys::FlutterSemanticsAction::kFlutterSemanticsActionCollapse)
                .0,
        );

        assert!(actions.contains(FlutterSemanticsAction::Tap));
        assert!(actions.contains(FlutterSemanticsAction::Collapse));
        assert!(!actions.contains(FlutterSemanticsAction::LongPress));
        assert_eq!(
            actions.iter().collect::<Vec<_>>(),
            [
                FlutterSemanticsAction::Tap,
                FlutterSemanticsAction::Collapse
            ]
        );
        assert_eq!(
            actions,
            [
                FlutterSemanticsAction::Tap,
                FlutterSemanticsAction::Collapse
            ]
            .into_iter()
            .collect()
        );
        assert!(FlutterSemanticsActions::empty().is_empty());
    }

    #[test]
    fn action_set_keeps_unknown_bits() {
        let actions = FlutterSemanticsActions::from_bits(1 << 31 | 1);

        assert_eq!(actions.bits(), 1 << 31 | 1);
        assert_eq!(
            actions.iter().collect::<Vec<_>>(),
            [FlutterSemanticsAction::Tap]
        );
    }

    #[test]
    fn actions_match_the_engine() {
        use sys::FlutterSemanticsAction as Raw;

        let expected = [
            (FlutterSemanticsAction::Tap, Raw::kFlutterSemanticsActionTap),
            (
                FlutterSemanticsAction::ShowOnScreen,
                Raw::kFlutterSemanticsActionShowOnScreen,
            ),
            (
                FlutterSemanticsAction::SetSelection,
                Raw::kFlutterSemanticsActionSetSelection,
            ),
            (
                FlutterSemanticsAction::CustomAction,
                Raw::kFlutterSemanticsActionCustomAction,
            ),
            (
                FlutterSemanticsAction::SetText,
                Raw::kFlutterSemanticsActionSetText,
            ),
            (
                FlutterSemanticsAction::Focus,
                Raw::kFlutterSemanticsActionFocus,
            ),
            (
                FlutterSemanticsAction::Collapse,
                Raw::kFlutterSemanticsActionCollapse,
            ),
        ];

        for (action, raw) in expected {
            assert_eq!(Raw::from(action), raw);
        }
    }

    #[test]
    fn accessibility_features_match_the_engine() {
        use sys::FlutterAccessibilityFeature as Raw;

        let features: FlutterAccessibilityFeatures = [
            FlutterAccessibilityFeature::AccessibleNavigation,
            FlutterAccessibilityFeature::HighContrast,
            FlutterAccessibilityFeature::DeterministicCursor,
        ]
        .into_iter()
        .collect();

        assert_eq!(
            Raw::from(features),
            Raw::kFlutterAccessibilityFeatureAccessibleNavigation
                | Raw::kFlutterAccessibilityFeatureHighContrast
                | Raw::kFlutterAccessibilityFeatureDeterministicCursor
        );
    }

    #[test]
    fn node_from_ffi() {
        let label = CString::new("Send").unwrap();
        let hint = CString::new("Sends the message").unwrap();
        let value = CString::new("50%").unwrap();
        let tooltip = CString::new("Tooltip").unwrap();
        let identifier = CString::new("send-button").unwrap();
        let locale = CString::new("fr-FR").unwrap();

        let traversal = [2, 3];
        let hit_test = [3, 2];
        let custom_actions = [7];

        let spell_out_data = sys::FlutterSpellOutStringAttribute {
            struct_size: size_of::<sys::FlutterSpellOutStringAttribute>(),
        };
        let locale_data = sys::FlutterLocaleStringAttribute {
            struct_size: size_of::<sys::FlutterLocaleStringAttribute>(),
            locale: locale.as_ptr(),
        };
        let spell_out = sys::FlutterStringAttribute {
            struct_size: size_of::<sys::FlutterStringAttribute>(),
            start: 0,
            end: 2,
            type_: sys::FlutterStringAttributeType::kSpellOut,
            __bindgen_anon_1: sys::FlutterStringAttribute__bindgen_ty_1 {
                spell_out: &spell_out_data,
            },
        };
        let in_locale = sys::FlutterStringAttribute {
            struct_size: size_of::<sys::FlutterStringAttribute>(),
            start: 2,
            end: 4,
            type_: sys::FlutterStringAttributeType::kLocale,
            __bindgen_anon_1: sys::FlutterStringAttribute__bindgen_ty_1 {
                locale: &locale_data,
            },
        };
        let mut label_attributes: [*const sys::FlutterStringAttribute; 2] =
            [&spell_out, &in_locale];

        let mut flags = raw_flags();
        flags.is_checked = sys::FlutterCheckState::kFlutterCheckStateMixed;
        flags.is_enabled = sys::FlutterTristate::kFlutterTristateTrue;
        flags.is_focused = sys::FlutterTristate::kFlutterTristateFalse;
        flags.is_button = true;
        flags.is_accessibility_focus_blocked = true;

        let mut raw = raw_node(1);
        raw.actions = sys::FlutterSemanticsAction::kFlutterSemanticsActionTap
            | sys::FlutterSemanticsAction::kFlutterSemanticsActionFocus;
        raw.text_selection_base = 1;
        raw.text_selection_extent = 4;
        raw.scroll_position = 12.5;
        raw.label = label.as_ptr();
        raw.hint = hint.as_ptr();
        raw.value = value.as_ptr();
        raw.tooltip = tooltip.as_ptr();
        raw.text_direction = sys::FlutterTextDirection::kFlutterTextDirectionRTL;
        raw.rect = sys::FlutterRect {
            left: 1.0,
            top: 2.0,
            right: 30.0,
            bottom: 40.0,
        };
        raw.transform = sys::FlutterTransformation {
            scaleX: 2.0,
            skewX: 0.0,
            transX: 10.0,
            skewY: 0.0,
            scaleY: 2.0,
            transY: 20.0,
            pers0: 0.0,
            pers1: 0.0,
            pers2: 1.0,
        };
        raw.child_count = traversal.len();
        raw.children_in_traversal_order = traversal.as_ptr();
        raw.children_in_hit_test_order = hit_test.as_ptr();
        raw.custom_accessibility_actions_count = custom_actions.len();
        raw.custom_accessibility_actions = custom_actions.as_ptr();
        raw.label_attribute_count = label_attributes.len();
        raw.label_attributes = label_attributes.as_mut_ptr();
        raw.flags2 = &mut flags;
        raw.heading_level = 2;
        raw.identifier = identifier.as_ptr();

        let node = unsafe { FlutterSemanticsNode::from_ffi(&raw) };

        assert_eq!(
            node,
            FlutterSemanticsNode {
                id: 1,
                flags: FlutterSemanticsFlags {
                    is_checked: FlutterCheckState::Mixed,
                    is_enabled: FlutterTristate::True,
                    is_focused: FlutterTristate::False,
                    is_button: true,
                    is_accessibility_focus_blocked: true,
                    ..Default::default()
                },
                actions: [FlutterSemanticsAction::Tap, FlutterSemanticsAction::Focus]
                    .into_iter()
                    .collect(),
                text_selection_base: 1,
                text_selection_extent: 4,
                scroll_position: 12.5,
                label: "Send".into(),
                hint: "Sends the message".into(),
                value: "50%".into(),
                tooltip: "Tooltip".into(),
                text_direction: FlutterTextDirection::Rtl,
                rect: FlutterRect {
                    left: 1.0,
                    top: 2.0,
                    right: 30.0,
                    bottom: 40.0,
                },
                transform: FlutterTransformation {
                    scale_x: 2.0,
                    trans_x: 10.0,
                    scale_y: 2.0,
                    trans_y: 20.0,
                    ..FlutterTransformation::IDENTITY
                },
                children_in_traversal_order: vec![2, 3],
                children_in_hit_test_order: vec![3, 2],
                custom_accessibility_actions: vec![7],
                platform_view_id: -1,
                label_attributes: vec![
                    FlutterStringAttribute {
                        start: 0,
                        end: 2,
                        kind: FlutterStringAttributeKind::SpellOut,
                    },
                    FlutterStringAttribute {
                        start: 2,
                        end: 4,
                        kind: FlutterStringAttributeKind::Locale("fr-FR".into()),
                    },
                ],
                heading_level: 2,
                identifier: "send-button".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn node_without_strings_or_children() {
        let raw = raw_node(4);

        let node = unsafe { FlutterSemanticsNode::from_ffi(&raw) };

        assert_eq!(node.id, 4);
        assert_eq!(node.label, "");
        assert_eq!(node.identifier, "");
        assert!(node.children_in_traversal_order.is_empty());
        assert!(node.label_attributes.is_empty());
        assert_eq!(node.flags, FlutterSemanticsFlags::default());
        assert!(node.actions.is_empty());
    }

    /// An engine from before `flags2`, `heading_level` and `identifier`
    /// were added leaves whatever is after its own end of the struct alone.
    #[test]
    fn node_from_an_older_engine() {
        let mut flags = raw_flags();
        flags.is_button = true;

        let mut raw = raw_node(1);
        raw.struct_size = offset_of!(sys::FlutterSemanticsNode2, flags2);
        raw.flags2 = &mut flags;
        raw.heading_level = 3;
        raw.identifier = ptr::dangling();

        let node = unsafe { FlutterSemanticsNode::from_ffi(&raw) };

        assert_eq!(node.flags, FlutterSemanticsFlags::default());
        assert_eq!(node.heading_level, 0);
        assert_eq!(node.identifier, "");
    }

    #[test]
    fn flags_from_an_older_engine() {
        let mut flags = raw_flags();
        flags.struct_size = offset_of!(sys::FlutterSemanticsFlags, is_accessibility_focus_blocked);
        flags.is_keyboard_key = true;
        flags.is_accessibility_focus_blocked = true;

        let flags = unsafe { FlutterSemanticsFlags::from_ffi(&flags) };

        assert!(flags.is_keyboard_key);
        assert!(!flags.is_accessibility_focus_blocked);
    }

    /// What the engine does, as of Flutter 3.50.
    #[test]
    fn flags_without_a_size() {
        let mut flags = raw_flags();
        flags.struct_size = 0;
        flags.is_checked = sys::FlutterCheckState::kFlutterCheckStateTrue;
        flags.is_button = true;
        flags.is_accessibility_focus_blocked = true;

        let flags = unsafe { FlutterSemanticsFlags::from_ffi(&flags) };

        assert_eq!(flags.is_checked, FlutterCheckState::True);
        assert!(flags.is_button);
        assert!(flags.is_accessibility_focus_blocked);
    }

    #[test]
    fn unknown_enum_values() {
        assert_eq!(FlutterTristate::from_ffi(99), FlutterTristate::None);
        assert_eq!(FlutterCheckState::from_ffi(99), FlutterCheckState::None);
        assert_eq!(
            FlutterTextDirection::from_ffi(99),
            FlutterTextDirection::Unknown
        );
    }

    #[test]
    fn update_from_ffi() {
        let label = CString::new("Copy link").unwrap();
        let hint = CString::new("Copies the link").unwrap();

        let mut root = raw_node(0);
        let children = [1];
        root.child_count = children.len();
        root.children_in_traversal_order = children.as_ptr();
        root.children_in_hit_test_order = children.as_ptr();
        let mut child = raw_node(1);
        let mut nodes: [*mut sys::FlutterSemanticsNode2; 2] = [&mut root, &mut child];

        let mut custom_action = sys::FlutterSemanticsCustomAction2 {
            struct_size: size_of::<sys::FlutterSemanticsCustomAction2>(),
            id: 7,
            override_action: sys::FlutterSemanticsAction(0),
            label: label.as_ptr(),
            hint: ptr::null(),
        };
        let mut override_action = sys::FlutterSemanticsCustomAction2 {
            struct_size: size_of::<sys::FlutterSemanticsCustomAction2>(),
            id: 8,
            override_action: sys::FlutterSemanticsAction::kFlutterSemanticsActionCopy,
            label: ptr::null(),
            hint: hint.as_ptr(),
        };
        let mut custom_actions: [*mut sys::FlutterSemanticsCustomAction2; 2] =
            [&mut custom_action, &mut override_action];

        let raw = sys::FlutterSemanticsUpdate2 {
            struct_size: size_of::<sys::FlutterSemanticsUpdate2>(),
            node_count: nodes.len(),
            nodes: nodes.as_mut_ptr(),
            custom_action_count: custom_actions.len(),
            custom_actions: custom_actions.as_mut_ptr(),
            view_id: 3,
        };

        let update = unsafe { FlutterSemanticsUpdate::from_ffi(&raw) };

        assert_eq!(update.view_id, 3);
        assert_eq!(
            update.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(update.nodes[0].children_in_traversal_order, [1]);
        assert_eq!(
            update.custom_actions,
            [
                FlutterSemanticsCustomAction {
                    id: 7,
                    override_action: None,
                    label: "Copy link".into(),
                    hint: String::new(),
                },
                FlutterSemanticsCustomAction {
                    id: 8,
                    override_action: Some(FlutterSemanticsAction::Copy),
                    label: String::new(),
                    hint: "Copies the link".into(),
                },
            ]
        );
    }

    #[test]
    fn empty_update_from_an_older_engine() {
        let raw = sys::FlutterSemanticsUpdate2 {
            struct_size: offset_of!(sys::FlutterSemanticsUpdate2, view_id),
            node_count: 0,
            nodes: ptr::null_mut(),
            custom_action_count: 0,
            custom_actions: ptr::null_mut(),
            view_id: 9,
        };

        let update = unsafe { FlutterSemanticsUpdate::from_ffi(&raw) };

        assert_eq!(update, FlutterSemanticsUpdate::default());
    }
}
