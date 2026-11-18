//! Renderer benchmark without a window.
//!
//! `DOCTOR_TOOLKIT_GUI_BENCH=instances:<n>` draws `n` instances of a few
//! synthetic meshes; `DOCTOR_TOOLKIT_GUI_BENCH=model:<stream>:<index>` draws a
//! game model (the game directory is the first argument). Each frame orbits
//! the camera, renders 1600×900 pixels and waits for the GPU; the median,
//! 99th percentile and maximum frame times are printed with the pick time.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::render::{
    GpuContext, Instance, MaterialData, MeshData, OrbitCamera, Scene, SceneRenderer, Vertex,
    Viewport, math,
};

const SIZE: [u32; 2] = [1600, 900];
/// Measured frames; `DOCTOR_TOOLKIT_GUI_BENCH_FRAMES` overrides (software renderers are slow).
fn frames() -> usize {
    std::env::var("DOCTOR_TOOLKIT_GUI_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
        .max(1)
}

pub fn run(spec: &str, game_dir: Option<&Path>) -> i32 {
    let Some(context) = GpuContext::headless() else {
        eprintln!("bench: no wgpu adapter");
        return 2;
    };
    println!("adapter: {}", context.adapter_summary());
    let mut renderer = SceneRenderer::new(context);
    println!("batching: {:?}", renderer.batching());
    renderer.set_mesh_budget(1024);
    // The zones of a level may name presets (`level:preset:miami`), so the
    // rest of the text is theirs.
    if let Some(zones) = spec.strip_prefix("level:") {
        let Some(dir) = game_dir else {
            eprintln!("bench: pass the game directory as the first argument");
            return 2;
        };
        return match level(&mut renderer, dir, zones) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("bench: {error}");
                1
            }
        };
    }
    let (scene, bounds) = match spec.split(':').collect::<Vec<_>>().as_slice() {
        ["instances", n] => {
            let Ok(n) = n.parse::<usize>() else {
                eprintln!("bench: bad instance count");
                return 2;
            };
            synthetic(&mut renderer, n)
        }
        ["model", stream, index] => {
            let Some(dir) = game_dir else {
                eprintln!("bench: pass the game directory as the first argument");
                return 2;
            };
            match model(&mut renderer, dir, stream, index) {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("bench: {error}");
                    return 1;
                }
            }
        }
        _ => {
            eprintln!(
                "bench: expected instances:<n>, model:<stream>:<index> or level:<zone>[+<zone>…] \
                 (a zone, a folder of zones or preset:<id>)"
            );
            return 2;
        }
    };
    let overview = OrbitCamera::framing(&bounds);
    measure(&mut renderer, &scene, overview, "overview");
    if spec.starts_with("instances") {
        // Inside the grid near the ground, as in a level: most objects are culled.
        let mut street = overview;
        street.distance = 12.0;
        street.pitch = 0.15;
        street.scene_radius = 40.0;
        measure(&mut renderer, &scene, street, "street level");
    }
    0
}

/// A sphere of `rings × segments × 2` triangles.
fn sphere(rings: u32, segments: u32) -> MeshData {
    let mut vertices = Vec::new();
    for r in 0..=rings {
        let theta = r as f32 / rings as f32 * std::f32::consts::PI;
        for s in 0..=segments {
            let phi = s as f32 / segments as f32 * std::f32::consts::TAU;
            let n = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            vertices.push(Vertex {
                position: math::scale(n, 0.5),
                normal: n,
                uv: [s as f32 / segments as f32, r as f32 / rings as f32],
                color: [255; 4],
                ..Vertex::default()
            });
        }
    }
    let mut indices = Vec::new();
    for r in 0..rings {
        for s in 0..segments {
            let a = r * (segments + 1) + s;
            let b = a + segments + 1;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    let mut mesh = MeshData::default();
    let half = indices.len() / 2 / 3 * 3;
    mesh.push_part("a".into(), vertices.clone(), &indices[..half], tint(0));
    mesh.push_part("b".into(), vertices, &indices[half..], tint(1));
    mesh
}

fn tint(i: usize) -> MaterialData {
    MaterialData {
        texture: None,
        color: [
            [0.8, 0.5, 0.3, 1.0],
            [0.3, 0.6, 0.8, 1.0],
            [0.6, 0.8, 0.3, 1.0],
            [0.8, 0.8, 0.8, 1.0],
        ][i % 4],
        surface: None,
        sky: false,
    }
}

/// `n` instances of four meshes (576 to 4096 triangles each) on a grid.
fn synthetic(renderer: &mut SceneRenderer, n: usize) -> (Scene, math::Aabb) {
    for (key, detail) in [(1u64, 16u32), (2, 24), (3, 12), (4, 32)] {
        renderer.upload_mesh(key, Arc::new(sphere(detail, detail * 2)));
    }
    let side = (n as f32).sqrt().ceil() as usize;
    let mut scene = Scene::default();
    let mut bounds = math::Aabb::EMPTY;
    for i in 0..n {
        let (x, z) = ((i % side) as f32 * 2.0, (i / side) as f32 * 2.0);
        let transform = math::mul(
            &math::translation([x, 0.0, z]),
            &math::mul(
                &math::rotation_y(i as f32 * 0.37),
                &math::uniform_scale(0.6 + (i % 7) as f32 * 0.1),
            ),
        );
        let unit = math::Aabb {
            min: [-0.5; 3],
            max: [0.5; 3],
        };
        bounds.union(&unit.transformed(&transform));
        scene
            .instances
            .push(Instance::new(1 + (i % 4) as u64, transform));
    }
    let triangles: usize = scene
        .instances
        .iter()
        .map(|i| renderer.mesh_data(i.mesh).map_or(0, |m| m.triangle_count()))
        .sum();
    println!("scene: {n} instances, {triangles} triangles");
    (scene, bounds)
}

fn model(
    renderer: &mut SceneRenderer,
    dir: &Path,
    stream: &str,
    index: &str,
) -> Result<(Scene, math::Aabb), String> {
    let game = Arc::new(doctor_toolkit::game::GameInstance::open(dir).map_err(|e| e.to_string())?);
    let index: usize = index.parse().map_err(|_| "bad resource index")?;
    let s = game
        .find_stream(stream)
        .ok_or_else(|| format!("stream {stream} not found"))?;
    let file = game.open_stream(s).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let (resources, _) = game.build_index(|_| {});
    let mut assets = doctor_toolkit::game::GameAssets::new(game.clone(), &resources);
    // The first material lookup reads every material of the game; keep it out of the load time.
    use doctor_toolkit::export::gltf::AssetSource;
    assets.material(0);
    let indexed = started.elapsed();
    let started = Instant::now();
    let model = crate::model_view::load_model(&file, index, &mut assets, renderer)
        .map_err(|e| e.to_string())?;
    println!(
        "model: {} — {} parts, {} triangles, {} vertices, {} textured materials; \
         index and material map {:.0} ms, load and upload {:.0} ms, {:.1} MB of textures",
        model.name,
        model.parts.len(),
        model.mesh.triangle_count(),
        model.mesh.vertices.len(),
        model.textured_materials,
        indexed.as_secs_f64() * 1000.0,
        started.elapsed().as_secs_f64() * 1000.0,
        renderer.texture_bytes() as f64 / 1e6,
    );
    Ok((Scene::single(model.key), model.mesh.bounds()))
}

fn measure(renderer: &mut SceneRenderer, scene: &Scene, mut camera: OrbitCamera, title: &str) {
    println!("-- {title}");
    let mut viewport = Viewport::new();
    let aspect = SIZE[0] as f32 / SIZE[1] as f32;
    // Warm-up: pipelines, targets, first uploads.
    for _ in 0..3 {
        renderer.render(&mut viewport, SIZE, scene, &camera.view(aspect));
    }
    renderer.wait();
    let frames = frames();
    let mut times = Vec::with_capacity(frames);
    let mut cpu = Vec::with_capacity(frames);
    for frame in 0..frames {
        camera.yaw = frame as f32 * 0.02;
        let started = Instant::now();
        renderer.render(&mut viewport, SIZE, scene, &camera.view(aspect));
        renderer.wait();
        times.push(started.elapsed());
        cpu.push(renderer.stats.cpu_time);
    }
    let stats = renderer.stats;
    if let Ok(path) = std::env::var("DOCTOR_TOOLKIT_GUI_BENCH_SHOT")
        && let path = path.replace(".png", &format!("_{}.png", title.replace(' ', "_")))
        && let Some((w, h, rgba)) = renderer.read_pixels(&viewport)
    {
        let png = doctor_toolkit::resource::pixels::encode_png(w as usize, h as usize, &rgba);
        if let Err(error) = png.map(|png| std::fs::write(&path, png)) {
            eprintln!("bench: cannot save {path}: {error}");
        }
    }
    // Both picking paths on the same pixels: the GPU id pass and CPU rays.
    let view = camera.view(aspect);
    let meshes: HashMap<_, _> = scene
        .instances
        .iter()
        .filter_map(|i| Some((i.mesh, renderer.mesh_data(i.mesh)?.clone())))
        .collect();
    let meshes: HashMap<_, &MeshData> = meshes.iter().map(|(k, m)| (*k, m.as_ref())).collect();
    let mut picks = Vec::new();
    let mut rays = Vec::new();
    let (mut hits, mut agree) = (0, 0);
    for i in 0..20u32 {
        let pixel = [SIZE[0] / 2 + i * 37 % 400, SIZE[1] / 2 + i * 53 % 300];
        let started = Instant::now();
        let gpu = renderer.pick(&mut viewport, scene, &view, pixel);
        picks.push(started.elapsed());
        let started = Instant::now();
        let ray = view.ray([
            (pixel[0] as f32 + 0.5) / SIZE[0] as f32,
            (pixel[1] as f32 + 0.5) / SIZE[1] as f32,
        ]);
        let cpu = scene.ray_pick(&ray, &meshes);
        rays.push(started.elapsed());
        hits += gpu.is_some() as usize;
        agree += (gpu.map(|p| p.instance) == cpu.map(|p| p.instance)) as usize;
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let summary = |mut v: Vec<Duration>| {
        v.sort();
        format!(
            "median {:.2} ms, p99 {:.2} ms, max {:.2} ms",
            ms(v[v.len() / 2]),
            ms(v[v.len() * 99 / 100]),
            ms(*v.last().unwrap())
        )
    };
    println!(
        "{frames} frames at {}×{}, {}×MSAA: {} (CPU record+submit {})",
        SIZE[0],
        SIZE[1],
        renderer.sample_count(),
        summary(times),
        summary(cpu)
    );
    println!(
        "last frame: {} instances drawn, {} culled, {} draw calls, {} triangles",
        stats.instances, stats.culled, stats.draw_calls, stats.triangles
    );
    println!("pick, GPU id pass + readback: {}", summary(picks));
    println!(
        "pick, CPU ray over every triangle: {}; {hits} of 20 pixels hit, {agree} agree",
        summary(rays)
    );
}

/// Loads zones like the level view (worker threads, batches uploaded as they
/// come) and measures frames from above the level and at street level.
fn level(renderer: &mut SceneRenderer, dir: &Path, zones: &str) -> Result<(), String> {
    use crate::level_view::camera::FlyCamera;
    use crate::level_view::loader::{self, LoadInput, Msg};
    use crate::level_view::scene::{self, Filter, ItemKind, MARKER_KEY};
    use doctor_toolkit::level::{Catalog, Lookup, Overrides, Presets, presets};

    let started = Instant::now();
    let game = Arc::new(doctor_toolkit::game::GameInstance::open(dir).map_err(|e| e.to_string())?);
    let (index, _) = game.build_index(|_| {});
    let index = Arc::new(index);
    let catalog = Catalog::new(&game, &index);
    let found = presets::resolve(
        zones.split('+'),
        &catalog,
        &Presets::of_game(&game, &catalog),
        &[],
    );
    if let Some(word) = found.unknown.first() {
        return Err(format!("no zone at {word}"));
    }
    let megabytes = catalog
        .zones
        .iter()
        .filter(|z| found.streams.contains(&z.stream))
        .map(|z| z.bytes)
        .sum::<u64>() as f64
        / 1e6;
    let streams: Vec<usize> = found.streams.into_iter().collect();
    let input = LoadInput {
        lookup: Arc::new(Lookup::new(&index)),
        parents: Arc::new(game.stream_parents().map_err(|e| e.to_string())?),
        game: game.clone(),
        index,
        overrides: Overrides::default(),
        texture_limit: 512,
    };
    let indexed = started.elapsed();
    let started = Instant::now();
    let job = loader::start(input, streams.clone(), egui::Context::default());
    let mut level = None;
    let mut read = Duration::ZERO;
    let mut upload = Duration::ZERO;
    loop {
        match job.rx.recv() {
            Ok(Msg::Level {
                level: l, elapsed, ..
            }) => {
                read = elapsed;
                renderer.set_mesh_budget(l.meshes.len() + 256);
                renderer.upload_mesh(MARKER_KEY, Arc::new(scene::marker_mesh()));
                level = Some(l);
            }
            Ok(Msg::Batch(batch)) => {
                let t = Instant::now();
                for (key, texture) in &batch.textures {
                    renderer.upload_texture(*key, texture);
                }
                for (_, key, mesh) in batch.meshes {
                    renderer.upload_mesh(key, mesh);
                }
                upload += t.elapsed();
            }
            Ok(Msg::Done {
                meshes,
                textures,
                triangles,
                ..
            }) => {
                println!(
                    "level {zones}: {} zone(s), {megabytes:.0} MB of streams; index {:.0} ms, \
                     level read {:.0} ms, \
                     meshes and textures {:.1} s (GPU upload {:.0} ms): {meshes} meshes, \
                     {} k triangles, {textures} textures, {:.0} MB of textures, {:.0} MB of mesh buffers",
                    streams.len(),
                    indexed.as_secs_f64() * 1000.0,
                    read.as_secs_f64() * 1000.0,
                    started.elapsed().as_secs_f64(),
                    upload.as_secs_f64() * 1000.0,
                    triangles / 1000,
                    renderer.texture_bytes() as f64 / 1e6,
                    renderer.mesh_bytes() as f64 / 1e6,
                );
                break;
            }
            Ok(Msg::Error(error)) => return Err(error),
            Err(_) => return Err("the loader stopped".into()),
        }
    }
    let level = level.ok_or("no level")?;
    let c = level.coverage();
    println!(
        "objects {}, placed {}, with a model {}, drawn {}; {} mesh instances",
        c.objects, c.placed, c.with_model, c.rendered, c.instances
    );
    let (items, _) = scene::build_items(&level);
    let bounds = scene::bounds(&items, &|key| renderer.mesh_data(key).map(|m| m.bounds()));
    let mut overview = FlyCamera::default();
    overview.frame(&bounds);
    // Street level: two metres above the object nearest the centre.
    let centre = bounds.center();
    let nearest = items
        .iter()
        .filter(|i| i.kind == ItemKind::Mesh)
        .min_by(|a, b| {
            let d = |i: &scene::Item| {
                math::length(math::sub([i.world[12], i.world[13], i.world[14]], centre))
            };
            d(a).total_cmp(&d(b))
        })
        .map_or(centre, |i| [i.world[12], i.world[13], i.world[14]]);
    let mut street = FlyCamera {
        eye: math::add(nearest, [0.0, 2.0, 0.0]),
        pitch: -0.05,
        ..FlyCamera::default()
    };
    street.far = overview.far;
    // `DOCTOR_TOOLKIT_GUI_BENCH_LOOK=game[:hours]`: shade as in the game, at
    // noon by default (the frames are otherwise drawn with the simple look).
    let look = std::env::var("DOCTOR_TOOLKIT_GUI_BENCH_LOOK")
        .ok()
        .and_then(|spec| {
            let mut words = spec.split(':');
            (words.next() == Some("game")).then_some(())?;
            let hours = words.next().and_then(|h| h.parse().ok()).unwrap_or(12.0);
            let stream = streams
                .iter()
                .map(|&s| game.streams()[s].stream_name())
                .find(|n| doctor_toolkit::time_of_day::city_of(n).is_some())?;
            crate::game_light::look(&game, &stream, hours)
        });
    if look.is_some() {
        println!(
            "shading as in the game{}",
            if renderer.has_game_path() {
                " (binding arrays: shadows and lamps)"
            } else {
                ""
            }
        );
    }
    let lights = scene::build_lights(&level);
    for (title, camera, filter) in [
        ("overview, LOD by distance", overview, Filter::default()),
        ("street level, LOD by distance", street, Filter::default()),
        (
            "overview, finest LOD",
            overview,
            Filter {
                lod: scene::LodMode::Finest,
                ..Filter::default()
            },
        ),
    ] {
        measure_level(
            renderer,
            &items,
            camera,
            &filter,
            bounds.center(),
            title,
            (look, &lights),
        );
    }
    Ok(())
}

fn measure_level(
    renderer: &mut SceneRenderer,
    items: &[crate::level_view::scene::Item],
    mut camera: crate::level_view::camera::FlyCamera,
    filter: &crate::level_view::scene::Filter,
    pivot: math::Vec3,
    title: &str,
    (look, lights): (
        Option<crate::render::GameLook>,
        &[crate::level_view::scene::WorldLight],
    ),
) {
    use crate::level_view::scene;
    println!("-- {title}");
    let mut viewport = Viewport::new();
    let aspect = SIZE[0] as f32 / SIZE[1] as f32;
    let mut scene = Scene::default();
    scene.options.game = look;
    // `DOCTOR_TOOLKIT_GUI_BENCH_SWITCHES=noshadows,nolamps`: the game look
    // without the sun's shadows or the lights of lamps.
    let switches = std::env::var("DOCTOR_TOOLKIT_GUI_BENCH_SWITCHES").unwrap_or_default();
    scene.options.shadows = !switches.contains("noshadows");
    scene.options.lamps = !switches.contains("nolamps");
    if let Some(look) = &look
        && scene.options.lamps
    {
        scene.lights = scene::scene_lights(lights, look.night);
    }
    let street = title.starts_with("street");
    let distance = if street {
        200.0
    } else {
        math::length(math::sub(pivot, camera.eye))
    };
    let mut filter = filter.clone();
    filter.min_angle = 2.0 * camera.fov_y / SIZE[1] as f32;
    let filter = &filter;
    let spheres: HashMap<u64, scene::MeshSphere> = items
        .iter()
        .filter_map(|i| {
            let b = renderer.mesh_data(i.mesh)?.bounds();
            Some((i.mesh, (b.center(), b.radius())))
        })
        .collect();
    let sphere = |key| spheres.get(&key).copied();
    for _ in 0..3 {
        scene::fill(&mut scene, items, camera.eye, filter, &sphere);
        renderer.render(&mut viewport, SIZE, &scene, &camera.view(aspect));
    }
    renderer.wait();
    let frames = frames();
    let (mut times, mut fills, mut cpu, mut gpu) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for frame in 0..frames {
        camera.yaw += 0.02;
        if !street {
            camera.look_at(pivot, distance);
        }
        let started = Instant::now();
        scene::fill(&mut scene, items, camera.eye, filter, &sphere);
        fills.push(started.elapsed());
        renderer.render(&mut viewport, SIZE, &scene, &camera.view(aspect));
        renderer.wait();
        times.push(started.elapsed());
        cpu.push(renderer.stats.cpu_time);
        // The pass time arrives with the next frame.
        if frame > 0 {
            gpu.extend(renderer.stats.gpu_time);
        }
    }
    renderer.render(&mut viewport, SIZE, &scene, &camera.view(aspect));
    renderer.wait();
    renderer.render(&mut viewport, SIZE, &scene, &camera.view(aspect));
    gpu.extend(renderer.stats.gpu_time);
    let stats = renderer.stats;
    if let Ok(path) = std::env::var("DOCTOR_TOOLKIT_GUI_BENCH_SHOT")
        && let path = path.replace(".png", &format!("_{}.png", title.replace([' ', ','], "_")))
        && let Some((w, h, rgba)) = renderer.read_pixels(&viewport)
    {
        let png = doctor_toolkit::resource::pixels::encode_png(w as usize, h as usize, &rgba);
        if let Err(error) = png.map(|png| std::fs::write(&path, png)) {
            eprintln!("bench: cannot save {path}: {error}");
        }
    }
    let view = camera.view(aspect);
    let meshes: HashMap<_, _> = scene
        .instances
        .iter()
        .filter_map(|i| Some((i.mesh, renderer.mesh_data(i.mesh)?.clone())))
        .collect();
    let meshes: HashMap<_, &MeshData> = meshes.iter().map(|(k, m)| (*k, m.as_ref())).collect();
    let (mut picks, mut rays) = (Vec::new(), Vec::new());
    for i in 0..10u32 {
        let pixel = [SIZE[0] / 2 + i * 37 % 400, SIZE[1] / 2 + i * 53 % 300];
        let started = Instant::now();
        renderer.pick(&mut viewport, &scene, &view, pixel);
        picks.push(started.elapsed());
        let started = Instant::now();
        let ray = view.ray([
            (pixel[0] as f32 + 0.5) / SIZE[0] as f32,
            (pixel[1] as f32 + 0.5) / SIZE[1] as f32,
        ]);
        scene.ray_pick(&ray, &meshes);
        rays.push(started.elapsed());
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let summary = |mut v: Vec<Duration>| {
        v.sort();
        format!(
            "median {:.2} ms, p99 {:.2} ms",
            ms(v[v.len() / 2]),
            ms(v[v.len() * 99 / 100])
        )
    };
    println!(
        "{frames} frames at {}×{}: {} (instance selection {}); {} drawn, {} culled, {} draw calls of {} parts, {} k triangles",
        SIZE[0],
        SIZE[1],
        summary(times),
        summary(fills),
        stats.instances,
        stats.culled,
        stats.draw_calls,
        stats.parts,
        stats.triangles / 1000
    );
    println!(
        "render CPU (prepare, record, submit) {}; GPU scene pass {}",
        summary(cpu),
        if gpu.is_empty() {
            "not timed".to_owned()
        } else {
            summary(gpu)
        }
    );
    println!(
        "pick: GPU id pass {}; CPU ray {}",
        summary(picks),
        summary(rays)
    );
}
