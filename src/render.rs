//! 正規simulation stateをRGBAフレームへ描画するrenderer側処理。

use crate::entities::player::Player;
use crate::simulation::material::AIR;
use crate::simulation::world::World;

/// ワールド、視覚粒子、プレイヤーをRGBAフレームへ描画します。
pub fn render_to_frame(world: &World, player: &Player, frame: &mut [u8]) {
    let width = world.width;

    for (i, pixel) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let Some(cell) = world.cells.get(i).copied() else {
            break;
        };
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        let is_surface = y == 0 || world.cells[((y - 1) * width + x) as usize].material == AIR;
        let rgba = world.registry.get_adaptive_color(
            cell.material,
            x,
            y,
            is_surface,
            cell.shade(),
            cell.is_falling(),
        );
        pixel.copy_from_slice(&rgba);
    }

    for particle in &world.particles {
        let px = particle.x.round() as i32;
        let py = particle.y.round() as i32;
        if px < 0 || px >= world.width as i32 || py < 0 || py >= world.height as i32 {
            continue;
        }
        let idx = ((py as u32 * world.width + px as u32) * 4) as usize;
        if idx + 3 >= frame.len() {
            continue;
        }
        let alpha = particle.color[3] as f32 / 255.0;
        let inv_alpha = 1.0 - alpha;
        frame[idx] = ((particle.color[0] as f32 * alpha) + (frame[idx] as f32 * inv_alpha)) as u8;
        frame[idx + 1] =
            ((particle.color[1] as f32 * alpha) + (frame[idx + 1] as f32 * inv_alpha)) as u8;
        frame[idx + 2] =
            ((particle.color[2] as f32 * alpha) + (frame[idx + 2] as f32 * inv_alpha)) as u8;
    }

    let p_x = player.x.to_f64().round() as i32;
    let p_y = player.y.to_f64().round() as i32;
    let p_w = player.width.to_f64().round() as i32;
    let p_h = player.height.to_f64().round() as i32;
    for dy in 0..p_h {
        for dx in 0..p_w {
            let x = p_x + dx;
            let y = p_y + dy;
            if x < 0 || x >= world.width as i32 || y < 0 || y >= world.height as i32 {
                continue;
            }
            let idx = ((y as u32 * world.width + x as u32) * 4) as usize;
            if idx + 3 >= frame.len() {
                continue;
            }
            let is_border = dx == 0 || dx == p_w - 1 || dy == 0 || dy == p_h - 1;
            let color = if is_border {
                [230, 40, 40, 255]
            } else {
                [255, 220, 80, 255]
            };
            frame[idx..idx + 4].copy_from_slice(&color);
        }
    }
}
