//! Windowなしでsimulationを一定tick数進める最小runner。

use std::time::Instant;

use charon_engine::simulation::world::World;

fn main() {
    let ticks = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(600);
    let mut world = match World::new(96, 64) {
        Ok(world) => world,
        Err(error) => {
            eprintln!("headless world creation failed: {error}");
            std::process::exit(1);
        }
    };
    let started = Instant::now();

    for _ in 0..ticks {
        if let Err(error) = world.update() {
            eprintln!("headless simulation stopped: {error}");
            std::process::exit(1);
        }
    }

    println!(
        "headless simulation complete: ticks={}, cells={}, fingerprint={:016x}, elapsed_ms={}",
        world.tick_count,
        world.cells.len(),
        world.state_fingerprint(),
        started.elapsed().as_millis()
    );
}
