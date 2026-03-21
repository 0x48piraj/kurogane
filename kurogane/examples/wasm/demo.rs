#![no_std]

use core::panic::PanicInfo;

#[link(wasm_import_module = "env")]
extern "C" {
    fn clear();
    fn draw_rect(x: f32, y: f32, size: f32, r: u8, g: u8, b: u8);
    fn width() -> f32;
    fn height() -> f32;
}

const BOX_COUNT: usize = 1000;
const SIZE: f32 = 16.0;

// Simulation state
static mut X: [f32; BOX_COUNT] = [0.0; BOX_COUNT];
static mut Y: [f32; BOX_COUNT] = [0.0; BOX_COUNT];
static mut VX: [f32; BOX_COUNT] = [0.0; BOX_COUNT];
static mut VY: [f32; BOX_COUNT] = [0.0; BOX_COUNT];
static mut COLOR: [(u8, u8, u8); BOX_COUNT] = [(0, 0, 0); BOX_COUNT];

// Simple deterministic RNG
static mut SEED: u32 = 1;

fn rand() -> f32 {
    unsafe {
        SEED = SEED.wrapping_mul(1664525).wrapping_add(1013904223);
        (SEED as f32) / (u32::MAX as f32)
    }
}

#[no_mangle]
pub extern "C" fn init() {
    unsafe {
        let w = width();
        let h = height();

        for i in 0..BOX_COUNT {
            X[i] = rand() * w;
            Y[i] = rand() * h;
            VX[i] = rand() * 4.0 - 2.0;
            VY[i] = rand() * 4.0 - 2.0;

            // HSL-ish random colors
            let r = (rand() * 255.0) as u8;
            let g = (rand() * 255.0) as u8;
            let b = (rand() * 255.0) as u8;
            COLOR[i] = (r, g, b);
        }
    }
}

#[no_mangle]
pub extern "C" fn tick() {
    unsafe {
        let w = width();
        let h = height();

        clear();

        for i in 0..BOX_COUNT {
            X[i] += VX[i];
            Y[i] += VY[i];

            // bounce off edges
            if X[i] < 0.0 || X[i] > w {
                VX[i] = -VX[i];
            }
            if Y[i] < 0.0 || Y[i] > h {
                VY[i] = -VY[i];
            }

            let (r, g, b) = COLOR[i];
            draw_rect(X[i], Y[i], SIZE, r, g, b);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
