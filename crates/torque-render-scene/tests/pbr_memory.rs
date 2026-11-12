//! What building a physically based material allocates: every byte under its working memory
//! (`torque_render_scene::hd::pbr::working_bytes`), the bound the loader checks before a map is decoded. Its own test binary:
//! the counting allocator is the process's.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use torque_render_scene::hd::TextureTable;
use torque_render_scene::hd::pbr::{self, MapImage, MapImages, PbrMaterial, working_bytes};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn map(w: u32, h: u32, seed: u8) -> MapImage {
    MapImage {
        rgba: (0..w * h * 4)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect(),
        width: w,
        height: h,
    }
}

#[test]
fn a_materials_build_allocates_no_more_than_its_working_bytes() {
    // Every kind of map, separate occlusion, roughness and metalness to pack, uneven sizes.
    for (w, h) in [(512u32, 512u32), (640, 360), (1024, 256)] {
        let maps = MapImages {
            base: Some(map(w, h, 1)),
            normal: Some(map(w, h, 2)),
            occlusion: Some(map(w, h, 3)),
            roughness: Some(map(w, h, 4)),
            metal: Some(map(w, h, 5)),
            height: Some(map(w, h, 6)),
            emissive: Some(map(w, h, 7)),
            orm: None,
        };
        let bound = working_bytes([(w, h); 7]);
        let mut textures = TextureTable::default();
        let region = Region::new(GLOBAL);
        let built = pbr::build(&PbrMaterial::default(), "m", maps, &mut textures);
        let allocated = region.change().bytes_allocated as u64;
        assert!(built.is_ok());
        println!("{w} x {h}: {allocated} bytes allocated, {bound} bound");
        assert!(allocated <= bound, "{w} x {h}: {allocated} over {bound}");
    }
}
