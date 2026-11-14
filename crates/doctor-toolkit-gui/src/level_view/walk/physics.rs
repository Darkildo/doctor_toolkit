//! Physics for walking a level: the Havok shapes of the placed objects as
//! static `rapier3d` colliders, and a capsule moved by rapier's kinematic
//! character controller (gravity, steps, slopes, jumps).
//!
//! Shapes keep their kind where rapier has one: spheres, boxes, capsules,
//! cylinders and convex hulls stay solid primitives, Havok meshes (simple and
//! extended, under MOPP) become triangle meshes. A shape placed with a scale
//! (the object matrix is not a rotation) is rebuilt with the scale applied:
//! primitives then become convex hulls of their tessellation.
//!
//! Only scene queries are used (no simulation step): the colliders have no
//! bodies and the bounding volume tree is built once over all of them.

use std::sync::Arc;

use doctor_toolkit::level::RowMatrix;
use doctor_toolkit::resource::hkx::geometry::shape_mesh;
use doctor_toolkit::resource::hkx::physics::{Physics, Shape, ShapeKind};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::glamx::{Mat3, Quat};
use rapier3d::parry::bounding_volume::BoundingVolume;
use rapier3d::parry::partitioning::{Bvh, BvhBuildStrategy};
use rapier3d::parry::query::DefaultQueryDispatcher;
use rapier3d::prelude::{
    Aabb, Collider, ColliderBuilder, ColliderHandle, ColliderSet, Pose, QueryFilter, QueryPipeline,
    Ray, RigidBodySet, SharedShape, Vector,
};

pub type V3 = [f32; 3];

fn v(p: V3) -> Vector {
    Vector::new(p[0], p[1], p[2])
}

fn a(p: Vector) -> V3 {
    [p.x, p.y, p.z]
}

/// An affine map `p ↦ cols · p + t` (columns of the linear part).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub cols: [V3; 3],
    pub t: V3,
}

impl Affine {
    pub const IDENTITY: Self = Self {
        cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    /// A row-vector matrix of the level (rows right, up, at, position).
    pub fn from_rows(m: &RowMatrix) -> Self {
        let r = |i: usize| [m[i][0], m[i][1], m[i][2]];
        Self {
            cols: [r(0), r(1), r(2)],
            t: r(3),
        }
    }

    fn translation(t: V3) -> Self {
        Self {
            t,
            ..Self::IDENTITY
        }
    }

    /// A rotation by the quaternion `q` (`x, y, z, w`), then a translation.
    fn rotation(q: [f32; 4], t: V3) -> Self {
        let m = Mat3::from_quat(Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize());
        Self {
            cols: [a(m.x_axis), a(m.y_axis), a(m.z_axis)],
            t,
        }
    }

    pub fn apply(&self, p: V3) -> V3 {
        let c = &self.cols;
        [0, 1, 2].map(|i| c[0][i] * p[0] + c[1][i] * p[1] + c[2][i] * p[2] + self.t[i])
    }

    fn linear(&self, p: V3) -> V3 {
        let c = &self.cols;
        [0, 1, 2].map(|i| c[0][i] * p[0] + c[1][i] * p[1] + c[2][i] * p[2])
    }

    /// `inner` first, then `self`.
    fn after(&self, inner: &Affine) -> Affine {
        Affine {
            cols: inner.cols.map(|c| self.linear(c)),
            t: self.apply(inner.t),
        }
    }

    /// The map as a pose when it is a rotation (no scale, no mirror).
    pub fn rigid(&self) -> Option<Pose> {
        let m = Mat3::from_cols(v(self.cols[0]), v(self.cols[1]), v(self.cols[2]));
        let ortho = (m.transpose() * m - Mat3::IDENTITY)
            .to_cols_array()
            .iter()
            .all(|e| e.abs() < 2e-3);
        if !ortho || m.determinant() <= 0.0 {
            return None;
        }
        let rotation = Quat::from_mat3(&m).normalize();
        Some(Pose::from_parts(v(self.t), rotation))
    }
}

/// Shapes of a collision in one frame, each with its pose.
pub type Parts = Vec<(Pose, SharedShape)>;

/// The shapes of every body of `physics`, mapped by `at` (model space to
/// the frame wanted). With a rigid `at` primitives stay primitives.
pub fn physics_parts(physics: &Physics, at: &Affine) -> Parts {
    let mut out = Vec::new();
    for body in physics.bodies() {
        let body_at = Affine {
            cols: body.rotation,
            t: body.translation,
        };
        shape_parts(&body.shape, &at.after(&body_at), &mut out);
    }
    out
}

fn xyz(p: [f32; 4]) -> V3 {
    [p[0], p[1], p[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Appends the parts of `shape` placed by `at`.
pub fn shape_parts(shape: &Shape, at: &Affine, out: &mut Parts) {
    let rigid = at.rigid();
    let hull = |points: Vec<V3>, out: &mut Parts| {
        let points: Vec<Vector> = points.into_iter().map(|p| v(at.apply(p))).collect();
        if let Some(s) = SharedShape::convex_hull(&points) {
            out.push((Pose::IDENTITY, s));
        }
    };
    let tessellated = |out: &mut Parts| hull(shape_mesh(shape).positions, out);
    match &shape.kind {
        ShapeKind::Sphere { radius } => match rigid {
            Some(pose) => out.push((pose, SharedShape::ball(*radius))),
            None => tessellated(out),
        },
        ShapeKind::Box { half_extents, .. } => match rigid {
            Some(pose) => out.push((
                pose,
                SharedShape::cuboid(half_extents[0], half_extents[1], half_extents[2]),
            )),
            None => tessellated(out),
        },
        ShapeKind::Capsule { radius, a, b } => match rigid {
            Some(pose) => out.push((pose, SharedShape::capsule(v(xyz(*a)), v(xyz(*b)), *radius))),
            None => tessellated(out),
        },
        ShapeKind::Cylinder {
            cylinder_radius,
            a,
            b,
            ..
        } => {
            let (a, b) = (xyz(*a), xyz(*b));
            let axis = sub(b, a);
            let half = length(axis) / 2.0;
            match rigid {
                Some(pose) if half > 1e-5 => {
                    // rapier's cylinder runs along Y around its centre.
                    let mid = [0, 1, 2].map(|i| (a[i] + b[i]) / 2.0);
                    let turn = Quat::from_rotation_arc(Vector::Y, v(axis).normalize());
                    let local = Pose::from_parts(v(mid), turn);
                    out.push((pose * local, SharedShape::cylinder(half, *cylinder_radius)));
                }
                _ => tessellated(out),
            }
        }
        ShapeKind::ConvexVertices { vertices, .. } => match rigid {
            Some(pose) => {
                let points: Vec<Vector> = vertices.iter().map(|p| v(*p)).collect();
                if let Some(s) = SharedShape::convex_hull(&points) {
                    out.push((pose, s));
                }
            }
            None => hull(vertices.clone(), out),
        },
        ShapeKind::ConvexTranslate {
            translation, child, ..
        } => shape_parts(
            child,
            &at.after(&Affine::translation(xyz(*translation))),
            out,
        ),
        ShapeKind::ConvexTransform {
            transform, child, ..
        } => {
            let inner = Affine {
                cols: [xyz(transform[0]), xyz(transform[1]), xyz(transform[2])],
                t: xyz(transform[3]),
            };
            shape_parts(child, &at.after(&inner), out)
        }
        ShapeKind::List { children, .. } => {
            for child in children {
                shape_parts(child, at, out);
            }
        }
        ShapeKind::Mopp { child, .. } => shape_parts(child, at, out),
        ShapeKind::SimpleMesh {
            vertices,
            triangles,
            ..
        } => {
            let n = vertices.len() as u32;
            let tris: Vec<[u32; 3]> = triangles
                .iter()
                .copied()
                .filter(|t| t.iter().all(|&i| i < n))
                .collect();
            push_mesh(vertices.iter().map(|p| xyz(*p)).collect(), tris, at, out);
        }
        ShapeKind::ExtendedMesh(m) => {
            let s = xyz(m.scaling);
            for part in &m.triangle_subparts {
                let n = part.vertices.len() as u32;
                let tris: Vec<[u32; 3]> = part
                    .indices
                    .iter()
                    .map(|i| [i[0], i[1], i[2]])
                    .filter(|t| t.iter().all(|&i| i < n))
                    .collect();
                let points = part
                    .vertices
                    .iter()
                    .map(|p| [p[0] * s[0], p[1] * s[1], p[2] * s[2]])
                    .collect();
                push_mesh(points, tris, at, out);
            }
            for part in &m.shape_subparts {
                let inner = Affine::rotation(part.rotation, xyz(part.translation));
                for child in &part.shapes {
                    shape_parts(child, &at.after(&inner), out);
                }
            }
        }
    }
}

/// A triangle mesh placed by `at`: posed when `at` is rigid, otherwise with
/// the map applied to the vertices.
fn push_mesh(points: Vec<V3>, triangles: Vec<[u32; 3]>, at: &Affine, out: &mut Parts) {
    // Degenerate triangles make rapier refuse the whole mesh.
    let triangles: Vec<[u32; 3]> = triangles
        .into_iter()
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    if triangles.is_empty() {
        return;
    }
    let (pose, points) = match at.rigid() {
        Some(pose) => (pose, points.into_iter().map(v).collect()),
        None => (
            Pose::IDENTITY,
            points.into_iter().map(|p| v(at.apply(p))).collect(),
        ),
    };
    if let Ok(s) = SharedShape::trimesh(points, triangles) {
        out.push((pose, s));
    }
}

/// What a collider belongs to (an index into [`World::owners`] is its user data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    /// 0: the loaded level; 1: ground zones loaded for walking.
    pub layer: usize,
    /// The level object (group, object).
    pub object: (usize, usize),
    pub hkx: u32,
}

/// Static colliders and the queries over them.
pub struct World {
    colliders: ColliderSet,
    bodies: RigidBodySet,
    bvh: Bvh,
    pub owners: Vec<Owner>,
    /// Bounds of everything, `None` when empty.
    pub bounds: Option<Aabb>,
}

/// Placed shapes of one owner.
pub struct Placed {
    pub owner: Owner,
    /// Shapes in world space: the shared shapes of the collision (cached
    /// per `hkx`) with poses mapped by the object's matrix.
    pub parts: Arc<Parts>,
    pub pose: Pose,
}

impl World {
    pub fn new(placed: &[Placed]) -> Self {
        let mut colliders = ColliderSet::new();
        let mut owners = Vec::new();
        let mut leaves = Vec::new();
        let mut bounds: Option<Aabb> = None;
        for p in placed {
            let owner = owners.len();
            owners.push(p.owner);
            for (local, shape) in p.parts.iter() {
                let collider: Collider = ColliderBuilder::new(shape.clone())
                    .position(p.pose * *local)
                    .user_data(owner as u128)
                    .build();
                let aabb = collider.compute_aabb();
                bounds = Some(match bounds {
                    Some(b) => b.merged(&aabb),
                    None => aabb,
                });
                let handle = colliders.insert(collider);
                leaves.push((handle.into_raw_parts().0 as usize, aabb));
            }
        }
        let bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves);
        Self {
            colliders,
            bodies: RigidBodySet::new(),
            bvh,
            owners,
            bounds,
        }
    }

    pub fn collider_count(&self) -> usize {
        self.colliders.len()
    }

    pub fn queries(&self) -> QueryPipeline<'_> {
        QueryPipeline {
            dispatcher: &DefaultQueryDispatcher,
            bvh: &self.bvh,
            bodies: &self.bodies,
            colliders: &self.colliders,
            filter: QueryFilter::default(),
        }
    }

    /// The owner of a collider.
    pub fn owner(&self, handle: ColliderHandle) -> Option<Owner> {
        let c = self.colliders.get(handle)?;
        self.owners.get(c.user_data as usize).copied()
    }

    /// Bounds of the colliders of a level object.
    pub fn object_bounds(&self, layer: usize, object: (usize, usize)) -> Option<Aabb> {
        self.colliders
            .iter()
            .filter(|(_, c)| {
                self.owners
                    .get(c.user_data as usize)
                    .is_some_and(|o| o.layer == layer && o.object == object)
            })
            .map(|(_, c)| c.compute_aabb())
            .reduce(|a, b| a.merged(&b))
    }

    /// The first thing straight below `from` within `max` metres: distance
    /// and owner.
    pub fn below(&self, from: V3, max: f32) -> Option<(f32, Option<Owner>)> {
        let ray = Ray::new(v(from), Vector::NEG_Y);
        let (handle, toi) = self.queries().cast_ray(&ray, max, true)?;
        Some((toi, self.owner(handle)))
    }

    /// The first thing straight below `from` within `max` metres whose
    /// owner `skip` does not pass over: its distance.
    pub fn below_except(&self, from: V3, max: f32, skip: &dyn Fn(Owner) -> bool) -> Option<f32> {
        let predicate = |_: ColliderHandle, c: &Collider| {
            !self
                .owners
                .get(c.user_data as usize)
                .is_some_and(|o| skip(*o))
        };
        let mut queries = self.queries();
        queries.filter = QueryFilter::default().predicate(&predicate);
        let ray = Ray::new(v(from), Vector::NEG_Y);
        queries.cast_ray(&ray, max, true).map(|(_, toi)| toi)
    }

    /// The first collider along the ray from `from` in the direction `dir`
    /// (unit length) within `max` metres: the distance and the normal there.
    pub fn cast(&self, from: V3, dir: V3, max: f32) -> Option<(f32, V3)> {
        let ray = Ray::new(v(from), v(dir));
        self.queries()
            .cast_ray_and_get_normal(&ray, max, true)
            .map(|(_, hit)| (hit.time_of_impact, a(hit.normal)))
    }

    /// What the capsule of `body` with feet at `feet` overlaps, if anything.
    pub fn overlapping(&self, body: &BodySize, feet: V3) -> Option<Owner> {
        let shape = body.shape();
        let (handle, _) = self
            .queries()
            .intersect_shape(body.pose(feet), &*shape)
            .next()?;
        self.owner(handle)
    }
}

/// The player capsule and how it moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodySize {
    /// Height from feet to the top of the head, metres.
    pub height: f32,
    pub radius: f32,
    /// Eye height above the feet.
    pub eye: f32,
    /// Highest step climbed without a jump.
    pub step: f32,
    /// Steepest slope walked up, degrees.
    pub max_slope: f32,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub jump_speed: f32,
    pub gravity: f32,
}

impl Default for BodySize {
    /// A man of 1.8 m: the game's player capsule is not read from its data
    /// (the character proxy is built in code); the radius agrees with the
    /// stage 18 check in the game, where the player slid along a 0.3 m wall
    /// 0.5–0.65 m from its axis (`docs/analysis/game-bench.md` §13.4).
    fn default() -> Self {
        Self {
            height: 1.8,
            radius: 0.35,
            eye: 1.65,
            step: 0.45,
            max_slope: 50.0,
            walk_speed: 1.6,
            run_speed: 5.5,
            jump_speed: 4.5,
            gravity: 9.81,
        }
    }
}

impl BodySize {
    pub fn shape(&self) -> SharedShape {
        let half = ((self.height - 2.0 * self.radius) / 2.0).max(0.01);
        SharedShape::capsule_y(half, self.radius)
    }

    /// Pose of the capsule centre for feet at `feet`.
    pub fn pose(&self, feet: V3) -> Pose {
        Pose::from_translation(v([feet[0], feet[1] + self.height / 2.0, feet[2]]))
    }

    fn controller(&self) -> KinematicCharacterController {
        KinematicCharacterController {
            offset: CharacterLength::Absolute(0.02),
            slide: true,
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(self.step),
                min_width: CharacterLength::Absolute(0.15),
                include_dynamic_bodies: false,
            }),
            max_slope_climb_angle: self.max_slope.to_radians(),
            min_slope_slide_angle: (self.max_slope + 5.0).to_radians(),
            snap_to_ground: Some(CharacterLength::Absolute(0.3)),
            ..KinematicCharacterController::default()
        }
    }
}

/// The walking player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Player {
    pub feet: V3,
    /// Vertical speed, metres per second (up positive).
    pub vertical: f32,
    pub grounded: bool,
    /// Seconds in the air.
    pub airborne: f32,
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StepResult {
    /// Horizontal distance wanted and made.
    pub wanted: f32,
    pub made: f32,
}

impl Player {
    pub fn new(feet: V3) -> Self {
        Self {
            feet,
            vertical: 0.0,
            grounded: false,
            airborne: 0.0,
        }
    }

    /// Moves by the horizontal velocity `wish` (metres per second) for `dt`
    /// seconds with gravity; `jump` jumps from the ground. Long frames are
    /// split into steps of at most 1/60 s.
    pub fn step(
        &mut self,
        world: &World,
        body: &BodySize,
        wish: [f32; 2],
        jump: bool,
        dt: f32,
    ) -> StepResult {
        let steps = (dt * 60.0).ceil().clamp(1.0, 12.0) as usize;
        let h = dt.min(0.2) / steps as f32;
        let controller = body.controller();
        let shape = body.shape();
        let queries = world.queries();
        let mut result = StepResult::default();
        let mut jump = jump;
        for _ in 0..steps {
            if jump && self.grounded {
                self.vertical = body.jump_speed;
                self.grounded = false;
            }
            jump = false;
            self.vertical -= body.gravity * h;
            let desired = Vector::new(wish[0] * h, self.vertical * h, wish[1] * h);
            let moved =
                controller.move_shape(h, &queries, &*shape, &body.pose(self.feet), desired, |_| {});
            let t = moved.translation;
            self.feet = [self.feet[0] + t.x, self.feet[1] + t.y, self.feet[2] + t.z];
            result.wanted += (desired.x * desired.x + desired.z * desired.z).sqrt();
            result.made += (t.x * t.x + t.z * t.z).sqrt();
            self.grounded = moved.grounded;
            if moved.grounded && self.vertical <= 0.0 {
                self.vertical = 0.0;
                self.airborne = 0.0;
            } else {
                self.airborne += h;
                // Bumped the head.
                if self.vertical > 0.0 && t.y < desired.y * 0.5 {
                    self.vertical = 0.0;
                }
            }
        }
        result
    }
}

/// How a walk from A to B ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Reached,
    /// No progress for a second at this point.
    Stuck(V3),
    /// Fell below everything at this point.
    Fell(V3),
    /// Out of time at this point.
    TimedOut(V3),
}

/// A straight walk from `from` towards `to`: the player turns to B every
/// step and the controller slides along what is in the way. Returns the
/// outcome and the path (feet positions, one per 0.1 s).
pub fn walk_to(world: &World, body: &BodySize, from: V3, to: V3) -> (Outcome, Vec<V3>) {
    let dt = 1.0 / 60.0;
    let mut player = Player::new(from);
    let mut path = vec![from];
    let distance = length(sub(to, from));
    let limit = distance / body.walk_speed * 3.0 + 5.0;
    let floor = world.bounds.map_or(-1e4, |b| b.mins.y) - 10.0;
    let mut time = 0.0;
    let mut last_progress = (time, horizontal(sub(to, from)));
    while time < limit {
        let d = sub(to, player.feet);
        let left = horizontal(d);
        if left <= body.radius + 0.25 {
            path.push(player.feet);
            return (Outcome::Reached, path);
        }
        let speed = body.walk_speed.min(left / dt);
        let wish = [d[0] / left * speed, d[2] / left * speed];
        player.step(world, body, wish, false, dt);
        time += dt;
        if (time * 10.0).floor() != ((time - dt) * 10.0).floor() {
            path.push(player.feet);
        }
        if player.feet[1] < floor {
            path.push(player.feet);
            return (Outcome::Fell(player.feet), path);
        }
        if left < last_progress.1 - 0.1 {
            last_progress = (time, left);
        } else if time - last_progress.0 > 1.0 {
            path.push(player.feet);
            return (Outcome::Stuck(player.feet), path);
        }
    }
    path.push(player.feet);
    (Outcome::TimedOut(player.feet), path)
}

fn horizontal(d: V3) -> f32 {
    (d[0] * d[0] + d[2] * d[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::resource::hkx::physics::Physics;

    fn fixed(shapes: Vec<ShapeKind>) -> Physics {
        Physics::fixed(
            shapes.into_iter().map(|s| (None, s.into())).collect(),
            Vec::new(),
        )
    }

    fn world(physics: &Physics, matrix: &RowMatrix) -> World {
        let at = Affine::from_rows(matrix);
        let (parts, pose) = match at.rigid() {
            Some(pose) => (physics_parts(physics, &Affine::IDENTITY), pose),
            None => (physics_parts(physics, &at), Pose::IDENTITY),
        };
        World::new(&[Placed {
            owner: Owner {
                layer: 0,
                object: (0, 0),
                hkx: 1,
            },
            parts: Arc::new(parts),
            pose,
        }])
    }

    /// A floor 40 × 40 m with its top at y = 0, as a Havok mesh.
    fn floor_mesh() -> ShapeKind {
        let s = 20.0;
        ShapeKind::SimpleMesh {
            radius: 0.0,
            welding_type: 6,
            vertices: vec![
                [-s, 0.0, -s, 0.0],
                [s, 0.0, -s, 0.0],
                [s, 0.0, s, 0.0],
                [-s, 0.0, s, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            welding: vec![0, 0],
            material_indices: Vec::new(),
        }
    }

    fn cube(half: V3, at: V3) -> ShapeKind {
        ShapeKind::ConvexTranslate {
            radius: 0.05,
            translation: [at[0], at[1], at[2], 0.0],
            child: Box::new(
                ShapeKind::Box {
                    radius: 0.05,
                    half_extents: [half[0], half[1], half[2], 0.0],
                }
                .into(),
            ),
        }
    }

    const IDENTITY: RowMatrix = doctor_toolkit::level::IDENTITY;

    fn settle(world: &World, body: &BodySize, player: &mut Player, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            player.step(world, body, [0.0, 0.0], false, 1.0 / 60.0);
        }
    }

    #[test]
    fn falls_onto_a_floor_and_stands() {
        let w = world(&fixed(vec![floor_mesh()]), &IDENTITY);
        let body = BodySize::default();
        let mut player = Player::new([0.0, 5.0, 0.0]);
        settle(&w, &body, &mut player, 2.0);
        assert!(player.grounded, "{player:?}");
        assert!(player.feet[1].abs() < 0.05, "{player:?}");
        let (toi, owner) = w.below([0.0, 1.0, 0.0], 10.0).unwrap();
        assert!((toi - 1.0).abs() < 1e-4);
        assert_eq!(owner.unwrap().hkx, 1);
        // Nothing below the floor's edge.
        assert!(w.below([30.0, 1.0, 0.0], 10.0).is_none());
    }

    #[test]
    fn walks_up_a_step_but_not_a_wall() {
        let body = BodySize::default();
        // A step of 0.3 m from x = 2 on, and a wall 2 m high from z = 2 on.
        let w = world(
            &fixed(vec![
                floor_mesh(),
                cube([4.0, 0.15, 10.0], [6.0, 0.15, -12.0]),
                cube([10.0, 1.0, 0.25], [0.0, 1.0, 2.25]),
            ]),
            &IDENTITY,
        );
        let mut player = Player::new([0.0, 0.5, -12.0]);
        settle(&w, &body, &mut player, 1.0);
        for _ in 0..240 {
            player.step(&w, &body, [body.walk_speed, 0.0], false, 1.0 / 60.0);
        }
        assert!(player.feet[0] > 4.0, "stepped up: {player:?}");
        assert!(
            (player.feet[1] - 0.3).abs() < 0.06,
            "on the step: {player:?}"
        );
        // Into the wall: stopped before it.
        let mut player = Player::new([0.0, 0.5, -2.0]);
        settle(&w, &body, &mut player, 1.0);
        for _ in 0..300 {
            player.step(&w, &body, [0.0, body.walk_speed], false, 1.0 / 60.0);
        }
        assert!(
            player.feet[2] < 2.0 - body.radius + 0.05,
            "stopped by the wall: {player:?}"
        );
        assert!(player.feet[1].abs() < 0.05);
        // A jump gets on top of a 0.8 m box, which is too high a step.
        let w = world(
            &fixed(vec![floor_mesh(), cube([1.0, 0.4, 1.0], [3.0, 0.4, 0.0])]),
            &IDENTITY,
        );
        let mut player = Player::new([0.0, 0.1, 0.0]);
        settle(&w, &body, &mut player, 0.5);
        for _ in 0..120 {
            player.step(&w, &body, [body.walk_speed, 0.0], false, 1.0 / 60.0);
        }
        assert!(player.feet[0] < 2.0 && player.feet[1] < 0.1, "{player:?}");
        player.step(&w, &body, [body.walk_speed, 0.0], true, 1.0 / 60.0);
        for _ in 0..60 {
            player.step(&w, &body, [body.walk_speed, 0.0], false, 1.0 / 60.0);
        }
        settle(&w, &body, &mut player, 1.0);
        assert!(player.feet[0] > 2.0 && player.grounded, "{player:?}");
        assert!(
            (player.feet[1] - 0.8).abs() < 0.06,
            "on the box: {player:?}"
        );
    }

    #[test]
    fn scaled_and_rotated_objects_keep_their_shapes() {
        // A box of 1 m scaled by 2 and moved: its top is at 2 m.
        let scaled: RowMatrix = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 2.0, 0.0],
            [10.0, 1.0, 0.0, 1.0],
        ];
        let w = world(&fixed(vec![cube([0.5, 0.5, 0.5], [0.0; 3])]), &scaled);
        let (toi, _) = w.below([10.5, 5.0, 0.5], 10.0).unwrap();
        assert!((toi - 3.0).abs() < 1e-3, "{toi}");
        // Turned 90° about Y: a long thin box along X runs along Z.
        let turned: RowMatrix = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!(Affine::from_rows(&turned).rigid().is_some());
        let w = world(&fixed(vec![cube([3.0, 0.5, 0.1], [0.0; 3])]), &turned);
        assert!(w.below([0.0, 5.0, 2.5], 10.0).is_some());
        assert!(w.below([2.5, 5.0, 0.0], 10.0).is_none());
        // Cylinders stand along their axis; spheres and capsules are solid.
        let w = world(
            &fixed(vec![
                ShapeKind::Cylinder {
                    radius: 0.05,
                    cylinder_radius: 0.5,
                    a: [0.0, 0.0, 0.0, 0.0],
                    b: [0.0, 0.0, 4.0, 0.0],
                },
                ShapeKind::Sphere { radius: 1.0 },
                ShapeKind::Capsule {
                    radius: 0.5,
                    a: [5.0, 0.0, 0.0, 0.0],
                    b: [5.0, 3.0, 0.0, 0.0],
                },
            ]),
            &IDENTITY,
        );
        let (toi, _) = w.below([0.0, 5.0, 3.5], 10.0).unwrap();
        assert!((toi - 4.5).abs() < 1e-3, "{toi}");
        let (toi, _) = w.below([0.0, 5.0, -0.5], 10.0).unwrap();
        assert!((toi - (5.0 - 0.75f32.sqrt())).abs() < 1e-3, "{toi}");
        let (toi, _) = w.below([5.0, 5.0, 0.0], 10.0).unwrap();
        assert!((toi - 1.5).abs() < 1e-3, "{toi}");
    }

    #[test]
    fn walks_from_a_to_b_or_reports_where_it_stopped() {
        let body = BodySize::default();
        let w = world(
            &fixed(vec![floor_mesh(), cube([0.25, 1.5, 3.0], [5.0, 1.5, 0.0])]),
            &IDENTITY,
        );
        // Open floor.
        let (outcome, path) = walk_to(&w, &body, [-5.0, 0.0, 10.0], [8.0, 0.0, 10.0]);
        assert_eq!(outcome, Outcome::Reached, "{path:?}");
        // A wall 6 m wide, B straight behind its middle: the walk stops.
        let (outcome, _) = walk_to(&w, &body, [0.0, 0.0, 0.0], [8.0, 0.0, 0.0]);
        let Outcome::Stuck(at) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(at[0] < 5.0 && at[0] > 3.5, "{at:?}");
        // Off the edge of the floor: falls.
        let (outcome, _) = walk_to(&w, &body, [15.0, 0.0, 15.0], [25.0, 0.0, 15.0]);
        assert!(matches!(outcome, Outcome::Fell(_)), "{outcome:?}");
    }
}
