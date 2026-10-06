//! Base rendering profiles. A profile is a look applied *underneath* the user's sliders (the sliders
//! stay at their displayed values); `profile.amount` (0–200 %) scales it. All looks are our own.
//!
//! Every look is a set of *deltas* on the user's settings, each multiplied by the amount: slider
//! offsets (contrast, saturation, colour mixer, B&W mix…), colour-grading wheels (added as colour
//! vectors to the user's wheels) and point-curve fades (black/white output levels composed after
//! the user's point curves). The CPU pipeline and the GPU path both render [`effective`]
//! settings, so a profile needs no kernel of its own. No look adds grain.

use std::borrow::Cow;

use lightcraft_develop::{DevelopSettings, Treatment, Wheel};
use lightcraft_geom::Point;

/// Add a colour-grading wheel (`hue` °, `sat` 0..100, `lum` −100..100) scaled by `k` to `w`, as a
/// colour vector (so a profile's tint combines with the user's instead of replacing it).
fn wheel(w: &mut Wheel, hue: f64, sat: f64, lum: f64, k: f64) {
    let (ua, ub) = (w.sat * w.hue.to_radians().cos(), w.sat * w.hue.to_radians().sin());
    let (pa, pb) = (sat * k * hue.to_radians().cos(), sat * k * hue.to_radians().sin());
    let (a, b) = (ua + pa, ub + pb);
    let s = a.hypot(b);
    if s > 1e-9 {
        w.hue = b.atan2(a).to_degrees().rem_euclid(360.0);
    } else if w.sat == 0.0 {
        w.hue = hue;
    }
    w.sat = s.clamp(0.0, 100.0);
    w.lum = (w.lum + lum * k).clamp(-100.0, 100.0);
}

/// Fade a point curve: output black level `black` and white level `white` (0..1, moved from the
/// identity by `k`), composed after the user's curve (`y → b + y·(w − b)`).
fn fade(pts: &mut Vec<Point>, black: f64, white: f64, k: f64) {
    let b = (black * k).clamp(0.0, 0.45);
    let w = (1.0 - (1.0 - white) * k).clamp(0.55, 1.0);
    if pts.len() < 2 {
        *pts = vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
    }
    for p in pts.iter_mut() {
        p.y = b + p.y.clamp(0.0, 1.0) * (w - b);
    }
}

/// The settings the pipeline actually renders: user settings plus the profile's look.
pub fn effective(s: &DevelopSettings) -> Cow<'_, DevelopSettings> {
    let k = (s.profile.amount / 100.0).clamp(0.0, 2.0);
    let id = s.profile.id.as_str();
    if id == "lc.color" || id.is_empty() || k == 0.0 {
        return Cow::Borrowed(s);
    }
    let mut e = s.clone();
    let add = |v: &mut f64, d: f64| *v = (*v + d * k).clamp(-100.0, 100.0);
    match id {
        // ---- Basic
        "lc.neutral" => {
            add(&mut e.light.contrast, -18.0);
            add(&mut e.color.saturation, -10.0);
            add(&mut e.light.highlights, -10.0);
            add(&mut e.light.shadows, 8.0);
        }
        "lc.vivid" => {
            add(&mut e.light.contrast, 16.0);
            add(&mut e.color.saturation, 14.0);
            add(&mut e.color.vibrance, 14.0);
            add(&mut e.light.blacks, -6.0);
        }
        "lc.landscape" => {
            add(&mut e.light.contrast, 10.0);
            add(&mut e.color.vibrance, 10.0);
            add(&mut e.mixer.green.sat, 14.0);
            add(&mut e.mixer.aqua.sat, 10.0);
            add(&mut e.mixer.blue.sat, 14.0);
            add(&mut e.mixer.blue.lum, -6.0);
            add(&mut e.effects.clarity, 6.0);
        }
        "lc.portrait" => {
            add(&mut e.light.contrast, -6.0);
            add(&mut e.mixer.orange.sat, -8.0);
            add(&mut e.mixer.orange.lum, 6.0);
            add(&mut e.mixer.red.sat, -5.0);
            add(&mut e.effects.texture, -6.0);
        }
        "lc.mono" => {
            e.treatment = Treatment::Bw;
            add(&mut e.light.contrast, 8.0);
        }
        // ---- Film
        "lc.film.warm-print" => {
            add(&mut e.light.contrast, 6.0);
            add(&mut e.color.saturation, -4.0);
            add(&mut e.mixer.orange.lum, 4.0);
            fade(&mut e.curve.master, 0.04, 0.97, k);
            wheel(&mut e.grading.highlights, 40.0, 18.0, 0.0, k);
            wheel(&mut e.grading.shadows, 25.0, 8.0, 0.0, k);
        }
        "lc.film.cool-fade" => {
            add(&mut e.light.contrast, -8.0);
            add(&mut e.color.saturation, -10.0);
            fade(&mut e.curve.master, 0.07, 0.95, k);
            wheel(&mut e.grading.shadows, 210.0, 16.0, 0.0, k);
            wheel(&mut e.grading.highlights, 190.0, 6.0, 0.0, k);
        }
        "lc.film.golden-hour" => {
            add(&mut e.color.vibrance, 10.0);
            add(&mut e.curve.highlights, -8.0);
            add(&mut e.mixer.orange.sat, 8.0);
            add(&mut e.mixer.yellow.hue, -6.0);
            wheel(&mut e.grading.global, 45.0, 10.0, 0.0, k);
            wheel(&mut e.grading.highlights, 35.0, 20.0, 0.0, k);
        }
        "lc.film.faded-slide" => {
            add(&mut e.light.contrast, 14.0);
            add(&mut e.color.vibrance, 8.0);
            add(&mut e.color.saturation, 6.0);
            add(&mut e.curve.shadows, 10.0);
            fade(&mut e.curve.blue, 0.06, 1.0, k);
            fade(&mut e.curve.red, 0.0, 0.97, k);
        }
        // ---- Cinematic
        "lc.cine.teal-amber" => {
            add(&mut e.light.contrast, 10.0);
            add(&mut e.mixer.orange.sat, 6.0);
            add(&mut e.mixer.green.hue, 20.0);
            add(&mut e.mixer.green.sat, -20.0);
            add(&mut e.mixer.blue.hue, -8.0);
            wheel(&mut e.grading.shadows, 195.0, 30.0, 0.0, k);
            wheel(&mut e.grading.highlights, 38.0, 24.0, 0.0, k);
        }
        "lc.cine.night-blue" => {
            add(&mut e.light.contrast, 12.0);
            add(&mut e.color.saturation, -12.0);
            add(&mut e.curve.darks, -8.0);
            wheel(&mut e.grading.shadows, 220.0, 28.0, -4.0, k);
            wheel(&mut e.grading.midtones, 215.0, 10.0, 0.0, k);
        }
        "lc.cine.desert-heat" => {
            add(&mut e.light.contrast, 8.0);
            add(&mut e.curve.lights, 6.0);
            add(&mut e.mixer.blue.sat, -25.0);
            add(&mut e.mixer.aqua.sat, -20.0);
            wheel(&mut e.grading.global, 32.0, 16.0, 0.0, k);
            wheel(&mut e.grading.highlights, 48.0, 12.0, 0.0, k);
        }
        "lc.cine.neon-dusk" => {
            add(&mut e.light.contrast, 10.0);
            add(&mut e.color.vibrance, 18.0);
            add(&mut e.mixer.magenta.sat, 12.0);
            add(&mut e.mixer.purple.sat, 12.0);
            wheel(&mut e.grading.shadows, 265.0, 22.0, 0.0, k);
            wheel(&mut e.grading.highlights, 330.0, 14.0, 0.0, k);
        }
        // ---- Muted
        "lc.muted.matte-soft" => {
            add(&mut e.light.contrast, -12.0);
            add(&mut e.color.saturation, -14.0);
            add(&mut e.curve.highlights, -6.0);
            fade(&mut e.curve.master, 0.08, 0.96, k);
        }
        "lc.muted.bleached" => {
            add(&mut e.light.contrast, 22.0);
            add(&mut e.color.saturation, -38.0);
            add(&mut e.curve.lights, 6.0);
            add(&mut e.effects.clarity, 8.0);
        }
        "lc.muted.pastel-haze" => {
            add(&mut e.light.contrast, -18.0);
            add(&mut e.color.saturation, -20.0);
            add(&mut e.color.vibrance, 12.0);
            fade(&mut e.curve.master, 0.1, 1.0, k);
            wheel(&mut e.grading.highlights, 330.0, 6.0, 0.0, k);
            wheel(&mut e.grading.shadows, 200.0, 6.0, 0.0, k);
        }
        "lc.muted.quiet-green" => {
            add(&mut e.light.contrast, -4.0);
            add(&mut e.color.saturation, -8.0);
            add(&mut e.mixer.green.sat, -30.0);
            add(&mut e.mixer.green.hue, -10.0);
            add(&mut e.mixer.yellow.sat, -18.0);
            add(&mut e.mixer.aqua.sat, -15.0);
            fade(&mut e.curve.master, 0.04, 1.0, k);
        }
        // ---- B&W
        "lc.bw.mono-rich" => {
            e.treatment = Treatment::Bw;
            add(&mut e.light.contrast, 18.0);
            add(&mut e.curve.darks, -8.0);
            add(&mut e.curve.lights, 6.0);
            add(&mut e.bw_mix.red, 8.0);
            add(&mut e.bw_mix.orange, 10.0);
            add(&mut e.bw_mix.blue, -12.0);
        }
        "lc.bw.red-filter" => {
            e.treatment = Treatment::Bw;
            add(&mut e.light.contrast, 12.0);
            add(&mut e.bw_mix.red, 40.0);
            add(&mut e.bw_mix.orange, 30.0);
            add(&mut e.bw_mix.yellow, 10.0);
            add(&mut e.bw_mix.green, -20.0);
            add(&mut e.bw_mix.aqua, -30.0);
            add(&mut e.bw_mix.blue, -45.0);
        }
        "lc.bw.soft" => {
            e.treatment = Treatment::Bw;
            add(&mut e.light.contrast, -14.0);
            add(&mut e.bw_mix.orange, 12.0);
            fade(&mut e.curve.master, 0.06, 0.97, k);
        }
        "lc.bw.sepia" => {
            e.treatment = Treatment::Bw;
            add(&mut e.light.contrast, 6.0);
            fade(&mut e.curve.master, 0.04, 0.98, k);
            wheel(&mut e.grading.shadows, 28.0, 18.0, 0.0, k);
            wheel(&mut e.grading.highlights, 45.0, 14.0, 0.0, k);
        }
        _ => return Cow::Borrowed(s),
    }
    Cow::Owned(e)
}

/// Ids of the looks [`effective`] implements (besides the identity `lc.color`).
pub const LOOK_IDS: &[&str] = &[
    "lc.neutral",
    "lc.vivid",
    "lc.landscape",
    "lc.portrait",
    "lc.mono",
    "lc.film.warm-print",
    "lc.film.cool-fade",
    "lc.film.golden-hour",
    "lc.film.faded-slide",
    "lc.cine.teal-amber",
    "lc.cine.night-blue",
    "lc.cine.desert-heat",
    "lc.cine.neon-dusk",
    "lc.muted.matte-soft",
    "lc.muted.bleached",
    "lc.muted.pastel-haze",
    "lc.muted.quiet-green",
    "lc.bw.mono-rich",
    "lc.bw.red-filter",
    "lc.bw.soft",
    "lc.bw.sepia",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_borrowed_and_others_change_things() {
        let s = DevelopSettings::default();
        assert!(matches!(effective(&s), Cow::Borrowed(_)));
        for id in LOOK_IDS {
            let mut t = s.clone();
            t.profile.id = (*id).into();
            assert_ne!(*effective(&t), t, "{id}");
            t.profile.amount = 0.0;
            assert_eq!(*effective(&t), t, "{id} at 0%");
        }
    }

    #[test]
    fn looks_scale_with_amount_and_never_add_grain() {
        let mut s = DevelopSettings::default();
        s.light.contrast = 5.0;
        for id in LOOK_IDS {
            s.profile.id = (*id).into();
            let at = |a: f64| {
                let mut t = s.clone();
                t.profile.amount = a;
                effective(&t).into_owned()
            };
            let (half, full, double) = (at(50.0), at(100.0), at(200.0));
            // slider deltas are linear in the amount
            let d = |e: &DevelopSettings| e.light.contrast - s.light.contrast;
            assert!((d(&full) - 2.0 * d(&half)).abs() < 1e-9, "{id}");
            assert!((d(&double) - 2.0 * d(&full)).abs() < 1e-9, "{id}");
            // wheels and point-curve fades grow with the amount
            let lift = |e: &DevelopSettings| e.curve.master.first().map_or(0.0, |p| p.y);
            for (a, b) in [(&half, &full), (&full, &double)] {
                assert!(a.grading.shadows.sat <= b.grading.shadows.sat + 1e-9, "{id}");
                assert!(a.grading.highlights.sat <= b.grading.highlights.sat + 1e-9, "{id}");
                assert!(lift(a) <= lift(b) + 1e-9, "{id}");
            }
            if lift(&full) > 0.0 {
                assert!((lift(&full) - 2.0 * lift(&half)).abs() < 1e-9, "{id}");
            }
            assert_eq!(full.grain, s.grain, "{id} adds no grain");
            assert_eq!(full.profile, s.profile, "{id}");
        }
    }

    #[test]
    fn wheels_and_fades_compose_with_user_settings() {
        let mut s = DevelopSettings::default();
        s.grading.shadows = Wheel { hue: 195.0, sat: 10.0, lum: 0.0 };
        s.curve.master = vec![Point::new(0.0, 0.0), Point::new(0.5, 0.6), Point::new(1.0, 1.0)];
        s.profile.id = "lc.cine.teal-amber".into();
        let e = effective(&s);
        assert!((e.grading.shadows.hue - 195.0).abs() < 1e-6 && (e.grading.shadows.sat - 40.0).abs() < 1e-6, "{:?}", e.grading.shadows);
        assert_eq!(e.curve.master, s.curve.master, "no fade in this look: the user's curve stays");
        s.profile.id = "lc.muted.matte-soft".into();
        let e = effective(&s);
        assert_eq!(e.curve.master.len(), 3, "user points kept");
        let ys: Vec<f64> = e.curve.master.iter().map(|p| p.y).collect();
        for (y, want) in ys.iter().zip([0.08, 0.08 + 0.6 * 0.88, 0.96]) {
            assert!((y - want).abs() < 1e-9, "{ys:?}");
        }
    }
}
