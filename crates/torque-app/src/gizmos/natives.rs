//! The scripts' side of [`Gizmos`]: `drawDebugLine` and its kin, with TGE's reading of their arguments.
//!
//! A vector is `"x y z"` and a color `"r g b"` or `"r g b a"` (floats, 0 to 1; an empty color is white), read as TGE's `sscanf`
//! reads a `Point3F` or a `ColorF`: missing numbers are 0 (an alpha is 1), text that is not a number ends the scan. A time is a
//! float of seconds and a depth switch a number (nonzero: depth tested; left out: tested). A rotation is `"x y z angle"`, the axis
//! and the angle in radians, as TGE's `AngAxisF`.

use std::rc::Rc;

use glam::{Quat, Vec3};
use torque_ts_text as ts_text;
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use super::NATIVE_SPECS;
use super::state::{Gizmos, Style};

fn vector(call: &NativeCall, i: usize) -> Vec3 {
    Vec3::from(call.vec3(i))
}

/// The color of argument `i`, with the time and depth switch of the arguments after it.
fn style(call: &NativeCall, i: usize) -> Style {
    let mut color = [0.0, 0.0, 0.0, 1.0];
    if ts_text::scan_floats(call.arg(i), &mut color) < 1 {
        color = [1.0; 4];
    }
    let seconds = if call.argc() > i + 1 {
        call.float(i + 1)
    } else {
        0.0
    };
    let depth = call.argc() <= i + 2 || call.int(i + 2) != 0;
    Style::new(color).seconds(seconds).depth(depth)
}

/// The rotation `"x y z angle"` names (none for an axis of no length).
fn rotation(call: &NativeCall, i: usize) -> Quat {
    let mut v = [0.0f32; 4];
    ts_text::scan_floats(call.arg(i), &mut v);
    let axis = Vec3::new(v[0], v[1], v[2]);
    if axis.length_squared() > 0.0 && v[3].is_finite() {
        Quat::from_axis_angle(axis.normalize(), v[3])
    } else {
        Quat::IDENTITY
    }
}

fn draw(name: &str, g: &Gizmos, call: &NativeCall) {
    match name {
        "drawDebugLine" => g.line(vector(call, 1), vector(call, 2), style(call, 3)),
        "drawDebugBox" => g.aabb(vector(call, 1), vector(call, 2), style(call, 3)),
        "drawDebugOrientedBox" => g.obb(
            vector(call, 1),
            vector(call, 2),
            rotation(call, 3),
            style(call, 4),
        ),
        "drawDebugSphere" => g.sphere(vector(call, 1), call.float(2), style(call, 3)),
        "drawDebugArrow" => g.arrow(vector(call, 1), vector(call, 2), style(call, 3)),
        "drawDebugCross" => g.cross(vector(call, 1), call.float(2), style(call, 3)),
        "drawDebugText" => {
            let text = String::from_utf8_lossy(call.cstr(2));
            g.label(vector(call, 1), &text, style(call, 3));
        }
        _ => g.clear(),
    }
}

/// Registers the natives over `gizmos`.
pub(super) fn register(vm: &mut Vm, gizmos: &Rc<Gizmos>) {
    for spec in NATIVE_SPECS {
        let g = Rc::clone(gizmos);
        let name = spec.name;
        vm.register(
            spec.name,
            spec.min,
            spec.max,
            spec.usage,
            move |vm, call| {
                draw(name, &g, call);
                for warning in g.take_warnings() {
                    vm.warn(warning);
                }
                Native::Void
            },
        );
    }
}

/// Tells the console what the Rust side of `gizmos` warned of (the plugin's tick).
pub(super) fn flush(vm: &mut Vm, gizmos: &Gizmos) {
    for warning in gizmos.take_warnings() {
        vm.warn(warning);
    }
}
