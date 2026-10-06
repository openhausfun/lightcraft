//! Library-wide keyword operations: the keyword tree (hierarchical keywords are written
//! `parent|child|grandchild`), rename / delete / merge across every photo, and suggestions.
//!
//! Rename, delete and merge produce one [`Op::Batch`] of [`Op::SetMeta`]s — one per photo that
//! changes — so they are a single undo step and replay from the op log like any other edit.
//! Keyword names compare case-insensitively; renaming a keyword renames its children too
//! (`travel|italy` → `trips|italy`).

use serde::Serialize;

use crate::{Catalog, CatalogError, Meta, Op, Result};

/// Separator of hierarchical keyword levels.
pub const SEP: char = '|';

/// Normalize a keyword: trim every level, drop empty levels.
pub fn clean(k: &str) -> String {
    k.split(SEP).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("|")
}

/// `k` is `parent` or one of its descendants (`parent|…`), ignoring case.
pub fn is_under(k: &str, parent: &str) -> bool {
    let (k, p) = (k.to_lowercase(), parent.to_lowercase());
    k == p || k.strip_prefix(&p).is_some_and(|rest| rest.starts_with(SEP))
}

/// Replace the `from` prefix of `k` (which [`is_under`] `from`) with `to`.
fn reparent(k: &str, from: &str, to: &str) -> String {
    let rest = &k[from.len().min(k.len())..];
    if to.is_empty() { rest.trim_start_matches(SEP).to_string() } else { format!("{to}{rest}") }
}

/// Keep the first of case-insensitively equal keywords.
fn dedupe(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|k| !k.is_empty() && seen.insert(k.to_lowercase()));
}

/// One node of the keyword tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeywordNode {
    /// This level's name (`italy`).
    pub name: String,
    /// The full keyword (`travel|italy`).
    pub path: String,
    /// Photos (not deleted) with this keyword or one below it.
    pub count: usize,
    pub children: Vec<KeywordNode>,
}

impl Catalog {
    /// The keyword tree: every keyword level with photo counts, sorted by name (case-insensitive).
    pub fn keyword_tree(&self) -> Vec<KeywordNode> {
        use std::collections::{HashMap, HashSet};
        // every keyword level (`travel`, `travel|italy`…) by its lower-case path: the name and
        // path as first written, and the photos counted (once per photo, even when two of its
        // keywords share a parent)
        struct Level {
            name: String,
            path: String,
            count: usize,
            parent: Option<String>,
        }
        let mut levels: HashMap<String, Level> = HashMap::new();
        let mut this_photo: HashSet<String> = HashSet::new();
        for p in self.photos().filter(|p| p.in_library()) {
            this_photo.clear();
            for k in &p.meta.keywords {
                let mut path = String::new();
                let mut lower = String::new();
                let mut parent: Option<String> = None;
                for part in k.split(SEP).map(str::trim).filter(|s| !s.is_empty()) {
                    if !path.is_empty() {
                        path.push(SEP);
                        lower.push(SEP);
                    }
                    path.push_str(part);
                    lower.push_str(&part.to_lowercase());
                    let l = levels.entry(lower.clone()).or_insert_with(|| Level {
                        name: part.to_string(),
                        path: path.clone(),
                        count: 0,
                        parent: parent.clone(),
                    });
                    if this_photo.insert(lower.clone()) {
                        l.count += 1;
                    }
                    parent = Some(lower.clone());
                }
            }
        }
        let mut kids: HashMap<Option<String>, Vec<String>> = HashMap::new();
        for (lower, l) in &levels {
            kids.entry(l.parent.clone()).or_default().push(lower.clone());
        }
        fn build(at: Option<String>, levels: &HashMap<String, Level>, kids: &mut HashMap<Option<String>, Vec<String>>) -> Vec<KeywordNode> {
            let Some(mut list) = kids.remove(&at) else { return Vec::new() };
            list.sort_by_key(|l| levels[l].name.to_lowercase());
            list.into_iter()
                .map(|l| {
                    let lv = &levels[&l];
                    let children = build(Some(l.clone()), levels, kids);
                    KeywordNode { name: lv.name.clone(), path: lv.path.clone(), count: lv.count, children }
                })
                .collect()
        }
        build(None, &levels, &mut kids)
    }

    /// `SetMeta` ops for every photo whose keywords `f` changes.
    fn keyword_ops(&self, f: impl Fn(&[String]) -> Vec<String>) -> Vec<Op> {
        self.photos()
            .filter_map(|p| {
                let mut kws = f(&p.meta.keywords);
                dedupe(&mut kws);
                (kws != p.meta.keywords).then(|| Op::SetMeta { id: p.id, meta: Box::new(Meta { keywords: kws, ..p.meta.clone() }) })
            })
            .collect()
    }

    /// Rename `from` (and the keywords below it) to `to` on every photo — one batch. Renaming onto
    /// an existing keyword merges the two.
    pub fn rename_keyword_ops(&self, from: &str, to: &str) -> Result<Op> {
        let (from, to) = (clean(from), clean(to));
        if from.is_empty() || to.is_empty() {
            return Err(CatalogError::Invalid("keyword names can't be empty".into()));
        }
        if is_under(&to, &from) && !to.eq_ignore_ascii_case(&from) {
            return Err(CatalogError::Invalid("can't move a keyword below itself".into()));
        }
        let ops = self.keyword_ops(|kws| kws.iter().map(|k| if is_under(k, &from) { reparent(k, &from, &to) } else { k.clone() }).collect());
        Ok(Op::Batch { ops })
    }

    /// Remove `keyword` and the keywords below it from every photo.
    pub fn delete_keyword_ops(&self, keyword: &str) -> Result<Op> {
        let k = clean(keyword);
        if k.is_empty() {
            return Err(CatalogError::Invalid("empty keyword".into()));
        }
        Ok(Op::Batch { ops: self.keyword_ops(|kws| kws.iter().filter(|x| !is_under(x, &k)).cloned().collect()) })
    }

    /// Merge several keywords (with their children) into `into`.
    pub fn merge_keywords_ops(&self, from: &[String], into: &str) -> Result<Op> {
        let into = clean(into);
        let from: Vec<String> = from.iter().map(|f| clean(f)).filter(|f| !f.is_empty() && !f.eq_ignore_ascii_case(&into)).collect();
        if into.is_empty() || from.is_empty() {
            return Err(CatalogError::Invalid("merge needs keywords and a target".into()));
        }
        if from.iter().any(|f| is_under(&into, f)) {
            return Err(CatalogError::Invalid("can't merge a keyword into one below it".into()));
        }
        let ops = self.keyword_ops(|kws| {
            kws.iter().map(|k| from.iter().find(|f| is_under(k, f)).map(|f| reparent(k, f, &into)).unwrap_or_else(|| k.clone())).collect()
        });
        Ok(Op::Batch { ops })
    }

    /// Keyword suggestions for a photo that has `current` keywords: with a typed `prefix`, the
    /// library's keywords containing it (those starting with it first); otherwise keywords that
    /// appear together with `current` on other photos, then the most used ones. Most frequent
    /// first, at most `n`, never one of `current`.
    pub fn keyword_suggestions(&self, current: &[String], prefix: &str, n: usize) -> Vec<String> {
        let all = self.keywords();
        let has = |k: &str| current.iter().any(|c| c.eq_ignore_ascii_case(k));
        let q = prefix.trim().to_lowercase();
        let mut scored: Vec<(i64, String)> = if !q.is_empty() {
            all.into_iter()
                .filter(|(k, _)| !has(k))
                .filter_map(|(k, c)| {
                    let l = k.to_lowercase();
                    let leaf = l.rsplit(SEP).next().unwrap_or(&l).to_string();
                    let rank = if l.starts_with(&q) || leaf.starts_with(&q) {
                        2
                    } else if l.contains(&q) {
                        1
                    } else {
                        return None;
                    };
                    Some((rank * 1_000_000 + c as i64, k))
                })
                .collect()
        } else {
            let mut co: std::collections::HashMap<String, i64> = Default::default();
            if !current.is_empty() {
                for p in self.photos().filter(|p| p.in_library()) {
                    if p.meta.keywords.iter().any(|k| has(k)) {
                        for k in p.meta.keywords.iter().filter(|k| !has(k)) {
                            *co.entry(k.clone()).or_default() += 1;
                        }
                    }
                }
            }
            all.into_iter().filter(|(k, _)| !has(k)).map(|(k, c)| (co.get(&k).copied().unwrap_or(0) * 1_000_000 + c as i64, k)).collect()
        };
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        scored.into_iter().take(n).map(|(_, k)| k).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Filter, Photo, PhotoId, Source};

    fn lib(kws: &[&[&str]]) -> (Catalog, Vec<PhotoId>) {
        let mut c = Catalog::new();
        let mut ids = Vec::new();
        for k in kws {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "a.jpg", "JPEG", 3, 2, "2026-01-01");
            p.meta.keywords = k.iter().map(|s| s.to_string()).collect();
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            ids.push(id);
        }
        (c, ids)
    }
    fn kws(c: &Catalog, id: PhotoId) -> Vec<String> {
        c.photo(id).unwrap().meta.keywords.clone()
    }

    #[test]
    fn tree_counts_photos_per_level() {
        let (c, _) = lib(&[&["travel|Italy|Rome", "beach"], &["Travel|italy"], &["travel|France", "travel|italy|rome"], &["beach"]]);
        let t = c.keyword_tree();
        assert_eq!(t.iter().map(|n| (n.name.as_str(), n.count)).collect::<Vec<_>>(), vec![("beach", 2), ("travel", 3)]);
        let travel = &t[1];
        assert_eq!(travel.children.iter().map(|n| (n.path.as_str(), n.count)).collect::<Vec<_>>(), vec![("travel|France", 1), ("travel|Italy", 3)]);
        assert_eq!(travel.children[1].children[0].path, "travel|Italy|Rome");
        assert_eq!(travel.children[1].children[0].count, 2);
        // filtering by a parent finds its children
        let f = Filter { keyword: Some("travel|italy".into()), ..Default::default() };
        assert_eq!(c.query(&f, &Default::default()).len(), 3);
    }

    #[test]
    fn rename_delete_merge_are_single_undoable_batches() {
        let (mut c, ids) = lib(&[&["travel|italy|rome", "beach"], &["Travel|Italy"], &["italia", "travel|italy"], &["sea"]]);
        let before = c.to_snapshot();
        let op = c.rename_keyword_ops("travel|italy", "Europe|Italy").unwrap();
        let Op::Batch { ops } = &op else { panic!() };
        assert_eq!(ops.len(), 3, "only photos that change");
        let inv = c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[0]), ["Europe|Italy|rome", "beach"]);
        assert_eq!(kws(&c, ids[1]), ["Europe|Italy"]);
        c.apply(inv).unwrap();
        assert_eq!(c.to_snapshot(), before);
        // merge `italia` into the existing hierarchical keyword: duplicates collapse
        let op = c.merge_keywords_ops(&["italia".into()], "travel|italy").unwrap();
        c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[2]), ["travel|italy"]);
        // delete removes the keyword and its children everywhere
        let op = c.delete_keyword_ops("TRAVEL").unwrap();
        c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[0]), ["beach"]);
        assert!(kws(&c, ids[1]).is_empty());
        assert_eq!(c.keywords(), vec![("beach".to_string(), 1), ("sea".to_string(), 1)]);
        // invalid requests
        assert!(c.rename_keyword_ops("beach", " ").is_err());
        assert!(c.rename_keyword_ops("a", "a|b").is_err());
        assert!(c.merge_keywords_ops(&["a".into()], "a|b").is_err());
        assert_eq!(clean(" a | |b "), "a|b");
    }

    #[test]
    fn suggestions_rank_co_occurrence_and_prefixes() {
        let (c, _) = lib(&[&["dog", "park"], &["dog", "park", "ball"], &["dog", "beach"], &["cat"], &["cat"], &["cat"], &["parade"], &["eagle"]]);
        // co-occurring with `dog` first (park twice), then the most used
        let s = c.keyword_suggestions(&["dog".into()], "", 3);
        assert_eq!(s, ["park", "ball", "beach"]);
        let s = c.keyword_suggestions(&[], "", 2);
        assert_eq!(s, ["cat", "dog"]);
        let s = c.keyword_suggestions(&["park".into()], "pa", 5);
        assert_eq!(s, ["parade"]);
        let s = c.keyword_suggestions(&[], "E", 5);
        assert_eq!(s, ["eagle", "beach", "parade"], "prefix matches before substring matches");
    }
}
