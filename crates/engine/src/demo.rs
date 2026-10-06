//! The demo library: procedurally generated photos with realistic metadata, albums and a few edits.

use std::sync::Arc;

use lightcraft_catalog::{Album, Flag, Meta, Op, Photo, Source};
use lightcraft_develop::DevelopSettings;

use crate::Session;

pub fn load(s: &mut Session) {
    let scenes = lightcraft_scenes::demo_library();
    let mut ids = Vec::new();
    let mut ops = Vec::new();
    for sc in &scenes {
        let id = s.catalog.alloc_photo_id();
        let file = format!("LC{:05}.JPG", 1200 + sc.id * 7);
        let mut p = Photo::new(id, Source::Demo { scene: sc.id }, &file, "JPEG", sc.width, sc.height, "2026-09-28T09:30:00");
        p.captured = Some(sc.meta.captured.clone());
        p.file_size = (sc.width as u64 * sc.height as u64) / 3;
        p.meta = Meta {
            camera: sc.meta.camera.into(),
            lens: sc.meta.lens.into(),
            focal_mm: Some(sc.meta.focal_mm),
            aperture: Some(sc.meta.aperture),
            shutter: sc.meta.shutter.into(),
            iso: Some(sc.meta.iso),
            location: sc.meta.location.into(),
            title: sc.name.clone(),
            keywords: sc.meta.keywords.iter().map(|k| k.to_string()).collect(),
            creator: "LightCraft Demo".into(),
            ..Default::default()
        };
        p.rating = [0, 3, 4, 5, 2, 0, 4, 3][sc.id as usize % 8];
        p.flag = match sc.id % 6 {
            1 => Flag::Pick,
            4 => Flag::Reject,
            _ => Flag::None,
        };
        // A few photos come pre-edited so the grid shows the "edited" badge and variety.
        let mut d = DevelopSettings::default();
        match sc.id {
            3 => {
                d.light.highlights = -60.0;
                d.light.shadows = 35.0;
                d.color.vibrance = 25.0;
                d.vignette.amount = -20.0;
            }
            4 => {
                d.light.exposure = 0.35;
                d.effects.clarity = 20.0;
                d.effects.dehaze = 15.0;
            }
            9 => {
                d.treatment = lightcraft_develop::Treatment::Bw;
                d.light.contrast = 35.0;
            }
            _ => {}
        }
        p.develop = Arc::new(d);
        ids.push(id);
        ops.push(Op::AddPhoto { photo: Box::new(p) });
    }
    let folder = s.catalog.alloc_album_id();
    ops.push(Op::AddAlbum {
        album: Album { id: folder, name: "Travel 2026".into(), parent: None, folder: true, photos: vec![], cover: None, smart: None, quick: false },
    });
    let by_kw = |kw: &str| -> Vec<_> { scenes.iter().zip(&ids).filter(|(sc, _)| sc.meta.keywords.contains(&kw)).map(|(_, id)| *id).collect() };
    for (name, parent, photos) in [
        ("Mountains", Some(folder), by_kw("mountains")),
        ("Coastlines", Some(folder), [by_kw("ocean"), by_kw("beach")].concat()),
        ("Deserts", Some(folder), by_kw("desert")),
        ("Night Sky", None, by_kw("night")),
        ("Garden", None, by_kw("flower")),
        ("Portfolio", None, ids.iter().copied().step_by(3).collect()),
    ] {
        let id = s.catalog.alloc_album_id();
        let cover = photos.first().copied();
        ops.push(Op::AddAlbum { album: Album { id, name: name.into(), parent, folder: false, photos, cover, smart: None, quick: false } });
    }
    for op in ops {
        let _ = s.catalog.apply(op);
    }
    if let Some(first) = s.visible_cloned().first() {
        s.selection = crate::Selection::single(*first);
    }
}
