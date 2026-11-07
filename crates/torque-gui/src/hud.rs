//! The HUD classes' state (TGE 1.4's `game/fps` HUDs and GuiGraphCtrl): what the client host tells the GUI of the connection
//! ([`HudFeed`]: the control object, the camera, the named shapes), the graph's plots ([`GraphState`]) and their console
//! methods. The drawing is `draw::hud`.
//!
//! - **GuiHealthBarHud** reads the connection's control object: its energy over its datablock's `maxEnergy` (`getEnergyValue`;
//!   a mount's when `inheritEnergyFromMount`), or its damage (`getDamageValue`). The host computes those values
//!   ([`HudControl::energy`], [`HudControl::damage`]).
//! - **GuiCrossHairHud** needs to know the control object is a Player or a Vehicle and the camera is in first person
//!   (`GameConnection::mCameraPos` 0).
//! - **GuiShapeNameHud** walks the global list of named shapes (at most [`MAX_NAMED_SHAPES`]: a ShapeBase is
//!   added when its update carries a name and taken out, the last one moved into its place, when its name goes or it is deleted), the
//!   control object left out, with the control camera's transform and fov and the scene's visible and fog distances. The ray test
//!   against the static world (`castRay` with the host's mask, the shape and its mounts out of it) is the host's
//!   ([`NamedShape::hidden`]); the distance, the field-of-view test, the fade and the projection are the GUI's. With its
//!   `distanceFade` set the fade is Torque3D's (whole up to that share of the visible distance, then down to 0 at it); without
//!   it, it follows the shape's name distance and the fog ([`crate::draw::name_opacity`]).
//! - **GuiGraphCtrl**: six plots of at most [`GRAPH_SAMPLES`] values, newest first.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/fps/guiHealthBarHud.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `T3D/fps/guiCrossHairHud.cpp` (Copyright (c) 2012 GarageGames, LLC), `T3D/fps/guiShapeNameHud.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), `gui/editor/guiGraphCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is
//! this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, Vm};

use crate::classes::reg;
use crate::{Gui, arg, atof, atoi};

/// The named-shape list's size (an update past it adds nothing).
pub const MAX_NAMED_SHAPES: usize = 1024;

/// The longest shape name drawn (net strings are at most 255 bytes).
pub const MAX_NAME_BYTES: usize = 255;

/// The control object of the connection, as the HUD classes read it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HudControl {
    /// Its type mask's Player bit (0x4000) and Vehicle bit (0x10000).
    pub player: bool,
    pub vehicle: bool,
    /// `GameConnection::mCameraPos` is 0: the first-person camera.
    pub first_person: bool,
    /// `getEnergyValue()`: the energy over the datablock's `maxEnergy` (0 when that is not positive), not clamped.
    pub energy: f32,
    /// `getDamageValue()`: the damage over the datablock's `maxDamage` (0 when that is not positive).
    pub damage: f32,
}

/// The control camera (`getControlCameraTransform`, `getControlCameraFov`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HudCamera {
    pub position: [f32; 3],
    /// The transform's rotation as TGE's row-major `MatrixF` (column 0 right, column 1 forward, column 2 up).
    pub rotation: [[f32; 3]; 3],
    /// Degrees.
    pub fov: f32,
}

/// One shape of the named-shape list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamedShape {
    pub name: Vec<u8>,
    /// Where the label is placed before `verticalOffset`: a Player's eye (column 3 of its render eye transform), else its world
    /// box's centre.
    pub position: [f32; 3],
    /// The name's colour (each channel a 4-bit value over 15; white by default).
    pub color: [f32; 3],
    /// `setShapeNameDistance`'s distance (8,192 by default; 12 bits over 4,095 times 8,192 on the wire).
    pub distance: f32,
    /// The host's ray from the camera to `position` hit the static world.
    pub hidden: bool,
}

/// What the client host tells the HUD classes each tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HudFeed {
    pub control: Option<HudControl>,
    pub camera: Option<HudCamera>,
    /// The client scene graph's visible distance (500 by default) and fog distance (250 by default).
    pub visible_distance: f32,
    pub fog_distance: f32,
    pub names: Vec<NamedShape>,
}

/// The most samples a plot keeps (the 201st falls off).
pub const GRAPH_SAMPLES: usize = 200;
/// GuiGraphCtrl's plots (`MaxPlots`; TGE's methods refuse a plot id above 6 and let 6 through, past the plots: this crate
/// refuses it too).
pub const GRAPH_PLOTS: usize = 6;

/// A plot's kind (`setGraphType`: Bar 3, Filled 2, Point 0, Polyline 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphType {
    Point,
    Polyline,
    Filled,
    Bar,
}

/// One plot (as constructed: 200 zeros, scale 1, a polyline).
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub data: Vec<f32>,
    pub max: f32,
    pub kind: GraphType,
    pub color: [f32; 3],
    /// `addAutoPlot`'s variable and its delay (ms), and the time it last added a value.
    pub auto: Option<(Vec<u8>, u32)>,
    pub auto_last: u32,
}

/// A GuiGraphCtrl's six plots.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphState {
    pub plots: Vec<Plot>,
}

impl Default for GraphState {
    fn default() -> GraphState {
        let colors = [
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
        ];
        GraphState {
            plots: colors
                .iter()
                .map(|c| Plot {
                    data: vec![0.0; GRAPH_SAMPLES],
                    max: 1.0,
                    kind: GraphType::Polyline,
                    color: *c,
                    auto: None,
                    auto_last: 0,
                })
                .collect(),
        }
    }
}

impl GraphState {
    /// `addDatum`: the value in front, the oldest past [`GRAPH_SAMPLES`] dropped, the scale raised to it.
    pub fn add(&mut self, plot: usize, v: f32) {
        let Some(p) = self.plots.get_mut(plot) else {
            return;
        };
        p.data.insert(0, v);
        p.data.truncate(GRAPH_SAMPLES);
        if p.max < v {
            p.max = v;
        }
    }
}

/// A plot id as the methods read it (`dAtoi`): above 6 the error and None; 6 and a negative id are None silently (TGE lets
/// them through to memory past or before the plots).
fn plot_id(vm: &mut Vm, text: &[u8]) -> Option<usize> {
    let n = atoi(text);
    if n > GRAPH_PLOTS as i32 {
        vm.error("Invalid plotID.".to_string());
        return None;
    }
    usize::try_from(n).ok().filter(|i| *i < GRAPH_PLOTS)
}

/// [`plot_id`] for `matchScale`: the error (None outer) stops the call; a silent refusal skips the id (Some(None)).
fn scale_id(vm: &mut Vm, text: &[u8]) -> Option<Option<usize>> {
    if atoi(text) > GRAPH_PLOTS as i32 {
        vm.error("Invalid plotID.".to_string());
        return None;
    }
    Some(plot_id(vm, text))
}

fn with_graph<R>(gui: &Gui, id: ObjId, f: impl FnOnce(&mut GraphState) -> R) -> Option<R> {
    gui.with_mut(id, |c| f(c.graph.get_or_insert_with(Box::default)))
}

/// The console methods of GuiGraphCtrl (`addDatum` 4, `getDatum` 4, `addAutoPlot` 5, `removeAutoPlot` 3, `setGraphType` 4,
/// `matchScale` 3..8).
pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    const G: &str = "GuiGraphCtrl";
    reg(
        vm,
        gui,
        G,
        "addDatum",
        4,
        4,
        "(int plotID, float v)",
        |vm, gui, id, c| {
            if let Some(p) = plot_id(vm, arg(c, 2)) {
                let v = atof(arg(c, 3)) as f32;
                with_graph(gui, id, |g| g.add(p, v));
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        G,
        "getDatum",
        4,
        4,
        "(int plotID, int samples)",
        |vm, gui, id, c| {
            let Some(p) = plot_id(vm, arg(c, 2)) else {
                return Native::Float(0.0);
            };
            let s = atoi(arg(c, 3));
            if s > GRAPH_SAMPLES as i32 {
                vm.error("Invalid sample.".to_string());
                return Native::Float(0.0);
            }
            let v = with_graph(gui, id, |g| {
                usize::try_from(s)
                    .ok()
                    .and_then(|s| g.plots[p].data.get(s).copied())
                    .unwrap_or(0.0)
            })
            .unwrap_or(0.0);
            Native::Float(f64::from(v))
        },
    );
    reg(
        vm,
        gui,
        G,
        "addAutoPlot",
        5,
        5,
        "(int plotID, string variable, int update)",
        |vm, gui, id, c| {
            if let Some(p) = plot_id(vm, arg(c, 2)) {
                let name = vm.intern_string(arg(c, 3)).to_vec();
                let delay = atoi(arg(c, 4)) as u32;
                with_graph(gui, id, |g| g.plots[p].auto = Some((name, delay)));
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        G,
        "removeAutoPlot",
        3,
        3,
        "(int plotID)",
        |vm, gui, id, c| {
            if let Some(p) = plot_id(vm, arg(c, 2)) {
                with_graph(gui, id, |g| g.plots[p].auto = None);
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        G,
        "setGraphType",
        4,
        4,
        "(int plotID, string graphType)",
        |vm, gui, id, c| {
            if let Some(p) = plot_id(vm, arg(c, 2)) {
                let t = arg(c, 3);
                let kind = if t.eq_ignore_ascii_case(b"Bar") {
                    Some(GraphType::Bar)
                } else if t.eq_ignore_ascii_case(b"Filled") {
                    Some(GraphType::Filled)
                } else if t.eq_ignore_ascii_case(b"Point") {
                    Some(GraphType::Point)
                } else if t.eq_ignore_ascii_case(b"Polyline") {
                    Some(GraphType::Polyline)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    with_graph(gui, id, |g| g.plots[p].kind = kind);
                }
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        G,
        "matchScale",
        3,
        8,
        "(int plotID, int plotID, ...)",
        |vm, gui, id, c| {
            // Every id checked first (one past the plots refuses the whole call), then the largest scale given to all.
            let mut ids = Vec::new();
            for i in 2..c.argv.len() {
                match scale_id(vm, arg(c, i)) {
                    Some(Some(p)) => ids.push(p),
                    Some(None) => {}
                    None => return Native::Void,
                }
            }
            with_graph(gui, id, |g| {
                let mut most = 0.0f32;
                for &p in &ids {
                    if most < g.plots[p].max {
                        most = g.plots[p].max;
                    }
                }
                for &p in &ids {
                    g.plots[p].max = most;
                }
            });
            Native::Void
        },
    );
}
