use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginId};
use ts_vm::vm::Vm;

use super::*;

/// A predictor that only has an id.
#[derive(Clone)]
struct Named(&'static str);

impl Prediction for Named {
    fn id(&self) -> &str {
        self.0
    }

    fn clone_box(&self) -> Box<dyn Prediction> {
        Box::new(self.clone())
    }
}

fn ids(slot: &PredictionSlot) -> Vec<&str> {
    slot.ids().collect()
}

#[test]
fn tge_is_always_first_and_the_default() {
    let (slot, warnings) = PredictionSlot::new(&[PredictionEntry::new(Named("smooth"))], None);
    assert!(warnings.is_empty());
    assert_eq!(ids(&slot), ["tge", "smooth"]);
    assert_eq!(slot.current(), TGE_PREDICTION);
    assert_eq!(slot.make().id(), TGE_PREDICTION);
}

#[test]
fn the_start_runs_and_a_missing_start_warns() {
    let entries = [PredictionEntry::new(Named("smooth"))];
    let (slot, _) = PredictionSlot::new(&entries, Some("smooth"));
    assert_eq!(slot.current(), "smooth");
    let (slot, warnings) = PredictionSlot::new(&entries, Some("gone"));
    assert_eq!(slot.current(), TGE_PREDICTION);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn invalid_and_repeated_ids_are_left_out() {
    let entries = [
        PredictionEntry::new(Named("a b")),
        PredictionEntry::new(Named("smooth")),
        PredictionEntry::new(Named("smooth")),
    ];
    let (slot, warnings) = PredictionSlot::new(&entries, None);
    assert_eq!(ids(&slot), ["tge", "smooth"]);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
}

#[test]
fn a_switch_counts_and_an_unknown_id_is_refused() {
    let (slot, _) = PredictionSlot::new(&[PredictionEntry::new(Named("smooth"))], None);
    let first = slot.generation();
    slot.switch("tge").unwrap();
    assert_eq!(slot.generation(), first, "no swap to the one running");
    slot.switch("smooth").unwrap();
    assert_eq!(slot.generation(), first + 1);
    let e = slot.switch("other").unwrap_err();
    assert_eq!(e.registered, ["tge", "smooth"]);
    assert_eq!(
        e.to_string(),
        "no predictor \"other\" (registered: tge, smooth)"
    );
    assert_eq!(slot.current(), "smooth");
}

#[derive(PluginId)]
#[plugin(id = "test.smooth")]
struct Smooth;

impl Plugin for Smooth {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().prediction(Named("smooth"));
    }
}

#[derive(PluginId)]
#[plugin(id = "test.other")]
struct Other;

impl Plugin for Other {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().prediction(Named("other"));
    }
}

#[derive(PluginId)]
#[plugin(id = "test.extra")]
struct Extra;

impl Plugin for Extra {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().add_prediction(Named("extra"));
    }
}

fn client(engine: &torque_engine::Engine) -> Vm {
    let mut vm = Vm::new();
    engine.replay(ClientStage::Natives, &mut vm).unwrap();
    vm
}

#[test]
fn an_engine_without_predictors_has_no_slot_or_natives() {
    let engine = EngineBuilder::new().build().unwrap();
    let vm = client(&engine);
    assert!(PredictionSlot::of(&vm).is_none());
    assert!(!vm.is_native_function(b"setPredictor"));
}

#[test]
fn the_games_predictor_starts_and_scripts_swap() {
    let engine = EngineBuilder::new()
        .add_plugins((Extra, Smooth))
        .build()
        .unwrap();
    let mut vm = client(&engine);
    let slot = PredictionSlot::of(&vm).expect("the slot");
    assert_eq!(slot.ids().collect::<Vec<_>>(), ["tge", "extra", "smooth"]);
    assert_eq!(slot.current(), "smooth");
    assert_eq!(vm.eval(b"return getPredictor();").unwrap(), b"smooth");
    assert_eq!(vm.eval(b"return setPredictor(\"extra\");").unwrap(), b"1");
    assert_eq!(slot.current(), "extra");
    assert_eq!(vm.eval(b"return setPredictor(\"none\");").unwrap(), b"0");
    assert_eq!(slot.current(), "extra");
}

#[test]
fn two_games_predictors_fail_the_build() {
    let err = EngineBuilder::new()
        .add_plugins((Smooth, Other))
        .build()
        .unwrap_err();
    assert!(err.to_string().contains("test.other"), "{err}");
}

/// A part that only has an id.
#[derive(Clone)]
struct Part(&'static str);

impl PredictionPart for Part {
    fn id(&self) -> &str {
        self.0
    }

    fn clone_box(&self) -> Box<dyn PredictionPart> {
        Box::new(self.clone())
    }
}

#[derive(PluginId)]
#[plugin(id = "test.parts")]
struct Parts;

impl Plugin for Parts {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .prediction_part(Part("props"))
            .prediction_part(Part("props"))
            .prediction_part(Part("bad id"))
            .prediction_part(Part("water"));
    }
}

#[test]
fn parts_run_beside_the_games_predictor_in_order_and_bad_ones_are_left_out() {
    let engine = EngineBuilder::new()
        .add_plugins((Parts, Smooth))
        .build()
        .unwrap();
    let mut vm = client(&engine);
    let slot = PredictionSlot::of(&vm).expect("the slot");
    assert_eq!(slot.part_ids().collect::<Vec<_>>(), ["props", "water"]);
    assert_eq!(slot.current(), "smooth", "a part is no predictor");
    let made: Vec<String> = slot
        .make_parts()
        .iter()
        .map(|p| p.id().to_owned())
        .collect();
    assert_eq!(made, ["props", "water"]);
    let log: Vec<String> = vm
        .drain_log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect();
    assert_eq!(log.len(), 2, "one warning for each left out: {log:?}");
}
