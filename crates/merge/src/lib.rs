//! LightCraft Photo Merge: HDR merge and panorama stitching (layer L3, no UI dependencies).
//!
//! - [`frame`]: inputs — scene-linear frames from raw files (camera RGB) or standard images.
//! - [`hdr`]: exposure brackets → one scene-linear radiance image (auto-align, exposure
//!   normalisation, saturation-aware weighting, deghosting with an overlay).
//! - [`pano`]: overlapping photos → one panorama (features, matching, RANSAC, rotation bundle
//!   adjustment, spherical/cylindrical/perspective projection, exposure compensation, seams,
//!   multi-band blending, boundary warp, auto crop, fill edges).
//! - [`output`]: results as linear DNGs (16-bit float for HDR) with the source's colour tags.
//!
//! Every algorithm here is our own implementation from published papers (cited in each module);
//! no code from GPL projects was consulted.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod align;
pub mod features;
pub mod frame;
pub mod hdr;
pub mod linalg;
pub mod output;
pub mod pano;
pub mod ransac;
pub mod synth;

pub use frame::{Frame, FrameColor, load_frame};
pub use hdr::{Deghost, HdrOptions, HdrResult, merge_hdr};
pub use output::{DngSamples, write_linear_dng};
pub use pano::{PanoOptions, PanoResult, Projection, stitch};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MergeError {
    #[error("{0} photo(s) selected; at least {1} are needed")]
    TooFew(usize, usize),
    #[error("{0}")]
    Mismatch(String),
    #[error("could not read a photo: {0}")]
    Decode(String),
    #[error("{0}")]
    NoOverlap(String),
    #[error("cancelled")]
    Cancelled,
    #[error("could not write the result: {0}")]
    Output(String),
}

pub type Result<T> = std::result::Result<T, MergeError>;

/// Progress callback: `(fraction 0..1, stage)`; return `false` to cancel.
pub type Progress<'a> = dyn Fn(f32, &str) -> bool + Sync + 'a;

/// A progress callback that ignores progress and never cancels.
pub fn no_progress(_: f32, _: &str) -> bool {
    true
}

pub(crate) fn check(p: &Progress, f: f32, stage: &str) -> Result<()> {
    if p(f.clamp(0.0, 1.0), stage) { Ok(()) } else { Err(MergeError::Cancelled) }
}
