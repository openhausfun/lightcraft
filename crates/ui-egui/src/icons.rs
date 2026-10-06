//! Our own vector icons, drawn in code on a 20×20 design grid (1.5 px strokes, rounded caps).
//! Original work (see assets/ATTRIBUTION.md) — not derived from any third-party icon set.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Sidebar,
    Back,
    Forward,
    Search,
    Filter,
    Bell,
    Share,
    Help,
    Cloud,
    Sliders,
    Crop,
    Eraser,
    Mask,
    Eye,
    /// Eye with a slash (hidden).
    EyeOff,
    Presets,
    Versions,
    Activity,
    More,
    Tag,
    Info,
    GridPhoto,
    GridSquare,
    Single,
    Compare,
    Survey,
    Sort,
    Star,
    StarFilled,
    Circle,
    FlagPick,
    FlagReject,
    Gear,
    Filmstrip,
    BeforeAfter,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    TriangleLeft,
    Plus,
    Minus,
    Album,
    SmartAlbum,
    Stack,
    Folder,
    /// Edit (a pencil).
    Pencil,
    Photos,
    Clock,
    Trash,
    Curve,
    ProfileGrid,
    Close,
    Check,
    Brush,
    Linear,
    Radial,
    Sky,
    Subject,
    Picker,
    Rotate,
    Flip,
    Invert,
    Dots,
    Video,
    Heart,
    Edited,
    /// Targeted adjustment: a ring with a centre dot and up/down arrows (drag vertically).
    Target,
    /// Community chat (opens the ArtCraft Discord): a speech bubble with three dots. Our own
    /// generic drawing, not any service's logo.
    Chat,
}

struct Pen<'a> {
    p: &'a Painter,
    r: Rect,
    s: Stroke,
    k: f32,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.r.min.x + x * self.k, self.r.min.y + y * self.k)
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::line(v, self.s));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::closed_line(v, self.s));
    }
    fn fill(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::convex_polygon(v, c, Stroke::NONE));
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.pt(x, y), r * self.k, self.s);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p.circle_filled(self.pt(x, y), r * self.k, self.s.color);
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        self.p.rect_stroke(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), round * self.k, self.s, egui::StrokeKind::Middle);
    }
    fn rect_fill(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        self.p.rect_filled(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), round * self.k, self.s.color);
    }
    fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let n = 24;
        let v: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.line(&v);
    }
}

fn star_pts(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
    (0..10)
        .map(|i| {
            let a = (-90.0 + i as f32 * 36.0f32).to_radians();
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect()
}

/// Paint `icon` centred in `rect` (square area used) with `color`.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(side, side));
    let k = side / 20.0;
    let pen = Pen { p, r, s: Stroke::new((1.5 * k).max(1.0), color), k };
    use Icon::*;
    match icon {
        Sidebar => {
            pen.rect(3.0, 4.0, 17.0, 16.0, 1.5);
            pen.line(&[(8.0, 4.0), (8.0, 16.0)]);
        }
        Back => pen.line(&[(16.0, 10.0), (4.0, 10.0), (9.0, 5.0)]),
        Forward => pen.line(&[(4.0, 10.0), (16.0, 10.0), (11.0, 5.0)]),
        Search => {
            pen.circle(8.5, 8.5, 5.0);
            pen.line(&[(12.2, 12.2), (16.5, 16.5)]);
        }
        Filter => pen.closed(&[(3.0, 4.0), (17.0, 4.0), (11.5, 10.5), (11.5, 16.0), (8.5, 14.5), (8.5, 10.5)]),
        Bell => {
            pen.line(&[(5.0, 14.0), (5.0, 9.0)]);
            pen.arc(10.0, 9.0, 5.0, 180.0, 360.0);
            pen.line(&[(15.0, 9.0), (15.0, 14.0)]);
            pen.line(&[(3.5, 14.0), (16.5, 14.0)]);
            pen.arc(10.0, 15.5, 1.8, 0.0, 180.0);
        }
        Share => {
            pen.line(&[(6.0, 8.0), (4.0, 8.0), (4.0, 17.0), (16.0, 17.0), (16.0, 8.0), (14.0, 8.0)]);
            pen.line(&[(10.0, 12.0), (10.0, 2.5)]);
            pen.line(&[(6.5, 6.0), (10.0, 2.5), (13.5, 6.0)]);
        }
        Help => {
            pen.circle(10.0, 10.0, 7.5);
            pen.arc(10.0, 8.0, 2.5, 180.0, 400.0);
            pen.line(&[(10.0, 10.5), (10.0, 11.8)]);
            pen.dot(10.0, 14.2, 0.9);
        }
        Cloud => {
            pen.arc(7.0, 11.5, 3.5, 90.0, 270.0);
            pen.arc(10.5, 8.5, 4.5, 190.0, 340.0);
            pen.arc(14.0, 11.8, 3.2, 270.0, 450.0);
            pen.line(&[(7.0, 15.0), (14.0, 15.0)]);
        }
        Sliders => {
            for (y, x) in [(5.0, 12.0), (10.0, 7.0), (15.0, 13.0)] {
                pen.line(&[(3.0, y), (x - 2.0, y)]);
                pen.line(&[(x + 2.0, y), (17.0, y)]);
                pen.circle(x, y, 2.0);
            }
        }
        Crop => {
            pen.line(&[(6.0, 2.5), (6.0, 14.0), (17.5, 14.0)]);
            pen.line(&[(2.5, 6.0), (14.0, 6.0), (14.0, 17.5)]);
            pen.line(&[(15.5, 2.0), (17.5, 4.0), (15.5, 6.0)]);
        }
        Eraser => {
            pen.closed(&[(3.0, 12.0), (10.0, 5.0), (16.0, 11.0), (11.0, 16.0), (7.0, 16.0)]);
            pen.line(&[(6.5, 8.5), (12.5, 14.5)]);
            pen.line(&[(7.0, 17.5), (17.0, 17.5)]);
        }
        Mask => {
            pen.circle(10.0, 10.0, 7.0);
            for i in 0..5 {
                for j in 0..5 {
                    let (x, y) = (5.0 + i as f32 * 2.5, 5.0 + j as f32 * 2.5);
                    if (x - 10.0).hypot(y - 10.0) < 5.8 {
                        pen.dot(x, y, 0.6);
                    }
                }
            }
        }
        Eye => {
            pen.arc(10.0, 16.0, 9.0, 212.0, 328.0);
            pen.arc(10.0, 4.0, 9.0, 32.0, 148.0);
            pen.circle(10.0, 10.0, 2.5);
        }
        EyeOff => {
            pen.arc(10.0, 16.0, 9.0, 212.0, 328.0);
            pen.arc(10.0, 4.0, 9.0, 32.0, 148.0);
            pen.circle(10.0, 10.0, 2.5);
            pen.line(&[(3.5, 3.5), (16.5, 16.5)]);
        }
        Presets => {
            pen.circle(8.0, 8.0, 5.0);
            pen.circle(12.0, 12.0, 5.0);
        }
        Versions => {
            pen.rect(5.0, 3.0, 17.0, 13.0, 1.5);
            pen.line(&[(3.0, 6.0), (3.0, 16.0), (14.0, 16.0)]);
        }
        Activity => {
            pen.circle(10.0, 10.0, 7.0);
            pen.line(&[(10.0, 6.0), (10.0, 10.0), (13.0, 12.0)]);
        }
        More | Dots => {
            for x in [5.0, 10.0, 15.0] {
                pen.dot(x, 10.0, 1.3);
            }
        }
        Tag => {
            pen.closed(&[(3.0, 3.5), (10.0, 3.5), (17.0, 10.5), (10.5, 17.0), (3.0, 10.0)]);
            pen.dot(7.0, 7.5, 1.2);
        }
        Info => {
            pen.circle(10.0, 10.0, 7.5);
            pen.line(&[(10.0, 9.0), (10.0, 14.0)]);
            pen.dot(10.0, 6.3, 0.9);
        }
        GridPhoto => {
            pen.rect_fill(3.0, 4.0, 10.0, 9.5, 0.5);
            pen.rect_fill(11.0, 4.0, 17.0, 9.5, 0.5);
            pen.rect_fill(3.0, 10.5, 8.0, 16.0, 0.5);
            pen.rect_fill(9.0, 10.5, 17.0, 16.0, 0.5);
        }
        GridSquare => {
            pen.rect_fill(3.5, 4.0, 9.5, 9.5, 0.5);
            pen.rect_fill(10.5, 4.0, 16.5, 9.5, 0.5);
            pen.rect_fill(3.5, 10.5, 9.5, 16.0, 0.5);
            pen.rect_fill(10.5, 10.5, 16.5, 16.0, 0.5);
        }
        Single => pen.rect_fill(3.0, 5.0, 17.0, 15.0, 0.5),
        Survey => {
            pen.rect(2.5, 3.5, 9.0, 9.0, 1.0);
            pen.rect(11.0, 3.5, 17.5, 9.0, 1.0);
            pen.rect(2.5, 11.0, 9.0, 16.5, 1.0);
            pen.rect(11.0, 11.0, 17.5, 16.5, 1.0);
        }
        Compare => {
            pen.rect_fill(2.0, 5.0, 8.8, 15.0, 0.5);
            pen.rect_fill(11.2, 5.0, 18.0, 15.0, 0.5);
        }
        Sort => {
            pen.line(&[(3.0, 5.0), (17.0, 5.0)]);
            pen.line(&[(3.0, 10.0), (13.0, 10.0)]);
            pen.line(&[(3.0, 15.0), (9.0, 15.0)]);
        }
        Star => pen.closed(&star_pts(10.0, 10.5, 7.5)),
        StarFilled => {
            let pts = star_pts(10.0, 10.5, 7.5);
            // convex fan: fill triangles from the centre
            for i in 0..10 {
                let a = pts[i];
                let b = pts[(i + 1) % 10];
                pen.fill(&[(10.0, 10.5), a, b], color);
            }
        }
        Circle => pen.circle(10.0, 10.0, 6.0),
        FlagPick => {
            pen.line(&[(5.0, 17.0), (5.0, 3.5)]);
            pen.closed(&[(5.0, 3.5), (15.0, 3.5), (12.5, 7.0), (15.0, 10.5), (5.0, 10.5)]);
        }
        FlagReject => {
            pen.line(&[(5.0, 17.0), (5.0, 3.5)]);
            pen.closed(&[(5.0, 3.5), (15.0, 3.5), (12.5, 7.0), (15.0, 10.5), (5.0, 10.5)]);
            pen.line(&[(8.0, 5.0), (12.0, 9.0)]);
            pen.line(&[(12.0, 5.0), (8.0, 9.0)]);
        }
        Gear => {
            pen.circle(10.0, 10.0, 2.5);
            for i in 0..8 {
                let a = (i as f32 * 45.0f32).to_radians();
                pen.line(&[(10.0 + 5.2 * a.cos(), 10.0 + 5.2 * a.sin()), (10.0 + 7.5 * a.cos(), 10.0 + 7.5 * a.sin())]);
            }
            pen.circle(10.0, 10.0, 5.2);
        }
        Filmstrip => {
            pen.rect(2.5, 5.0, 17.5, 15.0, 1.0);
            pen.rect_fill(5.0, 11.0, 8.5, 13.5, 0.3);
            pen.rect_fill(9.5, 11.0, 13.0, 13.5, 0.3);
            pen.line(&[(2.5, 9.5), (17.5, 9.5)]);
        }
        BeforeAfter => {
            pen.rect(3.0, 4.0, 17.0, 16.0, 1.0);
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.rect_fill(10.0, 4.0, 17.0, 16.0, 0.5);
        }
        ChevronDown => pen.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]),
        ChevronRight => pen.line(&[(7.5, 5.0), (12.5, 10.0), (7.5, 15.0)]),
        ChevronLeft => pen.line(&[(12.5, 5.0), (7.5, 10.0), (12.5, 15.0)]),
        TriangleLeft => pen.fill(&[(13.0, 6.0), (13.0, 14.0), (7.0, 10.0)], color),
        Plus => {
            pen.line(&[(10.0, 4.0), (10.0, 16.0)]);
            pen.line(&[(4.0, 10.0), (16.0, 10.0)]);
        }
        Minus => pen.line(&[(4.0, 10.0), (16.0, 10.0)]),
        Album => {
            pen.rect(3.0, 5.0, 15.0, 17.0, 1.0);
            pen.line(&[(6.0, 2.5), (17.5, 2.5), (17.5, 14.0)]);
        }
        Stack => {
            pen.rect(2.5, 7.0, 13.5, 17.5, 1.0);
            pen.line(&[(5.0, 4.5), (16.0, 4.5), (16.0, 14.5)]);
            pen.line(&[(7.5, 2.0), (18.5, 2.0), (18.5, 11.5)]);
        }
        SmartAlbum => {
            pen.rect(3.0, 5.0, 15.0, 17.0, 1.0);
            pen.line(&[(6.0, 2.5), (17.5, 2.5), (17.5, 14.0)]);
            // a small funnel: the album is a saved filter
            pen.closed(&[(5.5, 8.5), (12.5, 8.5), (10.0, 11.5), (10.0, 14.5), (8.0, 13.5), (8.0, 11.5)]);
        }
        Folder => pen.closed(&[(2.5, 5.0), (8.0, 5.0), (9.5, 7.0), (17.5, 7.0), (17.5, 15.5), (2.5, 15.5)]),
        Pencil => {
            pen.closed(&[(4.0, 16.0), (4.5, 12.5), (13.5, 3.5), (16.5, 6.5), (7.5, 15.5)]);
            pen.line(&[(11.5, 5.5), (14.5, 8.5)]);
        }
        Photos => {
            pen.rect(2.5, 4.0, 17.5, 16.0, 1.5);
            pen.line(&[(2.5, 13.5), (7.0, 9.5), (11.0, 13.0), (13.5, 11.0), (17.5, 14.5)]);
            pen.dot(13.5, 7.5, 1.3);
        }
        Clock => {
            pen.circle(10.0, 10.0, 7.0);
            pen.line(&[(10.0, 5.5), (10.0, 10.0), (13.5, 11.5)]);
        }
        Trash => {
            pen.line(&[(3.0, 5.5), (17.0, 5.5)]);
            pen.line(&[(8.0, 5.5), (8.0, 3.0), (12.0, 3.0), (12.0, 5.5)]);
            pen.closed(&[(5.0, 5.5), (15.0, 5.5), (14.0, 17.0), (6.0, 17.0)]);
        }
        Curve => {
            pen.rect(3.0, 3.0, 17.0, 17.0, 1.5);
            let pts: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let y = t * t * (3.0 - 2.0 * t);
                    (4.5 + 11.0 * t, 15.5 - 11.0 * y)
                })
                .collect();
            pen.line(&pts);
        }
        ProfileGrid => {
            pen.rect_fill(3.5, 3.5, 9.0, 9.0, 0.5);
            pen.rect_fill(11.0, 3.5, 16.5, 9.0, 0.5);
            pen.rect_fill(3.5, 11.0, 9.0, 16.5, 0.5);
            pen.circle(14.0, 14.0, 2.5);
            pen.line(&[(15.8, 15.8), (17.5, 17.5)]);
        }
        Close => {
            pen.line(&[(5.0, 5.0), (15.0, 15.0)]);
            pen.line(&[(15.0, 5.0), (5.0, 15.0)]);
        }
        Check => pen.line(&[(4.0, 10.5), (8.5, 15.0), (16.0, 5.5)]),
        Brush => {
            pen.line(&[(16.5, 3.5), (9.0, 11.0)]);
            pen.closed(&[(9.0, 11.0), (6.0, 11.5), (4.0, 14.5), (3.5, 17.0), (6.0, 16.5), (8.5, 14.0)]);
        }
        Linear => {
            for (i, x) in [4.0, 7.5, 11.0, 14.5].iter().enumerate() {
                let a = 1.0 - i as f32 * 0.22;
                pen.p.line_segment([pen.pt(*x, 3.0), pen.pt(*x, 17.0)], Stroke::new(pen.s.width, color.gamma_multiply(a)));
            }
        }
        Radial => {
            pen.circle(10.0, 10.0, 7.0);
            pen.p.circle_stroke(pen.pt(10.0, 10.0), 3.5 * k, Stroke::new(pen.s.width, color.gamma_multiply(0.5)));
        }
        Sky => {
            pen.arc(8.0, 11.0, 3.2, 150.0, 300.0);
            pen.arc(12.0, 9.5, 3.8, 210.0, 360.0);
            pen.line(&[(5.0, 13.5), (15.5, 13.5)]);
            pen.line(&[(2.5, 17.0), (17.5, 17.0)]);
        }
        Subject => {
            pen.circle(10.0, 7.0, 3.0);
            pen.arc(10.0, 18.0, 6.5, 200.0, 340.0);
            pen.rect(2.5, 2.5, 17.5, 17.5, 2.0);
        }
        Picker => {
            pen.line(&[(4.0, 16.0), (11.5, 8.5)]);
            pen.closed(&[(11.0, 5.0), (15.0, 9.0), (17.0, 7.0), (13.0, 3.0)]);
        }
        Rotate => {
            pen.arc(10.0, 10.0, 6.5, 200.0, 470.0);
            pen.line(&[(3.0, 5.5), (4.0, 9.0), (7.5, 8.0)]);
        }
        Flip => {
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.closed(&[(8.0, 5.0), (8.0, 15.0), (3.0, 15.0)]);
            pen.fill(&[(12.0, 5.0), (12.0, 15.0), (17.0, 15.0)], color);
        }
        Invert => {
            pen.circle(10.0, 10.0, 7.0);
            pen.p.add(Shape::convex_polygon(
                (0..=16)
                    .map(|i| {
                        pen.pt(
                            10.0 + 7.0 * ((90.0 + i as f32 * 180.0 / 16.0f32).to_radians()).cos(),
                            10.0 + 7.0 * ((90.0 + i as f32 * 180.0 / 16.0f32).to_radians()).sin(),
                        )
                    })
                    .collect(),
                color,
                Stroke::NONE,
            ));
        }
        Video => {
            pen.rect(2.5, 5.0, 13.0, 15.0, 1.5);
            pen.closed(&[(13.0, 8.5), (17.5, 6.0), (17.5, 14.0), (13.0, 11.5)]);
        }
        Heart => pen.closed(&[(10.0, 16.5), (3.5, 9.5), (4.0, 5.5), (7.5, 4.0), (10.0, 6.5), (12.5, 4.0), (16.0, 5.5), (16.5, 9.5)]),
        Edited => {
            pen.circle(10.0, 10.0, 7.0);
            pen.line(&[(7.0, 13.0), (13.0, 7.0)]);
        }
        Target => {
            pen.circle(8.0, 10.0, 5.0);
            pen.dot(8.0, 10.0, 1.3);
            pen.line(&[(15.5, 3.5), (15.5, 16.5)]);
            pen.line(&[(13.5, 5.5), (15.5, 3.5), (17.5, 5.5)]);
            pen.line(&[(13.5, 14.5), (15.5, 16.5), (17.5, 14.5)]);
        }
        Chat => {
            // rounded bubble with a tail at the lower left
            pen.closed(&[
                (4.5, 3.5),
                (15.5, 3.5),
                (17.0, 5.0),
                (17.0, 12.0),
                (15.5, 13.5),
                (9.0, 13.5),
                (5.5, 16.8),
                (5.5, 13.5),
                (4.5, 13.5),
                (3.0, 12.0),
                (3.0, 5.0),
            ]);
            for x in [6.8, 10.0, 13.2] {
                pen.dot(x, 8.5, 1.1);
            }
        }
    }
}
