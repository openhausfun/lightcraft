//! Device-memory bookkeeping: a view's stages are counted, released buffers are pooled within
//! the pool limit, and trimming frees them. Skips when no GPU adapter exists.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};

#[test]
fn stages_are_counted_and_the_pool_is_bounded_and_trimmed() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    let limit = 64u64 << 20;
    lightcraft_gpu::set_pool_limit(limit);
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.effects.clarity = 30.0;
    let stages = StageCache::default();
    // photos of slightly different sizes, like stepping through a library
    for (i, (w, h)) in [(1200usize, 800usize), (1190, 810), (1210, 790), (800, 1200)].into_iter().enumerate() {
        let src = Arc::new(lightcraft_scenes::demo_library()[i].render(w, h));
        let r = lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(w, h), Some(&stages)).expect("gpu render");
        assert_eq!((r.image.width, r.image.height), (w, h));
        let m = lightcraft_gpu::memory();
        assert!(m.pooled <= limit, "pool {} over its limit", m.pooled);
        assert_eq!(m.retired, 0, "a finished render leaves nothing retired");
    }
    let held = lightcraft_gpu::stage_bytes(&stages);
    assert!(held >= 1200 * 800 * 3 * 4, "the view's stages hold at least its resampled source: {held}");
    let before = lightcraft_gpu::memory().allocated;
    stages.clear();
    let m = lightcraft_gpu::memory();
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0);
    assert!(m.pooled <= limit && m.allocated <= before);
    lightcraft_gpu::trim_pool(0);
    let m = lightcraft_gpu::memory();
    assert_eq!(m.pooled, 0);
    assert!(m.allocated < before, "trimming frees the pooled buffers: {} → {}", before, m.allocated);
}
