//! The server container (`Container`, `gServerContainer`) and its queries: `initContainerRadiusSearch`,
//! `initContainerBoxSearch`, `containerSearchNext`, `containerSearchCurrDist`, `containerSearchCurrRadiusDist`,
//! `containerBoxEmpty`, `containerFindFirst`, `containerFindNext`, `containerRayCast`, `calcExplosionCoverage`. A host adds
//! its own queries with [`collect`] and the argument readers ([`mask_arg`], [`radii_args`]).
//!
//! **What the container holds**: every scene object in the server scene graph, in the bins of [`torque_scene::bins`] (their
//! grid is the [`ContainerConfig`]'s: TGE's 16 x 16 cells of 64 units by default)
//! (`addToScene` puts them there by their world box; `setTransform`, `setScale` and a mover's tick move them,
//! [`super::object::moved`]), and whatever the [`SpatialIndex`]es registered with it hold. Every query is
//! `Container::findObjects` (each index in the order it was
//! registered, then the bins) or `Container::castRay` ([`container_ray`]: the gate, each index with the raw mask, then the
//! bins' overflow list and the cells the segment steps through, each object answering with its own `castRay`, [`super::rays`];
//! strictly nearer replaces). `containerRayCast`'s exemptions are the collision-disable count raised for the cast. The
//! radius and box searches keep the indexes' and the bins' objects and sort them together; `containerBoxEmpty` asks each index
//! whether what it found blocks the box, then a bins object for its polygon list ([`super::rays::build_poly_list`])
//! into an `EarlyOutPolyList` of the box's planes. `collide`'s queries reach the same container through
//! [`super::container_world::ContainerWorld`].
//!
//! **Order.** The order an index gives its objects in is its own; the order the sort leaves equal distances in is
//! the search sort's ([`torque_scene::library::SceneLibrary::sort_by_distance`]).
//!
//! **Cost.** Every candidate a radius search filters and every compare the sort makes are charged
//! ([`torque_scene::query::COMPARE_WORK`], [`CANDIDATE_WORK`]), each before the work it pays for, and an index charges its own
//! walks; a walk or a sort the budget cannot pay ends the run with the step limit. The lists a search keeps are cut at
//! [`MAX_SEARCH_RESULTS`], and the cursor calls (`containerSearchNext`, `containerFindNext`) walk those lists once, so their
//! skips are already paid for by the search that made the list. No query runs script, so none holds a borrow across script
//! execution and none adds a recursive path.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `scene/sceneContainer.cpp` (Copyright (c) 2012 GarageGames, LLC); `T3D/fx/explosion.cpp` (Copyright (c) 2012 GarageGames,
//! LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;
use torque_scene::Geometry as _;

use shapes::collide::box_planes;
use torque_scene::bins::{BinGeometry, Bins};
use torque_scene::query::CANDIDATE_WORK;
use torque_scene::query::{
    FindList, MAX_SEARCH_RESULTS, Search, SearchItem, box_distance_squared, center_distance,
    find_box, radii_box, radius_box, radius_distance, size_box, within_radius,
};
use torque_scene::{Charge, Exhausted, NO_HIT, ray_point};
use ts_vm::sim::{ObjId, Objects};
use ts_vm::text::{format_g, to_f64};
use ts_vm::vm::{Level, Native, NativeCall, Vm, VmError};

use super::access::{matrix_of, scale_of};
use super::camera::is_class;
use super::ext;
use crate::shapes::Shapes;

/// What the natives share: the search list `containerSearchNext` walks and the find list `containerFindNext` walks (the engine
/// keeps both in globals).
#[derive(Debug, Default)]
pub struct ContainerState {
    search: Search<Found>,
    find: FindList,
}

/// What a container query found (`Container::findObjects`): an object an index gave, with the index's number and its
/// key for the object, or a scene object from the bins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub object: u32,
    /// The index that gave the object and its key for it; none for an object of the bins.
    pub from: Option<(usize, u64)>,
}

impl SearchItem for Found {
    fn object(&self) -> u32 {
        self.object
    }
}

/// A hit an index's ray walk answered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexHit {
    pub hit: ContainerHit,
    /// The index asks to be told when its hit is the cast's answer ([`SpatialIndex::ray_answered`]).
    pub tell: bool,
}

/// A spatial index the container asks ahead of its bins, in the order the indexes were registered (`Container::add_index`): the
/// order of a host's index is the order the scope of a connection's ghosts and so their ids follow. Every method is a leaf: no
/// script runs and no borrow is held across one.
pub trait SpatialIndex {
    /// The objects of `mask` whose box the query box reaches, in the index's order, each with the index's key for it; the walk
    /// is charged to `charge`.
    fn find(
        &self,
        vm: &Vm,
        query: [f32; 6],
        mask: u32,
        charge: Charge,
        found: &mut dyn FnMut(u32, u64),
    ) -> Result<(), Exhausted>;

    /// The nearest hit of `mask` on the segment among the index's objects (`shapes`, when given, lets an object answer from its
    /// meshes). Charged to the run.
    fn cast_ray(
        &self,
        vm: &Vm,
        shapes: Option<&Shapes>,
        start: [f32; 3],
        end: [f32; 3],
        mask: u32,
    ) -> Result<Option<IndexHit>, VmError>;

    /// The index's hit (one it asked to be told of) was the answer of a cast.
    fn ray_answered(&self) {}

    /// `containerBoxEmpty`'s test of the index's own objects among `found` (every entry the query found is looked at, and paid
    /// for, in order): whether one of them blocks the box.
    fn box_blocked(
        &self,
        vm: &Vm,
        shapes: &Shapes,
        query: [f32; 6],
        index: usize,
        found: &[Found],
    ) -> Result<bool, VmError>;

    /// Whether `object` is still what the index found as `key` (object ids come round again).
    fn still(&self, objects: &Objects, object: u32, key: u64) -> bool;

    /// Whether the index holds the object (`calcExplosionCoverage`'s "in the container").
    fn holds(&self, vm: &Vm, object: ObjId) -> bool;
}

/// What a container is made of: the shape of its bins and the spatial indexes asked ahead of them, in that order. The default
/// is TGE 1.4's container: its 16 x 16 bins ([`BinGeometry::default`]) and no index.
#[derive(Default)]
pub struct ContainerConfig {
    /// The grid of the bins and the type bits they answer for.
    pub bins: BinGeometry,
    /// The indexes, in the order every query asks them (all before the bins).
    pub indexes: Vec<Rc<dyn SpatialIndex>>,
    /// The order a collision phase builds its working list in (`Convex::updateWorkingList`); none keeps the container's (TGE).
    pub candidate_order: Option<Rc<dyn CandidateOrder>>,
}

/// A host's order of a convex's working-list candidates: `ids` are the container's finds in its order; `charge` pays for each
/// compare before it is made (false: the budget is spent, and the order is left as it stands).
pub trait CandidateOrder {
    fn order(
        &self,
        world: &dyn collide::World,
        ids: &mut [collide::ObjectId],
        charge: &mut dyn FnMut() -> bool,
    );
}

/// The server container: the bins and the spatial indexes asked ahead of them.
pub struct Container {
    bins: RefCell<Bins>,
    indexes: Vec<Rc<dyn SpatialIndex>>,
    candidate_order: Option<Rc<dyn CandidateOrder>>,
}

impl std::fmt::Debug for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Container")
            .field("bins", &self.bins.borrow().len())
            .field("indexes", &self.indexes.len())
            .finish()
    }
}

impl Container {
    /// A container of `config`'s bins and indexes, empty.
    pub fn new(config: ContainerConfig) -> Container {
        Container {
            bins: RefCell::new(Bins::with_geometry(config.bins)),
            indexes: config.indexes,
            candidate_order: config.candidate_order,
        }
    }

    /// The host's order of a working list's candidates, if it has one.
    pub fn candidate_order(&self) -> Option<&Rc<dyn CandidateOrder>> {
        self.candidate_order.as_ref()
    }

    /// The bins (a short borrow: never held across script).
    pub fn bins(&self) -> Ref<'_, Bins> {
        self.bins.borrow()
    }

    /// The bins, to place, move or take out an object (a short borrow: never held across script).
    pub fn bins_mut(&self) -> RefMut<'_, Bins> {
        self.bins.borrow_mut()
    }

    /// The indexes, in their order.
    pub fn indexes(&self) -> &[Rc<dyn SpatialIndex>] {
        &self.indexes
    }
}

/// What the bins read of a scene object, from the VM's objects: the scene state (boxes and sphere), the type mask and the
/// collision-disable count. An object of the bins answers a ray through its own class ([`super::rays`]), never through this
/// view.
pub struct SceneView<'a> {
    vm: &'a Vm,
    /// The last object whose type mask was read, with its world box: a box walk tests an object's mask and then its box, and
    /// this answers the box without a second look-up of the object.
    last: Cell<Option<LastBox>>,
}

/// An object and its world box (min, max).
type LastBox = (u32, [f32; 3], [f32; 3]);

impl<'a> SceneView<'a> {
    pub fn new(vm: &'a Vm) -> SceneView<'a> {
        SceneView {
            vm,
            last: Cell::new(None),
        }
    }

    fn scene(&self, object: u32) -> ext::SceneState {
        ext::get(self.vm.objects(), object)
            .and_then(|ext| ext.scene)
            .unwrap_or_default()
    }
}

impl torque_scene::Geometry for SceneView<'_> {
    fn sphere(&self, object: u32) -> ([f32; 3], f32) {
        let scene = self.scene(object);
        (scene.world_center, scene.world_radius)
    }
    fn world_box(&self, object: u32) -> ([f32; 3], [f32; 3]) {
        if let Some((id, min, max)) = self.last.get()
            && id == object
        {
            return (min, max);
        }
        let scene = self.scene(object);
        (scene.world_min, scene.world_max)
    }
    fn type_mask(&self, object: u32) -> u32 {
        let Some(ext) = ext::get(self.vm.objects(), object) else {
            self.last.set(None);
            return 0;
        };
        let scene = ext.scene.unwrap_or_default();
        self.last
            .set(Some((object, scene.world_min, scene.world_max)));
        ext.type_mask
    }
    /// The collision-disable count is not zero (`containerRayCast`'s exemptions raise it for the cast).
    fn collision_disabled(&self, object: u32) -> bool {
        ext::get(self.vm.objects(), object).is_some_and(|e| e.collision_off != 0)
    }
}

type State = Rc<RefCell<ContainerState>>;

/// A `Point3F` argument.
#[deprecated(note = "use `NativeCall::vec3`")]
pub fn vec3_arg(c: &NativeCall, i: usize) -> [f32; 3] {
    c.vec3(i)
}

/// A bitset argument: `atoi`, as an unsigned number (`-1` is every bit).
pub fn mask_arg(c: &NativeCall, i: usize) -> u32 {
    c.int(i) as u32
}

/// Registers a global function with its spec's argument counts, charging the bytes of its arguments first (as every host
/// native does).
fn register(
    vm: &mut Vm,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
) {
    vm.register(name, min, max, usage, move |vm, c| {
        let bytes = c
            .argv
            .iter()
            .skip(1)
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        if bytes > 0 && !vm.charge_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
            return Native::Fail(VmError::StepLimit);
        }
        f(vm, c)
    });
}

/// `Container::findObjects` over a box: the objects each index reaches, in its order, then the bins' objects (at most
/// [`MAX_SEARCH_RESULTS`] in all are kept; an index's walk goes on to its end, the bins' stops there).
pub fn collect(
    vm: &Vm,
    container: &Container,
    query: [f32; 6],
    mask: u32,
) -> Result<Vec<Found>, VmError> {
    let mut charge = |units: u64| vm.charge_work(units);
    let mut out: Vec<Found> = Vec::new();
    for (number, index) in container.indexes().iter().enumerate() {
        index
            .find(vm, query, mask, &mut charge, &mut |object, key| {
                if out.len() < MAX_SEARCH_RESULTS {
                    out.push(Found {
                        object,
                        from: Some((number, key)),
                    });
                }
            })
            .map_err(|_| VmError::StepLimit)?;
    }
    let view = SceneView::new(vm);
    container
        .bins()
        .find(query, mask, &view, &mut charge, &mut |object| {
            out.push(Found { object, from: None });
            out.len() < MAX_SEARCH_RESULTS
        })
        .map_err(|_| VmError::StepLimit)?;
    Ok(out)
}

fn world_box_of(vm: &Vm, object: u32) -> ([f32; 3], [f32; 3]) {
    SceneView::new(vm).world_box(object)
}

/// Whether a result is still what it was found as: its object exists and, for an index's, is what the index found (object ids
/// come round again after `MAX_DYNAMIC_ID`, and a search list outlives the script that made it, so the id alone could name an
/// unrelated object). The lists keep weak references: a deleted object reads as a dead one.
fn alive<'a>(vm: &'a Vm, container: &'a Container) -> impl Fn(Found) -> bool + 'a {
    let indexes = container.indexes();
    move |found| {
        let objects = vm.objects();
        objects.get(found.object).is_some()
            && found.from.is_none_or(|(number, key)| {
                indexes
                    .get(number)
                    .is_some_and(|index| index.still(objects, found.object, key))
            })
    }
}

/// Pays for `units`, or the error that ends the run.
fn pay(vm: &Vm, units: u64) -> Result<(), VmError> {
    if vm.charge_work(units) {
        Ok(())
    } else {
        Err(VmError::StepLimit)
    }
}

/// `initContainerRadiusSearch(pos, radius, mask)`: the objects whose box is strictly closer than the radius,
/// sorted by the distance to their box's centre.
fn init_radius_search(vm: &mut Vm, container: &Container, state: &State, c: &NativeCall) -> Native {
    super::links::resolve(vm);
    let (pos, radius, mask) = (c.vec3(1), c.float(2), mask_arg(c, 3));
    state.borrow_mut().search.clear();
    let found = match collect(vm, container, radius_box(pos, radius), mask) {
        Ok(found) => found,
        Err(e) => return Native::Fail(e),
    };
    if let Err(e) = pay(vm, CANDIDATE_WORK.saturating_mul(found.len() as u64)) {
        return Native::Fail(e);
    }
    let kept: Vec<Found> = found
        .into_iter()
        .filter(|entry| {
            let (min, max) = world_box_of(vm, entry.object);
            within_radius(box_distance_squared(min, max, pos), radius)
        })
        .collect();
    start_search(vm, state, pos, kept)
}

/// `initContainerBoxSearch(pos, size, mask)`: the objects the box of `size` around `pos` reaches, sorted.
fn init_box_search(vm: &mut Vm, container: &Container, state: &State, c: &NativeCall) -> Native {
    super::links::resolve(vm);
    let (pos, size, mask) = (c.vec3(1), c.vec3(2), mask_arg(c, 3));
    state.borrow_mut().search.clear();
    match collect(vm, container, size_box(pos, size), mask) {
        Ok(found) => start_search(vm, state, pos, found),
        Err(e) => Native::Fail(e),
    }
}

fn start_search(vm: &Vm, state: &State, pos: [f32; 3], found: Vec<Found>) -> Native {
    let mut charge = |units: u64| vm.charge_work(units);
    let started = state.borrow_mut().search.start(
        pos,
        found,
        &|object| world_box_of(vm, object),
        &mut charge,
    );
    match started {
        Ok(()) => Native::Void,
        Err(_) => Native::Fail(VmError::StepLimit),
    }
}

/// The y and z radii of `containerBoxEmpty` (arguments 3 to 5): one radius is all three, two give z the x radius again,
/// three are three.
pub fn radii_args(c: &NativeCall) -> [f32; 3] {
    let x = c.float(3);
    let argc = c.argc();
    let y = if argc > 4 { c.float(4) } else { x };
    let z = if argc > 5 { c.float(5) } else { x };
    [x, y, z]
}

/// `containerBoxEmpty(mask, center, xRadius, yRadius, zRadius)`: true when no object's collision geometry
/// reaches into the box. The engine collects the objects the box reaches, asks each for its collision polygons clipped to the
/// box's six planes (an `EarlyOutPolyList`), and says empty when none is left. Each index answers for its own objects first
/// ([`SpatialIndex::box_blocked`]: an index for its own objects), then the bins' objects are asked for their polygon
fn box_empty(vm: &mut Vm, container: &Container, shapes: &Shapes, c: &NativeCall) -> Native {
    super::links::resolve(vm);
    let query = radii_box(c.vec3(2), radii_args(c));
    let found = match collect(vm, container, query, mask_arg(c, 1)) {
        Ok(found) => found,
        Err(e) => return Native::Fail(e),
    };
    let mut blocked = false;
    for (number, index) in container.indexes().iter().enumerate() {
        match index.box_blocked(vm, shapes, query, number, &found) {
            Ok(true) => blocked = true,
            Ok(false) => {}
            Err(e) => return Native::Fail(e),
        }
    }
    // The other objects the box reaches: each one's `buildPolyList` into the list of the box's six planes, which keeps
    // nothing but the fact that something reached inside (`Container::collideBox` with an `EarlyOutPolyList`).
    if !blocked {
        let mut list = box_list(query);
        for entry in found.iter().filter(|e| e.from.is_none()) {
            if !vm.charge_work(CANDIDATE_WORK) {
                return Native::Fail(VmError::StepLimit);
            }
            let fed = super::rays::animate_for_poly(vm, entry.object)
                .and_then(|()| super::rays::build_poly_list(vm, entry.object, &mut list));
            if let Err(e) = fed {
                return Native::Fail(e);
            }
            if !collide::PolyList::is_empty(&list) {
                blocked = true;
                break;
            }
        }
    }
    Native::Bool(!blocked)
}

/// The `EarlyOutPolyList` of `containerBoxEmpty`: the six planes of the box in their fixed order ([`box_planes`]).
fn box_list(query: [f32; 6]) -> collide::EarlyOutPolyList {
    let mut list = collide::EarlyOutPolyList::new();
    for [x, y, z, d] in box_planes(
        [query[0], query[1], query[2]],
        [query[3], query[4], query[5]],
    ) {
        list.planes.push(collide::Plane { x, y, z, d });
    }
    list
}

/// `containerFindFirst(mask, pos, xRadius, yRadius, zRadius)` and `containerFindNext()`: the
/// objects the box of the absolute radii around `pos` reaches, unsorted, one id per call and an empty string at the end.
fn find_first(vm: &mut Vm, container: &Container, state: &State, c: &NativeCall) -> Native {
    super::links::resolve(vm);
    let radii = [c.float(3), c.float(4), c.float(5)];
    let (query, mask) = (find_box(c.vec3(2), radii), mask_arg(c, 1));
    let found = match collect(vm, container, query, mask) {
        Ok(found) => found,
        Err(e) => return Native::Fail(e),
    };
    let objects: Vec<u32> = found.iter().map(|e| e.object).collect();
    let mut state = state.borrow_mut();
    state.find.start(objects);
    next_found(&mut state)
}

fn next_found(state: &mut ContainerState) -> Native {
    match state.find.next_object() {
        Some(object) => Native::Str(object.to_string().into_bytes()),
        None => Native::Str(Vec::new()),
    }
}

/// What `Container::castRay` found: the object, the fraction, the point (recomputed from the world segment, `ray_point`) and
/// the normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContainerHit {
    pub object: u32,
    pub t: f32,
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

/// The indexes' half of `Container::castRay` and the bins' candidates in the bins' order ([`torque_scene::Bins::cast_ray`]):
/// the nearest hit of the indexes (an earlier index wins a tie) with the index that made it, then the objects whose own
/// `castRay` the caller asks.
type Candidates = (Option<(IndexHit, Rc<dyn SpatialIndex>)>, Vec<u32>);

fn cast_candidates(
    vm: &Vm,
    container: &Container,
    shapes: Option<&Shapes>,
    start: [f32; 3],
    end: [f32; 3],
    mask: u32,
) -> Result<Candidates, VmError> {
    let mut best: Option<(IndexHit, Rc<dyn SpatialIndex>)> = None;
    for index in container.indexes() {
        if let Some(hit) = index.cast_ray(vm, shapes, start, end, mask)?
            && best.as_ref().is_none_or(|(b, _)| hit.hit.t < b.hit.t)
        {
            best = Some((hit, Rc::clone(index)));
        }
    }
    let mut candidates = Vec::new();
    bins_candidates(vm, container, start, end, mask, &mut candidates)?;
    Ok((best, candidates))
}

/// The bins' half of `Container::castRay`: the objects of the mask's bits the bins answer for (the bins' mask, every bit by
/// default) whose box the segment crosses, overflow list first, then the cells the segment steps through
/// ([`torque_scene::Bins::cast_ray`]).
pub fn bins_candidates(
    vm: &Vm,
    container: &Container,
    start: [f32; 3],
    end: [f32; 3],
    mask: u32,
    candidates: &mut Vec<u32>,
) -> Result<(), VmError> {
    let view = SceneView::new(vm);
    let mut charge = |units: u64| vm.charge_work(units);
    container
        .bins()
        .cast_ray(start, end, mask, &view, &mut charge, &mut |object| {
            candidates.push(object);
            Ok(())
        })
        .map_err(|_| VmError::StepLimit)
}

/// Whether all six floats of the segment are finite: `Container::castRay`'s gate (not NaN, within `+-f32max`).
fn finite_segment(start: [f32; 3], end: [f32; 3]) -> bool {
    start.iter().chain(&end).all(|v| v.is_finite())
}

/// `Container::castRay` on the server container: the gate, each index with the raw mask, then the bins (overflow list
/// first, then the cells the segment steps through) for the mask's bits the bins answer for (the bins' mask, every bit by
/// default), each object answering with its own `castRay` ([`super::rays`]; its `animate(detail)` calls first), taken only
/// when strictly nearer than the best so far (an index wins a tie, then the first candidate reached). Charged; a refusal
/// ends the run ([`VmError::StepLimit`]). An index that asked to be told when its hit is the answer is told
/// ([`SpatialIndex::ray_answered`]).
pub fn container_ray(
    vm: &mut Vm,
    container: &Container,
    shapes: Option<&Shapes>,
    start: [f32; 3],
    end: [f32; 3],
    mask: u32,
) -> Result<Option<ContainerHit>, VmError> {
    if !finite_segment(start, end) {
        return Ok(None);
    }
    super::links::resolve(vm);
    let (indexed, candidates) = cast_candidates(vm, container, shapes, start, end, mask)?;
    let mut found = indexed.as_ref().map(|(h, _)| h.hit);
    let mut best = found.map_or(NO_HIT, |h| h.t);
    let mut from_bins = false;
    for object in candidates {
        if vm.objects().get(object).is_none() {
            continue;
        }
        super::rays::animate_for_ray(vm, object)?;
        if let Some(hit) = super::rays::object_ray(vm, object, start, end)?
            && hit.t < best
        {
            best = hit.t;
            from_bins = true;
            found = Some(ContainerHit {
                object,
                t: hit.t,
                point: ray_point(start, end, hit.t),
                normal: hit.normal,
            });
        }
    }
    if let Some((hit, index)) = indexed
        && hit.tell
        && !from_bins
    {
        index.ray_answered();
    }
    Ok(found)
}

/// `disableCollision` on the object's collision-disable count.
pub fn disable_collision(vm: &mut Vm, id: ObjId) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.collision_off = e.collision_off.saturating_add(1);
    }
}

/// `enableCollision`: one off a count that is not zero.
pub fn enable_collision(vm: &mut Vm, id: ObjId) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), id)
        && e.collision_off != 0
    {
        e.collision_off -= 1;
    }
}

/// `containerRayCast(start, end, mask, exempt...)` (over `Container::castRay`): the nearest object the
/// segment crosses, as `"id x y z nx ny nz"`, or `"0"`. Each exempt argument is read with `atoi` (a name reads as 0, which is
/// no object) and, when it is a scene object, has its collision disabled for the cast (the count goes up before and down after,
/// in the order given, so a repeated id counts twice). A segment with a coordinate that is not a finite number is nothing.
fn ray_cast(vm: &mut Vm, container: &Container, shapes: &Shapes, c: &NativeCall) -> Native {
    let (start, end, mask) = (c.vec3(1), c.vec3(2), mask_arg(c, 3));
    let exempt: Vec<u32> = (4..c.argc())
        .filter_map(|i| u32::try_from(c.int(i)).ok())
        .filter(|id| is_class(vm, *id, "SceneObject"))
        .collect();
    for &id in &exempt {
        disable_collision(vm, id);
    }
    let found = container_ray(vm, container, Some(shapes), start, end, mask);
    for &id in &exempt {
        enable_collision(vm, id);
    }
    match found {
        Err(e) => Native::Fail(e),
        Ok(None) => Native::Str(b"0".to_vec()),
        Ok(Some(hit)) => {
            let g = |v: f32| format_g(to_f64(v));
            let [px, py, pz] = hit.point;
            let [nx, ny, nz] = hit.normal;
            let text = format!(
                "{} {} {} {} {} {} {}",
                hit.object,
                g(px),
                g(py),
                g(pz),
                g(nx),
                g(ny),
                g(nz)
            );
            Native::Str(text.into_bytes())
        }
    }
}

// The usage text of the two box calls (they share it).
/// `containerBoxEmpty`'s usage line (a host's box query of the same arguments may share it).
pub const BOX_USAGE: &str = "(bitset mask, Point3F center, float xRadius, float yRadius, float zRadius)See if any objects of given types are present in box of given extent.\n\n@note Extent parameter is last since only one radius is often needed. If one radius is provided, the yRadius and zRadius are assumed to be the same.\n@param  mask   Indicates the type of objects we are checking against.\n@param  center Center of box.\n@param  xRadius See above.\n@param  yRadius See above.\n@param  zRadius See above.";

const RAY_USAGE: &str = "( Point3F start, Point3F end, bitset mask, SceneObject exempt=NULL,exempt2,exempt3,exempt4,exempt5,exempt6 )Cast a ray from start to end, checking for collision against items matching mask.\n\nIf exempt is specified, then it is temporarily excluded from collision checks (For instance, you might want to exclude the player if said player was firing a weapon.)\n@returns A string containing either null, if nothing was struck, or these fields:\n            - The ID of the object that was struck.\n            - The x, y, z position that it was struck.\n            - The x, y, z of the normal of the face that was struck.";

/// Registers the container natives.
pub fn register_natives(vm: &mut Vm, container: &Rc<Container>, shapes: &Rc<Shapes>) {
    let state: State = Rc::default();
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "initContainerRadiusSearch",
        4,
        4,
        "(Point3F pos, float radius, bitset mask)Start a search for items within radius of pos, filtering by bitset mask.",
        move |vm, c| init_radius_search(vm, &w, &s, c),
    );
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "initContainerBoxSearch",
        4,
        4,
        "(Point3F pos, Point3F boxSize, bitset mask)Start a search for items within a box, filtering by bitset mask.",
        move |vm, c| init_box_search(vm, &w, &s, c),
    );
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "containerSearchNext",
        1,
        1,
        "Get next item from a search started with initContainerRadiusSearch.",
        move |vm, _| {
            let alive = alive(vm, &w);
            let next = s.borrow_mut().search.next(&alive);
            Native::Int(next.map_or(0, |e| i32::try_from(e.object).unwrap_or(0)))
        },
    );
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "containerSearchCurrDist",
        1,
        1,
        "Get distance of the center of the current item from the center of the current initContainerRadiusSearch.",
        move |vm, _| {
            let state = s.borrow();
            let value = state.search.current(&alive(vm, &w)).map_or(0.0, |entry| {
                let (min, max) = world_box_of(vm, entry.object);
                center_distance(min, max, state.search.center())
            });
            Native::Float(to_f64(value))
        },
    );
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "containerSearchCurrRadiusDist",
        1,
        1,
        "Get the distance of the closest point of the current item from the center of the current initContainerRadiusSearch.",
        move |vm, _| {
            let state = s.borrow();
            let value = state.search.current(&alive(vm, &w)).map_or(0.0, |entry| {
                let (min, max) = world_box_of(vm, entry.object);
                radius_distance(min, max, state.search.center())
            });
            Native::Float(to_f64(value))
        },
    );
    let (w, sh) = (Rc::clone(container), Rc::clone(shapes));
    register(vm, "containerBoxEmpty", 4, 6, BOX_USAGE, move |vm, c| {
        box_empty(vm, &w, &sh, c)
    });
    let (w, s) = (Rc::clone(container), Rc::clone(&state));
    register(
        vm,
        "containerFindFirst",
        6,
        6,
        "(bitset type, Point3F point, float x, float y, float z)Find objects matching the bitmask type within a box centered at point, with extents x, y, z.

Returns the first object found; thereafter, you can get more results using containerFindNext().",
        move |vm, c| {
        find_first(vm, &w, &s, c)
    });
    let s = Rc::clone(&state);
    register(
        vm,
        "containerFindNext",
        1,
        1,
        "Get more results from a previous call to containerFindFirst().",
        move |_, _| next_found(&mut s.borrow_mut()),
    );
    let (w, sh) = (Rc::clone(container), Rc::clone(shapes));
    register(vm, "containerRayCast", 4, 10, RAY_USAGE, move |vm, c| {
        ray_cast(vm, &w, &sh, c)
    });
    let (w, sh) = (Rc::clone(container), Rc::clone(shapes));
    register(
        vm,
        "calcExplosionCoverage",
        4,
        4,
        "(Point3F source, SceneObject originator, bitset coverageMask)",
        move |vm, c| explosion_coverage(vm, &w, &sh, c),
    );
}

/// `calcExplosionCoverage(source, object, mask)` (console record, body; a script's radius damage scales an explosion's
/// damage by it): the object is found by `atoi` of its argument (a name reads as 0, no object) and must be a scene object in
/// the container (else a warning and 1). Its centre is the object box's, `f32(f32((min + max) * 0.5) * scale)` per axis, moved
/// by the transform (`mulP`). With the object's collision disabled: no hit of `mask` from the source to the centre is 1; else a
/// ray from the source one unit up that is clear, then a clear ray from there to the centre, is 1; else 0. **Engine-only:** a
/// source `sscanf` cannot read is zeros, and "in the container" is the bins or a spatial index.
fn explosion_coverage(
    vm: &mut Vm,
    container: &Container,
    shapes: &Shapes,
    c: &NativeCall,
) -> Native {
    let source = c.vec3(1);
    let mask = mask_arg(c, 3);
    let id = u32::try_from(c.int(2))
        .ok()
        .filter(|id| is_class(vm, *id, "SceneObject"));
    let Some(id) = id else {
        vm.log_bytes(
            Level::Warning,
            &[
                &b"calcExplosionCoverage: couldn't find object: "[..],
                c.arg(2),
            ]
            .concat(),
        );
        return Native::Float(1.0);
    };
    let in_container = container.bins().contains(id)
        || container.indexes().iter().any(|index| index.holds(vm, id));
    if !in_container {
        vm.log_bytes(
            Level::Warning,
            b"calcExplosionCoverage: object is on the client, or not in the container system",
        );
        return Native::Float(1.0);
    }
    let (low, high) = ext::get(vm.objects(), id)
        .and_then(|e| e.scene)
        .map_or(([0.0; 3], [0.0; 3]), |sc| (sc.object_min, sc.object_max));
    let scale = scale_of(vm, id);
    let half = torque_scene::transform::box_center(low, high);
    let center: [f32; 3] = std::array::from_fn(|i| half[i] * scale[i]);
    let center = collide::math::mul_p(
        &matrix_of(vm, id).unwrap_or(collide::math::IDENTITY),
        center,
    );
    let up = [source[0] + 0.0, source[1] + 0.0, source[2] + 1.0];
    disable_collision(vm, id);
    let covered = (|| -> Result<bool, VmError> {
        if container_ray(vm, container, Some(shapes), source, center, mask)?.is_none() {
            return Ok(true);
        }
        Ok(
            container_ray(vm, container, Some(shapes), source, up, mask)?.is_none()
                && container_ray(vm, container, Some(shapes), up, center, mask)?.is_none(),
        )
    })();
    enable_collision(vm, id);
    match covered {
        Ok(true) => Native::Float(1.0),
        Ok(false) => Native::Float(0.0),
        Err(e) => Native::Fail(e),
    }
}
