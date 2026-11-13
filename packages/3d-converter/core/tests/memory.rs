//! One-shot `convert` writes the scene it read, in place, as litegen's HTTP
//! converter does: `units` and `up` cost it no copy of the scene, where a
//! read-once `Model`, which keeps its scene for the next write, copies it.
//! Measured as bytes allocated (not peak memory, which the parse sets), by a
//! counting allocator this test binary installs.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use litegen_3d_converter::{convert, Format, Input, Model, ReadOptions, Unit, UpAxis, WriteOptions};

struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Bytes allocated while `f` runs.
fn allocated(f: impl FnOnce()) -> usize {
    let before = ALLOCATED.load(Ordering::Relaxed);
    f();
    ALLOCATED.load(Ordering::Relaxed) - before
}

const N: usize = 120;

/// The grid's geometry as the scene holds it: f32 positions and u32 indices.
const GEOMETRY_BYTES: usize = (N + 1) * (N + 1) * 12 + 2 * N * N * 3 * 4;

/// A 2 x 120 x 120 triangle OBJ grid.
fn grid() -> Vec<u8> {
    let n = N;
    let mut s = String::new();
    for y in 0..=n {
        for x in 0..=n {
            s.push_str(&format!("v {} {} {}\n", x as f32 / n as f32, ((x * y) % 7) as f32 / 100.0, y as f32 / n as f32));
        }
    }
    for y in 0..n {
        for x in 0..n {
            let a = y * (n + 1) + x + 1;
            s.push_str(&format!("f {a} {} {}\nf {} {} {}\n", a + n + 1, a + 1, a + 1, a + n + 1, a + n + 2));
        }
    }
    s.into_bytes()
}

#[test]
fn units_and_up_cost_a_one_shot_convert_no_copy_of_the_scene() {
    let obj = grid();
    let plain = WriteOptions::new(Format::Stl);
    let framed = WriteOptions { units: Some(Unit::Mm), up: Some(UpAxis::Z), ..WriteOptions::new(Format::Stl) };
    let read = ReadOptions::default();

    // The control: a Model keeps its scene, so a framed write copies it.
    let model = Model::read(Input::Bytes(&obj), &read).unwrap();
    model.write(&plain).unwrap(); // the first write transcodes; measure the next
    let model_plain = allocated(|| drop(model.write(&plain).unwrap()));
    let model_framed = allocated(|| drop(model.write(&framed).unwrap()));
    let copy = model_framed - model_plain;
    assert!(copy >= GEOMETRY_BYTES, "the measure sees the Model's copy of the scene: {copy} bytes, geometry {GEOMETRY_BYTES}");

    let convert_plain = allocated(|| drop(convert(Input::Bytes(&obj), &read, &plain).unwrap()));
    let convert_framed = allocated(|| drop(convert(Input::Bytes(&obj), &read, &framed).unwrap()));
    let extra = convert_framed.saturating_sub(convert_plain);
    assert!(extra < copy / 4, "convert with units and up allocated {extra} bytes more than without; a scene copy is {copy}");
}
