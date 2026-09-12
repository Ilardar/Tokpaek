//! Programmatic tray icon: two arcs forming a single circular boundary,
//! each arc having 12 cells colored in traffic-light colors (green, yellow, red).
//! Returns 32x32 RGBA8.

pub const SIZE: u32 = 32;

pub fn rgba() -> Vec<u8> {
    use std::f32::consts::PI;

    let w = SIZE as usize;
    let h = SIZE as usize;
    let mut buf = vec![0u8; w * h * 4];

    let cx = 16.0_f32;
    let cy = 16.0_f32;
    let r_out = 13.5_f32;
    let r_in = 8.5_f32;
    let split_half_rad = 6.0_f32.to_radians();
    let span = PI - 2.0 * split_half_rad;
    let n_cells = 12.0_f32;
    let cell_pitch = span / n_cells;
    let cell_gap = cell_pitch * 0.18;

    let c_bg = [18.0_f32, 20.0, 26.0, 255.0];
    let c_track = [34.0_f32, 38.0, 48.0, 255.0];
    let c_green = [16.0_f32, 196.0, 128.0, 255.0];
    let c_yellow = [250.0_f32, 185.0, 25.0, 255.0];
    let c_red = [244.0_f32, 63.0, 94.0, 255.0];

    let pad = 1.0_f32;
    let cx_tile = SIZE as f32 / 2.0;
    let r_tile = cx_tile - pad;

    let sample = |x: f32, y: f32| -> [f32; 4] {
        // Circular tile: the backing plate is a disc, not a squircle.
        let dx_t = x - cx_tile;
        let dy_t = y - cx_tile;
        if dx_t * dx_t + dy_t * dy_t > r_tile * r_tile {
            return [0.0, 0.0, 0.0, 0.0];
        }

        let dx = x - cx;
        let dy = y - cy;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < r_in || dist > r_out {
            return c_bg;
        }

        // Within annular ring:
        // Top arc (dy < 0): angle from 9 o'clock to 3 o'clock
        // Bottom arc (dy >= 0): angle from 9 o'clock to 3 o'clock
        let theta = if dy < 0.0 {
            (-dy).atan2(-dx)
        } else {
            dy.atan2(-dx)
        };

        if theta < split_half_rad || theta > PI - split_half_rad {
            return c_bg;
        }

        let arc_t = theta - split_half_rad;
        let cell_idx = (arc_t / cell_pitch).floor() as usize;
        if cell_idx >= 12 {
            return c_bg;
        }

        let pos_in_cell = arc_t - cell_idx as f32 * cell_pitch;
        if pos_in_cell > cell_pitch - cell_gap {
            return c_track;
        }

        if cell_idx < 4 {
            c_green
        } else if cell_idx < 8 {
            c_yellow
        } else {
            c_red
        }
    };

    // 4x4 supersampling for antialiased subpixels
    const SS: usize = 4;
    for py in 0..h {
        for px in 0..w {
            let mut acc = [0.0_f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let c = sample(x, y);
                    for k in 0..4 {
                        acc[k] += c[k];
                    }
                }
            }
            let idx = (py * w + px) * 4;
            let count = (SS * SS) as f32;
            buf[idx] = (acc[0] / count).round() as u8;
            buf[idx + 1] = (acc[1] / count).round() as u8;
            buf[idx + 2] = (acc[2] / count).round() as u8;
            buf[idx + 3] = (acc[3] / count).round() as u8;
        }
    }

    buf
}

