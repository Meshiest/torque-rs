use std::any::Any;
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use super::*;
use crate::draw::{DrawCache, GuiBatch, draw_canvas};
use crate::ext::GuiExtension;
use crate::{Gui, canvas};

/// A host that keeps TGE's bevels.
struct Classic;

impl GuiExtension for Classic {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn classic_boxes(&self, _vm: &Vm, _profile: ObjId) -> bool {
        true
    }
}

const PROFILES: &str = "new GuiControlProfile(BackProfile) { opaque = 1; fillColor = \"0 0 0 255\"; }; \
    new GuiControlProfile(ButtonProfile) { fillColor = \"40 50 60 255\"; fillColorHL = \"200 150 100 255\"; \
       fillColorNA = \"20 20 20 255\"; borderColor = \"90 100 110 255\"; }; \
    new GuiControlProfile(PlainProfile) { }; \
    new GuiControlProfile(RoundProfile) { opaque = 1; fillColor = \"30 120 60 255\"; cornerRadius = 8; }; \
    new GuiControlProfile(RoundBorderProfile) { opaque = 1; fillColor = \"30 120 60 255\"; border = 1; \
       borderColor = \"250 250 250 255\"; cornerRadius = 8; }; \
    new GuiControlProfile(SquareProfile) { opaque = 1; fillColor = \"30 120 60 255\"; border = 1; borderColor = \"250 250 250 255\"; }; \
    new GuiControlProfile(HugeProfile) { opaque = 1; fillColor = \"30 120 60 255\"; cornerRadius = 500; }; \
    new GuiControlProfile(RoundButtonProfile) { fillColor = \"40 50 60 255\"; borderColor = \"90 100 110 255\"; cornerRadius = 9; }; \
    new GuiControlProfile(FadeProfile) { opaque = 1; fillColor = \"200 40 40 255\"; };";

/// A canvas whose content holds `body`.
fn rig(body: &str) -> (Vm, Gui) {
    let mut vm = Vm::new();
    let gui = crate::register(&mut vm);
    gui.force_canvas_size(Some((200, 100)));
    vm.eval(PROFILES.as_bytes()).unwrap();
    assert!(canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(
        format!("new GuiControl(Top) {{ profile = BackProfile; extent = \"200 100\"; visible = 1; {body} }};")
            .as_bytes(),
    )
    .unwrap();
    let top = vm.objects().find(b"Top").unwrap();
    canvas::set_content(&mut vm, &gui, top).unwrap();
    (vm, gui)
}

fn button(name: &str, profile: &str, at: &str, extent: &str) -> String {
    format!(
        "new GuiButtonCtrl({name}) {{ profile = {profile}; position = \"{at}\"; extent = \"{extent}\"; visible = 1; enabled = 1; }};"
    )
}

/// The batches drawn with the rounded boxes' texture.
fn box_batches(vm: &Vm, gui: &Gui) -> Vec<GuiBatch> {
    let id = box_texture().id;
    draw_canvas(vm, gui, &mut DrawCache::default())
        .draws
        .batches
        .into_iter()
        .filter(|b| b.texture.as_ref().is_some_and(|t| t.id == id))
        .collect()
}

fn alpha(x: i32, y: i32) -> u8 {
    let t = box_texture();
    t.rgba[((y as u32 * t.size.0 + x as u32) * 4 + 3) as usize]
}

#[test]
fn the_sprites_are_solid_inside_empty_outside_and_soft_at_the_corners() {
    let r = BOX_RADIUS;
    let side = 2 * r + 1;
    let y = SPRITES.rows[r as usize];
    // The whole shape: the corner texel empty, the middle and the middle of each edge solid, the arc's texels in between.
    assert_eq!(alpha(0, y), 0);
    assert_eq!(alpha(r, y + r), 255);
    assert_eq!(alpha(0, y + r), 255);
    assert_eq!(alpha(r, y), 255);
    let corner: Vec<u8> = (0..r)
        .flat_map(|ty| (0..r).map(move |tx| alpha(tx, y + ty)))
        .collect();
    assert!(corner.iter().any(|a| *a > 0 && *a < 255), "{corner:?}");
    // The border: solid on the edge, empty in the middle; the fill under it half a pixel in.
    assert_eq!(alpha(2 * side, y + r), 255);
    assert_eq!(alpha(2 * side + r, y + r), 0);
    assert_eq!(alpha(side, y + r), 128);
    assert_eq!(alpha(side + r, y + r), 255);
    // Every radius's sprites are symmetric.
    for r in 1..=BOX_RADIUS {
        let (side, y) = (2 * r + 1, SPRITES.rows[r as usize]);
        for k in 0..3 {
            for ty in 0..side {
                for tx in 0..side {
                    let a = alpha(k * side + tx, y + ty);
                    assert_eq!(a, alpha(k * side + side - 1 - tx, y + ty));
                    assert_eq!(a, alpha(k * side + tx, y + side - 1 - ty));
                }
            }
        }
    }
}

#[test]
fn a_button_is_one_batch_of_a_fill_and_a_border_colored_by_its_state() {
    let (mut vm, gui) = rig(&button("Btn", "ButtonProfile", "10 20", "80 30"));
    let id = vm.objects().find(b"Btn").unwrap();
    let batches = box_batches(&vm, &gui);
    assert_eq!(batches.len(), 1);
    let quads = &batches[0].quads;
    // Nine fill pieces, eight border pieces (its middle is empty), the corners one texel a pixel.
    assert_eq!(quads.len(), 17);
    assert!(quads[..9].iter().all(|q| q.color == [40, 50, 60, 255]));
    assert!(quads[9..].iter().all(|q| q.color == [90, 100, 110, 255]));
    let cover = |qs: &[crate::draw::GuiQuad]| {
        qs.iter()
            .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, q| {
                [
                    a[0].min(q.rect[0]),
                    a[1].min(q.rect[1]),
                    a[2].max(q.rect[2]),
                    a[3].max(q.rect[3]),
                ]
            })
    };
    assert_eq!(cover(&quads[..9]), [10.0, 20.0, 90.0, 50.0]);
    assert_eq!(cover(&quads[9..]), [10.0, 20.0, 90.0, 50.0]);
    let area: f32 = quads[..9]
        .iter()
        .map(|q| (q.rect[2] - q.rect[0]) * (q.rect[3] - q.rect[1]))
        .sum();
    assert_eq!(area, 80.0 * 30.0);
    let r = BOX_RADIUS as f32;
    assert_eq!(quads[0].rect, [10.0, 20.0, 10.0 + r, 20.0 + r]);
    let fill_of = |vm: &Vm, gui: &Gui| box_batches(vm, gui)[0].quads[4].color;
    // Over: the highlight; pressed: the highlight darkened; not active: the disabled fill.
    gui.with_mut(id, |c| c.mouse_over = true);
    assert_eq!(fill_of(&vm, &gui), [200, 150, 100, 255]);
    gui.with_mut(id, |c| c.depressed = true);
    assert_eq!(fill_of(&vm, &gui), [160, 120, 80, 255]);
    gui.with_mut(id, |c| (c.depressed, c.mouse_over) = (false, false));
    vm.eval(b"Btn.setActive(false);").unwrap();
    assert_eq!(fill_of(&vm, &gui), [20, 20, 20, 255]);
}

#[test]
fn a_profile_without_colors_gets_made_ones() {
    // Away from the cursor (at 0 0), so the box is not hovered.
    let (vm, gui) = rig(&button("Btn", "PlainProfile", "50 50", "40 20"));
    let quads = &box_batches(&vm, &gui)[0].quads;
    assert_eq!(quads[4].color, DEFAULT_FILL);
    assert_eq!(quads[9].color, [134, 136, 139, 255]);
}

#[test]
fn a_small_box_takes_a_smaller_radius_and_an_empty_one_draws_nothing() {
    let (vm, gui) = rig(&format!(
        "{}{}",
        button("Small", "ButtonProfile", "0 0", "12 5"),
        button("Empty", "ButtonProfile", "50 0", "0 10")
    ));
    let batches = box_batches(&vm, &gui);
    assert_eq!(batches.len(), 1);
    let q = &batches[0].quads;
    assert_eq!(q.len(), 17);
    assert_eq!(q[0].rect, [0.0, 0.0, 2.0, 2.0]);
    assert_eq!(q[4].rect, [2.0, 2.0, 10.0, 3.0]);
}

#[test]
fn the_classic_rule_keeps_the_raised_box() {
    let (vm, gui) = rig(&button("Btn", "ButtonProfile", "10 20", "80 30"));
    gui.set_extension(Rc::new(Classic));
    assert!(box_batches(&vm, &gui).is_empty());
    let frame = draw_canvas(&vm, &gui, &mut DrawCache::default());
    let quads: Vec<_> = frame.draws.batches.iter().flat_map(|b| &b.quads).collect();
    // The content's fill, then the box: its fill and six bevel lines.
    assert_eq!(quads.len(), 8);
    assert_eq!(quads[1].rect, [10.0, 20.0, 90.0, 50.0]);
    assert_eq!(quads[1].color, [40, 50, 60, 255]);
}

fn panel(name: &str, profile: &str, at: &str, extent: &str, extra: &str) -> String {
    format!(
        "new GuiControl({name}) {{ profile = {profile}; position = \"{at}\"; extent = \"{extent}\"; visible = 1; {extra} }};"
    )
}

/// The quads of the frame with the color `color`, in order.
fn quads_colored(vm: &Vm, gui: &Gui, color: [u8; 4]) -> Vec<crate::draw::GuiQuad> {
    draw_canvas(vm, gui, &mut DrawCache::default())
        .draws
        .batches
        .into_iter()
        .flat_map(|b| b.quads)
        .filter(|q| q.color == color)
        .collect()
}

fn wide_alpha(x: i32, y: i32) -> u8 {
    let t = &WIDE_SPRITES.texture;
    t.rgba[((y as u32 * t.size.0 + x as u32) * 4 + 3) as usize]
}

#[test]
fn a_profile_with_a_corner_radius_draws_its_fill_with_rounded_corners() {
    let (vm, gui) = rig(&panel("Card", "RoundProfile", "10 10", "80 50", ""));
    let wide = WIDE_SPRITES.texture.id;
    let frame = draw_canvas(&vm, &gui, &mut DrawCache::default());
    let batch = frame
        .draws
        .batches
        .iter()
        .find(|b| b.texture.as_ref().is_some_and(|t| t.id == wide))
        .expect("the card is drawn with the corner sprites");
    // Nine pieces that cover the card exactly, the corners one texel a pixel.
    assert_eq!(batch.quads.len(), 9);
    assert!(batch.quads.iter().all(|q| q.color == [30, 120, 60, 255]));
    assert_eq!(batch.quads[0].rect, [10.0, 10.0, 18.0, 18.0]);
    let area: f32 = batch
        .quads
        .iter()
        .map(|q| (q.rect[2] - q.rect[0]) * (q.rect[3] - q.rect[1]))
        .sum();
    assert_eq!(area, 80.0 * 50.0);
}

#[test]
fn a_rounded_fill_and_border_are_one_shape_and_a_profile_without_a_radius_stays_square() {
    let (vm, gui) = rig(&format!(
        "{}{}",
        panel("Round", "RoundBorderProfile", "10 10", "80 50", ""),
        panel("Square", "SquareProfile", "110 10", "80 50", "")
    ));
    let fill = quads_colored(&vm, &gui, [30, 120, 60, 255]);
    let border = quads_colored(&vm, &gui, [250, 250, 250, 255]);
    // The round one: nine fill pieces and eight border pieces. The square one: a fill and the four lines of an outline.
    assert_eq!(fill.len(), 9 + 1);
    assert_eq!(border.len(), 8 + 4);
    assert_eq!(fill[9].rect, [110.0, 10.0, 190.0, 60.0]);
    assert!(border[8..].iter().all(|q| q.rect[0] >= 110.0));
    // The corner of a round border is a sprite texel, not a square line.
    assert_eq!(border[0].rect, [10.0, 10.0, 18.0, 18.0]);
}

#[test]
fn the_radius_is_cut_to_the_cap_and_to_half_the_shorter_side() {
    let (vm, gui) = rig(&format!(
        "{}{}",
        panel("Huge", "HugeProfile", "10 10", "100 100", ""),
        panel("Thin", "HugeProfile", "120 10", "60 10", "")
    ));
    let fill = quads_colored(&vm, &gui, [30, 120, 60, 255]);
    // Nine pieces, and six of the thin one: its middle row has no height.
    assert_eq!(fill.len(), 9 + 6);
    let side = MAX_PROFILE_RADIUS as f32;
    assert_eq!(fill[0].rect, [10.0, 10.0, 10.0 + side, 10.0 + side]);
    // A box 10 high takes a radius of 5, the engine's own sprites.
    assert_eq!(fill[9].rect, [120.0, 10.0, 125.0, 15.0]);
}

#[test]
fn the_wide_sprites_are_solid_inside_empty_at_the_corner_and_symmetric() {
    for r in [BOX_RADIUS + 1, 12, MAX_PROFILE_RADIUS] {
        let (side, y) = (2 * r + 1, WIDE_SPRITES.rows[r as usize]);
        assert_eq!(wide_alpha(0, y), 0, "radius {r}");
        assert_eq!(wide_alpha(r, y + r), 255);
        assert_eq!(wide_alpha(0, y + r), 255);
        assert_eq!(
            wide_alpha(2 * side + r, y + r),
            0,
            "the border is empty inside"
        );
        assert_eq!(wide_alpha(2 * side, y + r), 255);
        for ty in 0..side {
            for tx in 0..side {
                let a = wide_alpha(tx, y + ty);
                assert_eq!(a, wide_alpha(side - 1 - tx, y + ty));
                assert_eq!(a, wide_alpha(tx, y + side - 1 - ty));
            }
        }
    }
}

#[test]
fn a_button_takes_the_radius_of_its_profile_instead_of_the_engines() {
    let (vm, gui) = rig(&button("Btn", "RoundButtonProfile", "10 20", "80 30"));
    let fill = quads_colored(&vm, &gui, [40, 50, 60, 255]);
    assert_eq!(fill.len(), 9);
    assert_eq!(fill[0].rect, [10.0, 20.0, 19.0, 29.0]);
}

#[test]
fn an_opacity_dims_the_control_and_the_controls_in_it() {
    let inner = panel("Inner", "FadeProfile", "5 5", "20 20", "opacity = 0.5;");
    let (vm, gui) = rig(&format!(
        "{}{}",
        panel(
            "Outer",
            "FadeProfile",
            "10 10",
            "80 80",
            &format!("opacity = 0.5; {inner}")
        ),
        panel("Plain", "FadeProfile", "110 10", "40 40", "")
    ));
    assert_eq!(quads_colored(&vm, &gui, [200, 40, 40, 128]).len(), 1);
    // The inner one has its own half on top of its parent's.
    let quarter = quads_colored(&vm, &gui, [200, 40, 40, 64]);
    assert_eq!(quarter.len(), 1);
    assert_eq!(quarter[0].rect, [15.0, 15.0, 35.0, 35.0]);
    // The plain one, beside them, is as it is.
    assert_eq!(quads_colored(&vm, &gui, [200, 40, 40, 255]).len(), 1);
}

#[test]
fn the_content_of_the_canvas_takes_an_opacity_too() {
    let (mut vm, gui) = rig(&panel("Child", "FadeProfile", "10 10", "30 30", ""));
    vm.eval(b"Top.opacity = 0.5;").unwrap();
    // The content's own black fill and its child's, both at half.
    let frame = draw_canvas(&vm, &gui, &mut DrawCache::default());
    let alphas: Vec<u8> = frame
        .draws
        .batches
        .iter()
        .flat_map(|b| b.quads.iter().map(|q| q.color[3]))
        .collect();
    assert_eq!(alphas, [128, 128]);
}

#[test]
fn opacity_starts_at_one_on_every_class() {
    let mut vm = Vm::new();
    crate::register(&mut vm);
    vm.eval(
        b"new GuiControl(A) { }; new GuiButtonCtrl(B) { }; new GuiScrollCtrl(C) { }; new GuiTextCtrl(D) { };",
    )
    .unwrap();
    for name in ["A", "B", "C", "D"] {
        let got = vm
            .eval(format!("return {name}.opacity;").as_bytes())
            .unwrap();
        assert_eq!(got, b"1", "{name}");
    }
}

#[test]
fn an_opacity_past_the_range_dims_to_nothing_or_not_at_all() {
    let (vm, gui) = rig(&format!(
        "{}{}",
        panel("Over", "FadeProfile", "10 10", "30 30", "opacity = 7;"),
        panel("Under", "FadeProfile", "60 10", "30 30", "opacity = -2;")
    ));
    // More than 1 is no dimming; less than 0 is none of it shown.
    assert_eq!(quads_colored(&vm, &gui, [200, 40, 40, 255]).len(), 1);
    assert_eq!(quads_colored(&vm, &gui, [200, 40, 40, 0]).len(), 1);
}

/// A host that dims every control it is asked about.
struct Dim;

impl GuiExtension for Dim {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn opacity(&self, _vm: &Vm, _id: ObjId) -> Option<f32> {
        Some(0.5)
    }
}

#[test]
fn the_extensions_opacity_dims_children_and_not_the_content_it_hangs_in() {
    let (vm, gui) = rig(&panel("Child", "FadeProfile", "10 10", "30 30", ""));
    gui.set_extension(Rc::new(Dim));
    let frame = draw_canvas(&vm, &gui, &mut DrawCache::default());
    let alphas: Vec<u8> = frame
        .draws
        .batches
        .iter()
        .flat_map(|b| b.quads.iter().map(|q| q.color[3]))
        .collect();
    // The content's black fill is as it was; its child's is dimmed.
    assert_eq!(alphas, [255, 128]);
}
