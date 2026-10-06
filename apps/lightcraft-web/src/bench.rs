//! Scripted in-app measurement (`?bench`): open the first photo in Detail, then drive the Exposure
//! control exactly like a slider drag (begin interaction → N `develop.set` → end) and time each
//! step from the command to the moment the new loupe texture is ready. Results go to the log
//! (the browser console on the web) as one `lightcraft-bench {…json…}` line.

use lightcraft_ui_egui::LightcraftApp;
use lightcraft_ui_egui::render::Slot;
use serde_json::{Value, json};

const STEPS: usize = 8;

#[derive(Debug)]
enum Phase {
    /// Wait for the grid's thumbnails to finish.
    Idle,
    /// Detail view requested; wait for the loupe.
    Opening {
        t0: f64,
    },
    /// A step was issued at `t0`; wait for a Main texture with a new key.
    Step {
        i: usize,
        t0: f64,
        key: u64,
    },
    Done,
}

pub struct Bench {
    phase: Phase,
    frames: u32,
    start_ms: f64,
    first_frame_ms: Option<f64>,
    thumbs_done_ms: Option<f64>,
    open_ms: f64,
    drafts: Vec<f64>,
    jobs: Vec<f64>,
    final_ms: f64,
}

impl Bench {
    /// `start_ms`: the page's time origin in the same clock as [`lightcraft_ui_egui::now_ms`].
    pub fn new(start_ms: f64) -> Self {
        Bench {
            phase: Phase::Idle,
            frames: 0,
            start_ms,
            first_frame_ms: None,
            thumbs_done_ms: None,
            open_ms: 0.0,
            drafts: vec![],
            jobs: vec![],
            final_ms: 0.0,
        }
    }

    fn main_key(app: &LightcraftApp) -> Option<u64> {
        app.renderer.textures.get(&Slot::Main).map(|t| t.key)
    }

    /// Call once per frame after `LightcraftApp::logic`. Returns the report when finished.
    pub fn step(&mut self, app: &mut LightcraftApp) -> Option<Value> {
        let now = lightcraft_ui_egui::now_ms();
        self.frames += 1;
        if self.first_frame_ms.is_none() && self.frames > 1 {
            self.first_frame_ms = Some(now - self.start_ms);
        }
        let idle = app.renderer.queued() == 0 && app.renderer.in_flight() == 0;
        match self.phase {
            Phase::Idle => {
                let thumbs = app.renderer.textures.keys().filter(|s| matches!(s, Slot::Thumb(_) | Slot::ThumbQuick(_))).count();
                if idle && thumbs > 0 && self.frames > 3 {
                    self.thumbs_done_ms = Some(now - self.start_ms);
                    let first = app.session.visible().first().copied();
                    if let Some(id) = first {
                        let _ = app.run("library.select", json!({"ids": [id.0]}));
                    }
                    let _ = app.run("view.detail", json!({}));
                    self.phase = Phase::Opening { t0: now };
                }
            }
            Phase::Opening { t0 } => {
                if idle && Self::main_key(app).is_some() && self.frames > 3 {
                    self.open_ms = now - t0;
                    let _ = app.run("develop.beginInteraction", json!({"label": "Exposure"}));
                    self.issue(app, 0, now);
                }
            }
            Phase::Step { i, t0, key } => {
                if idle && Self::main_key(app).is_some_and(|k| k != key) {
                    let dt = now - t0;
                    if i < STEPS {
                        self.drafts.push(dt);
                        self.jobs.push(app.renderer.last_main_ms);
                        if i + 1 < STEPS {
                            self.issue(app, i + 1, now);
                        } else {
                            // release: the full-resolution render after the drag
                            let key = Self::main_key(app).unwrap_or(0);
                            let _ = app.run("develop.endInteraction", json!({}));
                            let _ = app.run("develop.set", json!({"control": "light.exposure", "value": 0.0}));
                            self.phase = Phase::Step { i: STEPS, t0: now, key };
                        }
                    } else {
                        self.final_ms = dt;
                        self.phase = Phase::Done;
                        return Some(self.report(app));
                    }
                }
            }
            Phase::Done => {}
        }
        None
    }

    fn issue(&mut self, app: &mut LightcraftApp, i: usize, now: f64) {
        let key = Self::main_key(app).unwrap_or(0);
        let v = 0.15 * (i as f64 + 1.0);
        let _ = app.run("develop.set", json!({"control": "light.exposure", "value": v}));
        self.phase = Phase::Step { i, t0: now, key };
    }

    fn report(&self, app: &LightcraftApp) -> Value {
        let mut d = self.drafts.clone();
        d.sort_by(f64::total_cmp);
        let median = d.get(d.len() / 2).copied().unwrap_or(0.0);
        let mean = if d.is_empty() { 0.0 } else { d.iter().sum::<f64>() / d.len() as f64 };
        let tex = app.renderer.textures.get(&Slot::Main).map(|t| t.size).unwrap_or([0, 0]);
        let r = |x: f64| (x * 10.0).round() / 10.0;
        json!({
            "first_frame_ms": self.first_frame_ms.map(r),
            "thumbs_done_ms": self.thumbs_done_ms.map(r),
            "open_detail_ms": r(self.open_ms),
            "slider_draft_ms": d.iter().copied().map(r).collect::<Vec<_>>(),
            "slider_draft_median_ms": r(median),
            "slider_draft_mean_ms": r(mean),
            "slider_job_ms": self.jobs.iter().copied().map(r).collect::<Vec<_>>(),
            "release_full_ms": r(self.final_ms),
            "loupe_px": tex,
            "frames": self.frames,
        })
    }
}
