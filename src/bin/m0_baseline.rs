//! M0で現状を固定する小規模な決定的sceneを実行します。大規模性能や製品性能の主張には使いません。

use std::time::Instant;

use charon_engine::simulation::command::{CommandError, WorldCommand};
use charon_engine::simulation::material::{AIR, SAND, STONE, WATER};
use charon_engine::simulation::world::World;

const EDGE: u32 = 64;

fn world() -> World {
    World::new(EDGE, EDGE).expect("64x64 fixture world")
}

fn paint(world: &mut World, x: i32, y: i32, radius: i32, material: u16) {
    world
        .apply_command(WorldCommand::PaintBrush {
            center_x: x,
            center_y: y,
            radius,
            material,
        })
        .expect("valid fixed fixture command");
}

fn prepare(name: &str) -> World {
    let mut world = world();
    match name {
        "b0" => {}
        "b1" => {
            // 水と砂を複数箇所へ配置し、液体の流動と粉体の落下を同じ条件で観察します。
            for row in 0..8 {
                for column in 0..8 {
                    let x = 4 + column * 7;
                    let y = 2 + row * 7;
                    let material = if (row + column) % 2 == 0 { WATER } else { SAND };
                    paint(&mut world, x, y, 1, material);
                }
            }
        }
        "b2" => {
            paint(&mut world, 32, 48, 12, STONE);
            for x in 0..EDGE as i32 {
                paint(&mut world, x, 62, 0, STONE);
            }
        }
        "b3" => {
            // 全cellの2割に熱energyを与えます。反応生成物や燃料timerの挙動はこのsceneの対象外です。
            for index in 0..(EDGE * EDGE / 5) {
                let x = index % EDGE;
                let y = index / EDGE;
                world.set_thermal_energy(x, y, 10_000);
            }
        }
        _ => unreachable!("validated scenario name"),
    }
    world
}

fn main() {
    let mut args = std::env::args().skip(1);
    let scenario = args.next().unwrap_or_else(|| "all".to_owned());
    let ticks = args
        .next()
        .and_then(|arg| arg.parse::<u64>().ok())
        .unwrap_or(120);
    let scenes: &[&str] = match scenario.as_str() {
        "all" => &["b0", "b1", "b2", "b3"],
        "b0" | "b1" | "b2" | "b3" => &[scenario.as_str()],
        "failure" => {
            run_failure_scene();
            return;
        }
        _ => {
            eprintln!(
                "usage: cargo run --locked --bin m0_baseline -- [all|b0|b1|b2|b3|failure] [ticks]"
            );
            std::process::exit(2);
        }
    };

    for scene in scenes {
        let mut world = prepare(scene);
        let started = Instant::now();
        for tick in 1..=ticks {
            if *scene == "b2" && tick % 60 == 0 {
                paint(&mut world, 32, 48, 8, AIR);
            }
            world.update().expect("fixed-size fixture tick");
        }
        println!(
            "scene={scene} dimensions={EDGE}x{EDGE} ticks={} elapsed_ms={} fingerprint={:016x} active_chunks={} resident_chunks={}",
            world.tick_count,
            started.elapsed().as_millis(),
            world.state_fingerprint(),
            world.active_chunk_count(),
            world.resident_chunk_count(),
        );
    }
}

fn run_failure_scene() {
    let mut world = world();
    paint(&mut world, 10, 10, 0, WATER);
    let before = world.state_fingerprint();
    let result = world.apply_command(WorldCommand::PaintBrush {
        center_x: 10,
        center_y: 10,
        radius: 65,
        material: SAND,
    });
    assert_eq!(result, Err(CommandError::InvalidRadius));
    assert_eq!(world.state_fingerprint(), before);
    println!(
        "scene=failure_invalid_brush rejected=InvalidRadius state_unchanged=true fingerprint={before:016x}"
    );
}
