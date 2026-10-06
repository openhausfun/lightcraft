//! Stacks: photos grouped under a top photo.
//!
//! A stack is catalog data ([`Stack`], changed by `AddStack` / `RemoveStack` / `SetStack` ops), so
//! grouping is undoable and journaled like everything else. The helpers here build the ops for the
//! usual gestures (group, ungroup, remove from stack, set top, collapse) and for programmatic
//! grouping such as merge results stacked with their sources ([`Catalog::stack_with_ops`]).
//! [`Catalog::arrange_stacks`] turns a query result into display order (members together,
//! collapsed stacks reduced to one photo).

use std::collections::{HashMap, HashSet};

use crate::{Catalog, CatalogError, Op, PhotoId, Result, Stack, StackId};

impl Catalog {
    pub fn stack(&self, id: StackId) -> Option<&Stack> {
        self.stacks.get(&id)
    }
    pub fn stacks(&self) -> impl Iterator<Item = &Stack> {
        self.stacks.values()
    }
    /// The stack a photo belongs to.
    pub fn stack_of(&self, photo: PhotoId) -> Option<&Stack> {
        self.stacks.values().find(|s| s.photos.contains(&photo))
    }
    /// Photo → (stack, position in the stack), for every stacked photo.
    pub fn stack_index(&self) -> HashMap<PhotoId, (StackId, usize)> {
        self.stacks.values().flat_map(|s| s.photos.iter().enumerate().map(move |(i, p)| (*p, (s.id, i)))).collect()
    }

    pub(crate) fn validate_stack(&self, id: StackId, photos: &[PhotoId]) -> Result<()> {
        if photos.len() < 2 {
            return Err(CatalogError::Invalid("a stack needs at least two photos".into()));
        }
        let mut seen = HashSet::new();
        for p in photos {
            if !seen.insert(*p) {
                return Err(CatalogError::Invalid(format!("photo {p:?} twice in a stack")));
            }
            if !self.photos.contains_key(p) {
                return Err(CatalogError::NoPhoto(*p));
            }
        }
        if self.stacks.values().any(|s| s.id != id && s.photos.iter().any(|p| seen.contains(p))) {
            return Err(CatalogError::Invalid("a photo can be in only one stack".into()));
        }
        Ok(())
    }

    /// Ops that put `top` and `members` into one stack with `top` on top. Stacks they already
    /// belong to are merged into it (their other members follow, in order). `None` if fewer than
    /// two photos would be in the stack.
    pub fn group_ops(&mut self, top: PhotoId, members: &[PhotoId], collapsed: bool) -> Option<Op> {
        let mut order: Vec<PhotoId> = Vec::new();
        let push = |order: &mut Vec<PhotoId>, p: PhotoId| {
            if !order.contains(&p) && self.photos.contains_key(&p) {
                order.push(p);
            }
        };
        push(&mut order, top);
        for m in members {
            push(&mut order, *m);
        }
        let mut touched: Vec<StackId> = Vec::new();
        for p in order.clone() {
            if let Some(s) = self.stack_of(p)
                && !touched.contains(&s.id)
            {
                touched.push(s.id);
            }
        }
        for sid in &touched {
            for p in self.stacks[sid].photos.clone() {
                push(&mut order, p);
            }
        }
        if order.len() < 2 || order.first() != Some(&top) {
            return None;
        }
        let mut ops: Vec<Op> = touched.iter().skip(1).map(|id| Op::RemoveStack { id: *id }).collect();
        match touched.first() {
            Some(keep) => ops.push(Op::SetStack { id: *keep, photos: order, collapsed }),
            None => {
                let id = self.alloc_stack_id();
                ops.push(Op::AddStack { stack: Stack { id, photos: order, collapsed } });
            }
        }
        Some(Op::Batch { ops })
    }

    /// Stack `sources` under `result` (e.g. an HDR or panorama merge with the photos it was made
    /// from), collapsed so the grid shows the result.
    pub fn stack_with_ops(&mut self, result: PhotoId, sources: &[PhotoId]) -> Option<Op> {
        self.group_ops(result, sources, true)
    }

    /// Move `photo` `delta` places within its stack (towards the top for negative; index 0 is the
    /// top). `None` when it isn't stacked or can't move.
    pub fn move_in_stack_ops(&self, photo: PhotoId, delta: isize) -> Option<Op> {
        let st = self.stack_of(photo)?;
        let i = st.photos.iter().position(|p| *p == photo)?;
        let to = (i as isize + delta).clamp(0, st.photos.len() as isize - 1) as usize;
        if to == i {
            return None;
        }
        let mut photos = st.photos.clone();
        let p = photos.remove(i);
        photos.insert(to, p);
        Some(Op::SetStack { id: st.id, photos, collapsed: st.collapsed })
    }

    /// Split `photo`'s stack in two: the photos before it stay, it and the ones after it form a
    /// new stack (topped by it). A part with a single photo is no longer stacked.
    pub fn split_stack_ops(&mut self, photo: PhotoId) -> Option<Op> {
        let st = self.stack_of(photo)?.clone();
        let i = st.photos.iter().position(|p| *p == photo)?;
        if i == 0 {
            return None;
        }
        let (head, tail) = st.photos.split_at(i);
        let mut ops = vec![if head.len() >= 2 {
            Op::SetStack { id: st.id, photos: head.to_vec(), collapsed: st.collapsed }
        } else {
            Op::RemoveStack { id: st.id }
        }];
        if tail.len() >= 2 {
            let id = self.alloc_stack_id();
            ops.push(Op::AddStack { stack: Stack { id, photos: tail.to_vec(), collapsed: st.collapsed } });
        }
        Some(Op::Batch { ops })
    }

    /// Ops that dissolve every stack containing one of `photos`.
    pub fn ungroup_ops(&self, photos: &[PhotoId]) -> Vec<Op> {
        self.stacks.values().filter(|s| s.photos.iter().any(|p| photos.contains(p))).map(|s| Op::RemoveStack { id: s.id }).collect()
    }

    /// Ops that take `photos` out of their stacks (a stack left with one photo is dissolved).
    pub fn remove_from_stacks_ops(&self, photos: &[PhotoId]) -> Vec<Op> {
        self.stacks
            .values()
            .filter(|s| s.photos.iter().any(|p| photos.contains(p)))
            .map(|s| {
                let rest: Vec<PhotoId> = s.photos.iter().copied().filter(|p| !photos.contains(p)).collect();
                if rest.len() < 2 { Op::RemoveStack { id: s.id } } else { Op::SetStack { id: s.id, photos: rest, collapsed: s.collapsed } }
            })
            .collect()
    }

    /// The op that moves `photo` to the top of its stack (`None` if unstacked or already on top).
    pub fn set_top_ops(&self, photo: PhotoId) -> Option<Op> {
        let s = self.stack_of(photo)?;
        if s.top() == photo {
            return None;
        }
        let mut photos = vec![photo];
        photos.extend(s.photos.iter().copied().filter(|p| *p != photo));
        Some(Op::SetStack { id: s.id, photos, collapsed: s.collapsed })
    }

    /// Ops that collapse or expand stacks.
    pub fn set_collapsed_ops(&self, stacks: &[StackId], collapsed: bool) -> Vec<Op> {
        stacks
            .iter()
            .filter_map(|id| self.stacks.get(id))
            .filter(|s| s.collapsed != collapsed)
            .map(|s| Op::SetStack { id: s.id, photos: s.photos.clone(), collapsed })
            .collect()
    }

    /// Display order for a query result: a stack's members appear together (in stack order) where
    /// its first member appears; a collapsed stack shows only its highest-ranked member in `ids` (the top,
    /// unless the filter hides it).
    pub fn arrange_stacks(&self, ids: &[PhotoId]) -> Vec<PhotoId> {
        if self.stacks.is_empty() {
            return ids.to_vec();
        }
        let index = self.stack_index();
        let present: HashSet<PhotoId> = ids.iter().copied().collect();
        let mut done: HashSet<StackId> = HashSet::new();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            match index.get(id) {
                None => out.push(*id),
                Some((sid, _)) => {
                    if !done.insert(*sid) {
                        continue;
                    }
                    let s = &self.stacks[sid];
                    let members = s.photos.iter().copied().filter(|p| present.contains(p));
                    if s.collapsed {
                        out.extend(members.take(1));
                    } else {
                        out.extend(members);
                    }
                }
            }
        }
        out
    }

    /// Auto-stack by capture time: runs of `ids` (not deleted, not already stacked, with a
    /// capture time) whose consecutive captures are at most `gap_secs` apart, in time order.
    /// Only runs of two or more are returned.
    pub fn auto_stack_groups(&self, ids: &[PhotoId], gap_secs: f64) -> Vec<Vec<PhotoId>> {
        let index = self.stack_index();
        let mut timed: Vec<(i64, PhotoId)> = ids
            .iter()
            .filter_map(|id| self.photos.get(id))
            .filter(|p| !p.deleted && !index.contains_key(&p.id))
            .filter_map(|p| Some((iso_seconds(p.captured.as_deref()?)?, p.id)))
            .collect();
        timed.sort();
        timed.dedup_by_key(|(_, id)| *id);
        let mut groups: Vec<Vec<PhotoId>> = Vec::new();
        let mut last: Option<i64> = None;
        for (t, id) in timed {
            match (last, groups.last_mut()) {
                (Some(l), Some(g)) if (t - l) as f64 <= gap_secs => g.push(id),
                _ => groups.push(vec![id]),
            }
            last = Some(t);
        }
        groups.retain(|g| g.len() >= 2);
        groups
    }
}

/// Seconds since 1970-01-01 for an ISO 8601 local time (`2026-04-01T10:00:00`, `2026-04-01 10:00`,
/// `2026-04-01`; fractions and zone suffixes are ignored). `None` if it doesn't parse.
pub fn iso_seconds(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b).filter(|x| x.bytes().all(|c| c.is_ascii_digit())).and_then(|x| x.parse().ok()) };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let (hh, mm, ss) = if s.len() >= 16 { (num(11, 13)?, num(14, 16)?, num(17, 19).unwrap_or(0)) } else { (0, 0, 0) };
    // days from civil (proleptic Gregorian)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Photo, Source};

    fn cat(n: usize, times: &[&str]) -> (Catalog, Vec<PhotoId>) {
        let mut c = Catalog::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, &format!("{i}.jpg"), "JPEG", 60, 40, "2026-01-01T00:00:00");
            p.captured = times.get(i).map(|t| t.to_string());
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            ids.push(id);
        }
        (c, ids)
    }

    #[test]
    fn iso_seconds_parses() {
        assert_eq!(iso_seconds("1970-01-01T00:00:00"), Some(0));
        assert_eq!(iso_seconds("1970-01-02"), Some(86_400));
        assert_eq!(iso_seconds("2026-04-01T10:00:05.123+02:00").unwrap() - iso_seconds("2026-04-01T10:00:00").unwrap(), 5);
        assert_eq!(iso_seconds("2024-03-01").unwrap() - iso_seconds("2024-02-28").unwrap(), 2 * 86_400, "leap year");
        assert_eq!(iso_seconds("2000-01-01T00:00:00"), Some(946_684_800));
        assert_eq!(iso_seconds("garbage"), None);
    }

    #[test]
    fn group_merge_set_top_remove_and_arrange() {
        let (mut c, ids) = cat(6, &[]);
        let [a, b, x, d, e, f] = ids[..] else { panic!("six ids") };
        let op = c.group_ops(b, &[a, b, x], false).unwrap();
        let inv = c.apply(op).unwrap();
        let s = c.stack_of(a).unwrap().clone();
        assert_eq!(s.photos, vec![b, a, x]);
        // query order a b x d e f → stack shown where its first member appears, in stack order
        assert_eq!(c.arrange_stacks(&ids), vec![b, a, x, d, e, f]);
        // collapse: only the top
        let ops = c.set_collapsed_ops(&[s.id], true);
        c.apply(Op::Batch { ops }).unwrap();
        assert_eq!(c.arrange_stacks(&ids), vec![b, d, e, f]);
        // filtered-out top: first visible member stands in
        assert_eq!(c.arrange_stacks(&[x, d, a]), vec![a, d]);
        // grouping with another stack merges both
        let op = c.group_ops(d, &[e], false).unwrap();
        c.apply(op).unwrap();
        let op = c.group_ops(f, &[a, e], true).unwrap();
        c.apply(op).unwrap();
        assert_eq!(c.stacks().count(), 1);
        assert_eq!(c.stack_of(f).unwrap().photos, vec![f, a, e, b, x, d]);
        // set top, remove from stack, ungroup
        c.apply(c.set_top_ops(x).unwrap()).unwrap();
        assert_eq!(c.stack_of(f).unwrap().top(), x);
        assert!(c.set_top_ops(x).is_none());
        c.apply(Op::Batch { ops: c.remove_from_stacks_ops(&[x, a, e, b]) }).unwrap();
        assert_eq!(c.stack_of(f).unwrap().photos, vec![f, d]);
        c.apply(Op::Batch { ops: c.remove_from_stacks_ops(&[d]) }).unwrap();
        assert_eq!(c.stacks().count(), 0, "a one-photo stack dissolves");
        // undo everything back to the first group's inverse state
        let (mut c2, _) = cat(6, &[]);
        let op = c2.group_ops(a, &[b], false).unwrap();
        c2.apply(op).unwrap();
        c2.apply(Op::Batch { ops: c2.ungroup_ops(&[b]) }).unwrap();
        assert_eq!(c2.stacks().count(), 0);
        let _ = inv;
    }

    #[test]
    fn stack_validation_and_permanent_delete() {
        let (mut c, ids) = cat(3, &[]);
        let id = c.alloc_stack_id();
        assert!(c.apply(Op::AddStack { stack: Stack { id, photos: vec![ids[0]], collapsed: false } }).is_err(), "one photo");
        assert!(c.apply(Op::AddStack { stack: Stack { id, photos: vec![ids[0], ids[0]], collapsed: false } }).is_err(), "duplicate");
        assert!(c.apply(Op::AddStack { stack: Stack { id, photos: vec![ids[0], PhotoId(99)], collapsed: false } }).is_err(), "missing");
        c.apply(Op::AddStack { stack: Stack { id, photos: vec![ids[0], ids[1]], collapsed: true } }).unwrap();
        let id2 = c.alloc_stack_id();
        assert!(c.apply(Op::AddStack { stack: Stack { id: id2, photos: vec![ids[1], ids[2]], collapsed: false } }).is_err(), "one stack per photo");
        // deleting a member permanently dissolves the 2-photo stack; undo restores it
        let before = c.to_snapshot();
        let inv = c.apply(c.delete_permanently_ops(ids[1])).unwrap();
        assert!(c.stack_of(ids[0]).is_none());
        c.apply(inv).unwrap();
        assert_eq!(c.to_snapshot(), before);
        // snapshot round trip keeps stacks and the id allocator
        let back = Catalog::from_snapshot(&c.to_snapshot()).unwrap();
        assert_eq!(back.stack(id), c.stack(id));
        assert_eq!(back.clone().alloc_stack_id(), c.clone().alloc_stack_id());
    }

    #[test]
    fn auto_stack_by_capture_time() {
        let times = ["2026-04-01T10:00:00", "2026-04-01T10:00:02", "2026-04-01T10:00:09", "2026-04-01T11:00:00", "2026-04-01T11:00:01"];
        let (mut c, ids) = cat(6, &times);
        let g = c.auto_stack_groups(&ids, 5.0);
        assert_eq!(g, vec![vec![ids[0], ids[1]], vec![ids[3], ids[4]]]);
        assert_eq!(c.auto_stack_groups(&ids, 10.0), vec![vec![ids[0], ids[1], ids[2]], vec![ids[3], ids[4]]]);
        assert_eq!(c.auto_stack_groups(&ids, 3600.0).len(), 1);
        // already stacked photos are skipped
        let op = c.group_ops(ids[0], &[ids[1]], true).unwrap();
        c.apply(op).unwrap();
        assert_eq!(c.auto_stack_groups(&ids, 10.0), vec![vec![ids[3], ids[4]]]);
    }
}
