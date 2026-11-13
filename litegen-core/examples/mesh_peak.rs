//! Peak heap of the converter, for the memory figures in
//! `docs/superpowers/3d-format-conversion.md` and on `mesh::MAX_INPUT_BYTES`.
//!
//! ```text
//! cargo run --release --example mesh_peak -- <file>...
//! ```
//!
//! Each file is read as the container its extension names. For each it
//! prints, as multiples of the file's size, the peak heap:
//!
//! - of reading it;
//! - of converting it to each container on its own, as the convert endpoint
//!   does: the input, the scene and one output alive;
//! - of writing every container from one read and holding them all, as a
//!   delivery of all ten does.
//!
//! The input's own bytes are counted in every figure.

use litegen::mesh::{self, MeshFormat, NoSidecars, WriteOptions};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The system allocator, counting the bytes it has handed out.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let now = LIVE.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(now, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Starts a measurement: the peak so far becomes what is alive now.
fn start() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

/// The peak since `start` returned `base`, plus the input's `len` bytes.
fn peak(base: usize, len: usize) -> f64 {
    (PEAK.load(Relaxed) - base + len) as f64 / len as f64
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: mesh_peak <file>...");
        std::process::exit(2);
    }
    for path in files {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("");
        let from = MeshFormat::parse(ext).unwrap_or_else(|| panic!("{path}: no container is named {ext:?}"));
        let len = bytes.len();
        let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();

        let base = start();
        let read = mesh::read_scene(&bytes, from, &NoSidecars).unwrap_or_else(|e| panic!("{path}: {e}"));
        let reading = peak(base, len);
        drop(read);

        let mut each = Vec::new();
        for to in MeshFormat::ALL {
            let base = start();
            let written = mesh::convert_scene(&bytes, from, &NoSidecars, to, WriteOptions::default());
            let p = peak(base, len);
            drop(written);
            each.push((to, p));
        }
        let (worst, worst_x) = each.iter().copied().fold((from, 0.0), |a, b| if b.1 > a.1 { b } else { a });

        let base = start();
        let scene = mesh::read_scene(&bytes, from, &NoSidecars).unwrap().scene;
        let held: Vec<_> = MeshFormat::ALL.iter().filter_map(|to| mesh::write_scene(&scene, *to, WriteOptions::default()).ok()).collect();
        let all = peak(base, len);
        drop(held);
        drop(scene);

        println!(
            "{name}: {:.1} MB; read {reading:.1}x; one conversion up to {worst_x:.1}x (to {worst:?}); all ten held {all:.1}x",
            len as f64 / 1e6
        );
        for (to, p) in each {
            println!("    to {to:?}: {p:.1}x");
        }
    }
}
