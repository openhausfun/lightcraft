//! The develop settings schema. Every field has a neutral default; serde uses `#[serde(default)]` so
//! older/newer files load (unknown fields are ignored, missing fields take defaults).

use lightcraft_geom::{CropGeometry, Homography, Orientation, Point};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevelopSettings {
    pub version: u32,
    pub profile: Profile,
    pub treatment: Treatment,
    pub wb: WhiteBalance,
    pub light: Light,
    pub curve: ToneCurve,
    pub color: ColorAdj,
    pub mixer: Mixer,
    /// Point Color: up to [`MAX_POINT_COLORS`] sampled colours, each with its own adjustment.
    pub point_colors: Vec<PointColor>,
    pub bw_mix: BwMix,
    pub grading: ColorGrading,
    pub effects: Effects,
    pub vignette: Vignette,
    pub grain: Grain,
    pub detail: Detail,
    pub optics: Optics,
    pub geometry: Geometry,
    pub crop: Crop,
    pub orientation: Orientation,
    pub masks: Vec<Mask>,
    pub spots: Vec<Spot>,
    pub red_eye: Vec<RedEye>,
    pub lens_blur: LensBlur,
    pub enhance: Enhance,
    /// Camera calibration: shadows tint and primary hue/saturation (applied before tone mapping).
    pub calibration: Calibration,
    /// Section on/off toggles (the "eye" buttons on panel headers): section id → enabled.
    pub disabled_sections: Vec<String>,
}

impl Default for DevelopSettings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            profile: Profile::default(),
            treatment: Treatment::Color,
            wb: WhiteBalance::default(),
            light: Light::default(),
            curve: ToneCurve::default(),
            color: ColorAdj::default(),
            mixer: Mixer::default(),
            point_colors: Vec::new(),
            bw_mix: BwMix::default(),
            grading: ColorGrading::default(),
            effects: Effects::default(),
            vignette: Vignette::default(),
            grain: Grain::default(),
            detail: Detail::default(),
            optics: Optics::default(),
            geometry: Geometry::default(),
            crop: Crop::default(),
            orientation: Orientation::Normal,
            masks: Vec::new(),
            spots: Vec::new(),
            red_eye: Vec::new(),
            lens_blur: LensBlur::default(),
            enhance: Enhance::default(),
            calibration: Calibration::default(),
            disabled_sections: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Treatment {
    #[default]
    Color,
    Bw,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Profile id from the profile registry (e.g. `lc.color`, `lc.vivid`, `lc.mono`).
    pub id: String,
    /// Creative profile amount 0..200 (%).
    pub amount: f64,
}

impl Default for Profile {
    fn default() -> Self {
        Self { id: "lc.color".into(), amount: 100.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WbMode {
    #[default]
    AsShot,
    Auto,
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
    Custom,
}

impl WbMode {
    pub const ALL: [WbMode; 9] = [
        WbMode::AsShot,
        WbMode::Auto,
        WbMode::Daylight,
        WbMode::Cloudy,
        WbMode::Shade,
        WbMode::Tungsten,
        WbMode::Fluorescent,
        WbMode::Flash,
        WbMode::Custom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            WbMode::AsShot => "As Shot",
            WbMode::Auto => "Auto",
            WbMode::Daylight => "Daylight",
            WbMode::Cloudy => "Cloudy",
            WbMode::Shade => "Shade",
            WbMode::Tungsten => "Tungsten",
            WbMode::Fluorescent => "Fluorescent",
            WbMode::Flash => "Flash",
            WbMode::Custom => "Custom",
        }
    }
    /// Preset temperature/tint for raw files (K, tint).
    pub fn preset(self) -> Option<(f64, f64)> {
        match self {
            WbMode::Daylight => Some((5500.0, 10.0)),
            WbMode::Cloudy => Some((6500.0, 10.0)),
            WbMode::Shade => Some((7500.0, 10.0)),
            WbMode::Tungsten => Some((2850.0, 0.0)),
            WbMode::Fluorescent => Some((3800.0, 21.0)),
            WbMode::Flash => Some((5500.0, 0.0)),
            _ => None,
        }
    }
}

/// White balance. For raw files `temp` is the scene illuminant in Kelvin; for rendered files
/// (JPEG etc.) the image is assumed D65-balanced and `temp`/`tint` are the same model with an
/// "as shot" of 6500 K / 0 (the UI shows a relative −100..100 scale for those, like Lightroom).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhiteBalance {
    pub mode: WbMode,
    pub temp: f64,
    pub tint: f64,
}

impl Default for WhiteBalance {
    fn default() -> Self {
        Self { mode: WbMode::AsShot, temp: 6500.0, tint: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    pub highlights: f64,
    pub lights: f64,
    pub darks: f64,
    pub shadows: f64,
    /// Region split points (percent), Lightroom defaults 25/50/75.
    pub split_shadows: f64,
    pub split_mid: f64,
    pub split_highlights: f64,
    /// Point curves in 0..1 (identity = empty or [(0,0),(1,1)]).
    pub master: Vec<Point>,
    pub red: Vec<Point>,
    pub green: Vec<Point>,
    pub blue: Vec<Point>,
    /// Refine Saturation 0..100: 100 keeps the saturation a curve produces, lower values pull it
    /// back towards the saturation before the curve (strong contrast curves oversaturate).
    pub refine_saturation: f64,
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self {
            highlights: 0.0,
            lights: 0.0,
            darks: 0.0,
            shadows: 0.0,
            split_shadows: 25.0,
            split_mid: 50.0,
            split_highlights: 75.0,
            master: Vec::new(),
            red: Vec::new(),
            green: Vec::new(),
            blue: Vec::new(),
            refine_saturation: 100.0,
        }
    }
}

impl ToneCurve {
    pub fn point_curve_is_identity(pts: &[Point]) -> bool {
        pts.is_empty() || pts.iter().all(|p| (p.x - p.y).abs() < 1e-9)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorAdj {
    pub vibrance: f64,
    pub saturation: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hsl {
    pub hue: f64,
    pub sat: f64,
    pub lum: f64,
}

/// The 8 colour-mixer bands in Lightroom order.
pub const MIXER_BANDS: [&str; 8] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];
/// Centre hue of each band in degrees (sRGB-ish HSV hue).
pub const MIXER_HUES: [f64; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 225.0, 270.0, 315.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mixer {
    pub red: Hsl,
    pub orange: Hsl,
    pub yellow: Hsl,
    pub green: Hsl,
    pub aqua: Hsl,
    pub blue: Hsl,
    pub purple: Hsl,
    pub magenta: Hsl,
}

impl Mixer {
    pub fn bands(&self) -> [Hsl; 8] {
        [self.red, self.orange, self.yellow, self.green, self.aqua, self.blue, self.purple, self.magenta]
    }
    pub fn is_neutral(&self) -> bool {
        self.bands().iter().all(|b| *b == Hsl::default())
    }
}

/// Most Point Color samples per photo (Lightroom's limit).
pub const MAX_POINT_COLORS: usize = 8;

/// One Point Color sample: a colour picked on the image (OkLCh, at the stage where Point Color
/// applies: after the colour mixer) and the adjustment of the colours around it. The range is a
/// box in OkLCh around the sample, widened by `range` (overall) and the per-axis widths, with a
/// soft edge.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointColor {
    /// Sampled OkLab lightness (0..1).
    pub lum: f64,
    /// Sampled OkLCh chroma (0..~0.37).
    pub chroma: f64,
    /// Sampled OkLCh hue, degrees.
    pub hue: f64,
    /// Hue shift −100..100 (±~29° at the sample).
    pub hue_shift: f64,
    /// Saturation shift −100..100 (chroma scale).
    pub sat_shift: f64,
    /// Luminance shift −100..100.
    pub lum_shift: f64,
    /// −100 compresses the colour spread within the range towards the sample (evens out e.g.
    /// skin), +100 expands it.
    pub variance: f64,
    /// Overall range 0..100 (scales all three widths).
    pub range: f64,
    pub hue_range: f64,
    pub sat_range: f64,
    pub lum_range: f64,
}

impl Default for PointColor {
    fn default() -> Self {
        Self {
            lum: 0.5,
            chroma: 0.1,
            hue: 0.0,
            hue_shift: 0.0,
            sat_shift: 0.0,
            lum_shift: 0.0,
            variance: 0.0,
            range: 50.0,
            hue_range: 50.0,
            sat_range: 50.0,
            lum_range: 50.0,
        }
    }
}

impl PointColor {
    /// True when the sample changes nothing.
    pub fn is_neutral(&self) -> bool {
        self.hue_shift == 0.0 && self.sat_shift == 0.0 && self.lum_shift == 0.0 && self.variance == 0.0
    }
}

/// B&W mix (used when treatment = Bw): per-band luminance contribution −100..100.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BwMix {
    pub red: f64,
    pub orange: f64,
    pub yellow: f64,
    pub green: f64,
    pub aqua: f64,
    pub blue: f64,
    pub purple: f64,
    pub magenta: f64,
}

impl BwMix {
    pub fn bands(&self) -> [f64; 8] {
        [self.red, self.orange, self.yellow, self.green, self.aqua, self.blue, self.purple, self.magenta]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    /// Hue in degrees 0..360.
    pub hue: f64,
    /// Saturation 0..100.
    pub sat: f64,
    /// Luminance −100..100.
    pub lum: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// 0..100, default 50.
    pub blending: f64,
    /// −100..100.
    pub balance: f64,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            shadows: Wheel::default(),
            midtones: Wheel::default(),
            highlights: Wheel::default(),
            global: Wheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    pub fn is_neutral(&self) -> bool {
        [self.shadows, self.midtones, self.highlights, self.global].iter().all(|w| w.sat == 0.0 && w.lum == 0.0)
    }
}

/// Camera calibration (Lightroom Classic's Calibration panel): a green/magenta tint of the shadows,
/// and a hue rotation / saturation scale of each working-space primary (−100..100), i.e. a
/// white-preserving 3×3 matrix applied in scene-linear light before tone mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub shadows_tint: f64,
    pub red_hue: f64,
    pub red_sat: f64,
    pub green_hue: f64,
    pub green_sat: f64,
    pub blue_hue: f64,
    pub blue_sat: f64,
}

impl Calibration {
    /// (hue, saturation) of the red, green and blue primaries.
    pub fn primaries(&self) -> [(f64, f64); 3] {
        [(self.red_hue, self.red_sat), (self.green_hue, self.green_sat), (self.blue_hue, self.blue_sat)]
    }
    pub fn is_neutral(&self) -> bool {
        *self == Calibration::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub texture: f64,
    pub clarity: f64,
    pub dehaze: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VignetteStyle {
    #[default]
    HighlightPriority,
    ColorPriority,
    PaintOverlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    pub amount: f64,
    pub midpoint: f64,
    pub roundness: f64,
    pub feather: f64,
    pub highlights: f64,
    pub style: VignetteStyle,
}

impl Default for Vignette {
    fn default() -> Self {
        Self { amount: 0.0, midpoint: 50.0, roundness: 0.0, feather: 50.0, highlights: 0.0, style: VignetteStyle::HighlightPriority }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    pub amount: f64,
    pub size: f64,
    pub roughness: f64,
    /// Seed for the procedural grain (stable per photo).
    pub seed: u32,
}

impl Default for Grain {
    fn default() -> Self {
        Self { amount: 0.0, size: 25.0, roughness: 50.0, seed: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub sharpen_amount: f64,
    pub sharpen_radius: f64,
    pub sharpen_detail: f64,
    pub sharpen_masking: f64,
    pub nr_luminance: f64,
    pub nr_detail: f64,
    pub nr_contrast: f64,
    pub nr_color: f64,
    pub nr_color_detail: f64,
    pub nr_color_smoothness: f64,
}

impl Default for Detail {
    /// Lightroom's raw defaults are sharpening 40/1.0/25/0 and colour NR 25/50/50; we use neutral
    /// values for rendered files and apply raw defaults at import time (`DevelopSettings::for_raw`).
    fn default() -> Self {
        Self {
            sharpen_amount: 0.0,
            sharpen_radius: 1.0,
            sharpen_detail: 25.0,
            sharpen_masking: 0.0,
            nr_luminance: 0.0,
            nr_detail: 50.0,
            nr_contrast: 0.0,
            nr_color: 0.0,
            nr_color_detail: 50.0,
            nr_color_smoothness: 50.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Optics {
    pub remove_ca: bool,
    pub lens_profile: bool,
    pub profile_distortion: f64,
    pub profile_vignetting: f64,
    pub defringe_purple_amount: f64,
    pub defringe_purple_hue_lo: f64,
    pub defringe_purple_hue_hi: f64,
    pub defringe_green_amount: f64,
    pub defringe_green_hue_lo: f64,
    pub defringe_green_hue_hi: f64,
    pub distortion: f64,
    pub vignetting: f64,
    pub vignetting_midpoint: f64,
    /// Manual lateral chromatic aberration (red/cyan, blue/yellow fringes), −100..100, added to the
    /// automatic estimate when `remove_ca` is on.
    pub ca_red: f64,
    pub ca_blue: f64,
}

impl Default for Optics {
    fn default() -> Self {
        Self {
            remove_ca: false,
            lens_profile: false,
            profile_distortion: 100.0,
            profile_vignetting: 100.0,
            defringe_purple_amount: 0.0,
            defringe_purple_hue_lo: 30.0,
            defringe_purple_hue_hi: 70.0,
            defringe_green_amount: 0.0,
            defringe_green_hue_lo: 40.0,
            defringe_green_hue_hi: 60.0,
            distortion: 0.0,
            vignetting: 0.0,
            vignetting_midpoint: 50.0,
            ca_red: 0.0,
            ca_blue: 0.0,
        }
    }
}

/// Lens corrections embedded in a DNG file (`OpcodeList3`: `WarpRectilinear`, `FixVignetteRadial`), converted
/// to the oriented, default-cropped image. This is camera/file data (stored on the photo record, not in the develop
/// settings); "Enable Profile Corrections" applies it, scaled by the profile distortion/vignetting amounts.
/// LightCraft never uses Adobe LCP lens profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddedLens {
    pub warp: Option<EmbeddedWarp>,
    pub vignette: Option<EmbeddedVignette>,
}

/// DNG `WarpRectilinear`: per plane (R, G, B) `[kr0, kr1, kr2, kr3, kt0, kt1]`. For an output (corrected) point at
/// offset `d = (p − center) / radius`, the source point is `center + radius · (d·f(r²) + tangential(d))`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedWarp {
    pub planes: [[f64; 6]; 3],
    /// Optical centre, normalized to the oriented image (0..1).
    pub center: Point,
    /// Normalisation radius as a fraction of the oriented image's long edge.
    pub radius: f64,
}

/// DNG `FixVignetteRadial`: gain `1 + k0 r² + k1 r⁴ + … + k4 r¹⁰` with `r = |p − center| / radius`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedVignette {
    pub k: [f64; 5],
    pub center: Point,
    pub radius: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Upright {
    #[default]
    Off,
    Auto,
    Guided,
    Level,
    Vertical,
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    pub upright: Upright,
    /// Guided Upright guides, up to 4 line segments, in normalized coordinates of the lens-corrected
    /// (pre-perspective) oriented image.
    pub guides: Vec<(Point, Point)>,
    /// The automatic Upright correction found by analysis for `upright` (Auto/Level/Vertical/Full), as a
    /// homography lens-corrected → transformed in centred coordinates (`(p − centre) / (long edge / 2)`).
    /// Stored so previews and exports use the identical transform; recomputed by `geometry.upright`.
    pub upright_transform: Option<Homography>,
    pub vertical: f64,
    pub horizontal: f64,
    pub rotate: f64,
    pub aspect: f64,
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub constrain_crop: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            upright: Upright::Off,
            guides: Vec::new(),
            upright_transform: None,
            vertical: 0.0,
            horizontal: 0.0,
            rotate: 0.0,
            aspect: 0.0,
            scale: 100.0,
            offset_x: 0.0,
            offset_y: 0.0,
            constrain_crop: false,
        }
    }
}

impl Geometry {
    pub fn is_identity(&self) -> bool {
        self.upright == Upright::Off
            && self.vertical == 0.0
            && self.horizontal == 0.0
            && self.rotate == 0.0
            && self.aspect == 0.0
            && self.scale == 100.0
            && self.offset_x == 0.0
            && self.offset_y == 0.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub geometry: CropGeometry,
    /// Locked aspect (w, h); None = free / original.
    pub aspect: Option<(u32, u32)>,
    pub flip_h: bool,
    pub flip_v: bool,
}

// ---------------------------------------------------------------------------------------------
// Masks and local adjustments

/// The sliders available inside a mask (Lightroom's local adjustments).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAdjustments {
    pub temp: f64,
    pub tint: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    pub texture: f64,
    pub clarity: f64,
    pub dehaze: f64,
    pub hue: f64,
    pub saturation: f64,
    pub sharpness: f64,
    pub noise: f64,
    pub moire: f64,
    pub defringe: f64,
    /// Colour overlay (hue degrees, saturation 0..100); sat 0 = none.
    pub color_hue: f64,
    pub color_sat: f64,
    /// Local vignette/grain are not in Lightroom; we keep the struct extensible.
    pub amount: f64,
}

impl Default for LocalAdjustments {
    fn default() -> Self {
        Self {
            temp: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            hue: 0.0,
            saturation: 0.0,
            sharpness: 0.0,
            noise: 0.0,
            moire: 0.0,
            defringe: 0.0,
            color_hue: 0.0,
            color_sat: 0.0,
            amount: 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskOp {
    #[default]
    Add,
    Subtract,
    Intersect,
}

/// One brush dab sequence. Coordinates normalized to the uncropped oriented image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushStroke {
    pub points: Vec<Point>,
    /// Radius as a fraction of the image's long edge.
    pub size: f64,
    /// 0..100.
    pub feather: f64,
    pub flow: f64,
    pub density: f64,
    pub erase: bool,
    pub auto_mask: bool,
}

impl Default for BrushStroke {
    fn default() -> Self {
        Self { points: Vec::new(), size: 0.03, feather: 50.0, flow: 100.0, density: 100.0, erase: false, auto_mask: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MaskShape {
    Brush {
        strokes: Vec<BrushStroke>,
    },
    /// Linear gradient: full effect at `start`, fading to none at `end` (normalized coords).
    Linear {
        start: Point,
        end: Point,
    },
    /// Radial gradient: ellipse centre, radii (normalized to the long edge), rotation (deg), feather 0..100.
    Radial {
        center: Point,
        rx: f64,
        ry: f64,
        angle: f64,
        feather: f64,
        invert: bool,
    },
    /// Colour range: sampled OkLab colours with a refine amount 0..100.
    ColorRange {
        samples: Vec<[f64; 3]>,
        refine: f64,
    },
    /// Luminance range (0..1 bounds with feathered falloff 0..1).
    LuminanceRange {
        lo: f64,
        hi: f64,
        lo_feather: f64,
        hi_feather: f64,
    },
    DepthRange {
        lo: f64,
        hi: f64,
        feather: f64,
    },
    /// AI / automatic selections, evaluated by a segmenter.
    Subject,
    Sky,
    Background,
    Object {
        hint: Vec<Point>,
    },
    People {
        person: u32,
        parts: Vec<String>,
    },
    Landscape {
        class: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskComponent {
    /// A name given in the Masking panel (Rename); `None` shows the shape's kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub op: MaskOp,
    #[serde(default)]
    pub invert: bool,
    pub shape: MaskShape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub invert: bool,
    pub components: Vec<MaskComponent>,
    pub adjust: LocalAdjustments,
    /// Refine Edges 0..100: the mask's edges snap to the photo's (guided filter).
    #[serde(skip_serializing_if = "is_zero")]
    pub refine: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Default for Mask {
    fn default() -> Self {
        Self { id: 0, name: "Mask 1".into(), visible: true, invert: false, components: Vec::new(), adjust: LocalAdjustments::default(), refine: 0.0 }
    }
}

// ---------------------------------------------------------------------------------------------
// Heal / remove / red eye

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpotMode {
    #[default]
    Remove,
    Heal,
    Clone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spot {
    pub mode: SpotMode,
    /// Target: a single point (circle spot) or a brushed path. Normalized coords.
    pub points: Vec<Point>,
    /// Radius as a fraction of the long edge.
    pub size: f64,
    pub feather: f64,
    pub opacity: f64,
    /// Source offset (normalized) from the target; None = auto (content-aware remove).
    pub source_offset: Option<Point>,
}

impl Default for Spot {
    fn default() -> Self {
        Self { mode: SpotMode::Remove, points: Vec::new(), size: 0.02, feather: 50.0, opacity: 100.0, source_offset: None }
    }
}

/// A red eye / pet eye correction: the user's ellipse (centre normalized, radii as fractions of
/// the long edge); the pupil inside it is found automatically.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedEye {
    pub center: Point,
    pub rx: f64,
    pub ry: f64,
    /// 0..100 (50 = the detected pupil).
    pub pupil_size: f64,
    /// 0..100.
    pub darken: f64,
    pub pet: bool,
    /// Pet eye catchlight: offset of its centre from the pupil centre, in pupil radii.
    #[serde(default)]
    pub catchlight: Option<Point>,
}

impl Default for RedEye {
    fn default() -> Self {
        Self { center: Point::new(0.5, 0.5), rx: 0.02, ry: 0.015, pupil_size: 50.0, darken: 50.0, pet: false, catchlight: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensBlur {
    pub enabled: bool,
    pub amount: f64,
    pub focal_distance: f64,
    pub focal_range: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Enhance {
    pub denoise: f64,
    pub raw_details: bool,
    pub super_resolution: bool,
}
