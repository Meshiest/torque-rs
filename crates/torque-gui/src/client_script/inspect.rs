//! A read-only look at what the realm's controls show now, for a game's tests and tools. It is Rust's alone: no script, and so no
//! server, reaches it, and it changes nothing.

use torque_client_script_format::ui::{Kind, MAX_CONTROLS, Prop};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::controls::NodeState;
use crate::control::{self, children};
use crate::{Gui, MAX_TREE_DEPTH};

/// One control of the realm that is shown: what it is, what it says and where it is on the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownControl {
    /// The control's kind.
    pub kind: Kind,
    /// Its text (a `text`, `button`, `toggle` or `textfield`'s; for a field, what the player has typed), else empty.
    pub text: String,
    /// The content path of its image (an `image`'s or a `button`'s), else empty.
    pub image: String,
    /// Where it is on the screen: left, top, width and height.
    pub rect: [i32; 4],
}

impl Gui {
    /// The realm's controls that are shown, in tree order: a control with `visible` off hides everything under it. Containers
    /// and mod roots are left out (they draw nothing). Empty when the realm has made nothing or the server layer is not up.
    pub fn client_script_controls(&self, vm: &Vm) -> Vec<ShownControl> {
        let Some(server) = self.0.borrow().client_script.server else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut stack: Vec<(ObjId, usize)> = vec![(server, 0)];
        let mut budget = MAX_CONTROLS * 2;
        while let Some((id, depth)) = stack.pop() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
            if id != server && !control::flag(vm, id, b"visible") {
                continue;
            }
            let shown = self.with_custom_state::<NodeState, _>(id, |state| {
                let kind = state.kind?;
                let text = match kind {
                    Kind::Text | Kind::Button | Kind::Toggle | Kind::TextField => {
                        state.props.text(Prop::Text)
                    }
                    _ => "",
                };
                let image = match kind {
                    Kind::Image | Kind::Button => state.props.text(Prop::Image),
                    _ => "",
                };
                Some((kind, text.to_string(), image.to_string()))
            });
            if let Some(Some((kind, text, image))) = shown {
                let (left, top) = control::screen_position(vm, id);
                let (_, (width, height)) = control::bounds(vm, id);
                found.push(ShownControl {
                    kind,
                    text,
                    image,
                    rect: [left, top, width, height],
                });
            }
            if depth < MAX_TREE_DEPTH {
                // The stack pops the last child first: push them back to front so the tree reads in order.
                stack.extend(children(vm, id).into_iter().rev().map(|c| (c, depth + 1)));
            }
        }
        found
    }
}
