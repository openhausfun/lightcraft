//! Library-wide keyword commands: list (tree with counts), suggestions, rename, delete, merge;
//! keyword sets (nine keywords a keystroke away: ⌥1–⌥9) and Recent Keywords.

use lightcraft_catalog::keywords::{clean, is_under};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

fn strs(p: &Value, key: &str) -> Vec<String> {
    match p.get(key) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Commit a keyword batch; returns how many photos changed. The keyword filter follows a renamed
/// keyword and is cleared when its keyword is deleted.
fn commit_keywords(s: &mut Session, label: &str, op: lightcraft_catalog::Op, follow: impl Fn(&str) -> Option<String>) -> Result<Value> {
    let n = match &op {
        lightcraft_catalog::Op::Batch { ops } => ops.len(),
        _ => 1,
    };
    if n > 0 {
        s.commit(label, op)?;
    }
    if let Some(k) = s.filter.keyword.clone() {
        s.filter.keyword = follow(&k);
    }
    Ok(json!({"changed": n}))
}

/// A named set of up to nine keywords.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeywordSet {
    pub name: String,
    pub keywords: Vec<String>,
}

/// The set name meaning "the nine most recently added keywords".
pub const RECENT: &str = "Recent Keywords";

/// Remember `added` as the most recent keywords (newest first, nine kept).
pub fn note_recent(s: &mut Session, added: &[String]) {
    for k in added.iter().rev() {
        let k = clean(k);
        if k.is_empty() {
            continue;
        }
        s.recent_keywords.retain(|x| !x.eq_ignore_ascii_case(&k));
        s.recent_keywords.insert(0, k);
    }
    s.recent_keywords.truncate(9);
    let _ = s.save_prefs();
}

/// The nine keywords ⌥1–⌥9 apply: the current set's, or the recent ones.
pub fn current_keywords(s: &Session) -> Vec<String> {
    let set = s.keyword_set.as_deref().and_then(|n| s.keyword_sets.iter().find(|x| x.name.eq_ignore_ascii_case(n)));
    let mut v = set.map_or_else(|| s.recent_keywords.clone(), |x| x.keywords.clone());
    v.truncate(9);
    v
}

/// `{sets: [{name, keywords}], current, keywords}` (Recent Keywords first).
pub fn keyword_sets_json(s: &Session) -> Value {
    let mut sets = vec![json!({"name": RECENT, "keywords": s.recent_keywords})];
    sets.extend(s.keyword_sets.iter().map(|x| json!({"name": x.name, "keywords": x.keywords})));
    json!({"sets": sets, "current": s.keyword_set.clone().unwrap_or_else(|| RECENT.into()), "keywords": current_keywords(s)})
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "keyword.sets", "Keyword Sets", [], None, "{} → {sets: [{name, keywords}], current, keywords: the nine ⌥1–⌥9 apply}", always, |s, _| Ok(keyword_sets_json(s))),
        cmd!("keyword.useSet", "Use Keyword Set", [], None, "{name} (\"Recent Keywords\" = the recently added ones)", always, |s, p| {
            let name = str_param(p, "name").map(str::trim).unwrap_or(RECENT);
            s.keyword_set = if name.eq_ignore_ascii_case(RECENT) || name.is_empty() {
                None
            } else {
                Some(
                    s.keyword_sets
                        .iter()
                        .find(|x| x.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| bad("keyword.useSet", format!("no keyword set `{name}`")))?
                        .name
                        .clone(),
                )
            };
            s.save_prefs()?;
            Ok(keyword_sets_json(s))
        }),
        cmd!(
            "keyword.saveSet",
            "Save Keyword Set",
            [],
            None,
            "{name, keywords?: [up to 9] (default: the current nine)} — replaces a set of that name and makes it current",
            always,
            |s, p| {
                let name = str_param(p, "name")
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(RECENT))
                    .ok_or_else(|| bad("keyword.saveSet", "missing or reserved `name`"))?
                    .to_string();
                let mut keywords: Vec<String> = if p.get("keywords").is_some() {
                    strs(p, "keywords").iter().map(|k| clean(k)).filter(|k| !k.is_empty()).collect()
                } else {
                    current_keywords(s)
                };
                keywords.truncate(9);
                let set = KeywordSet { name: name.clone(), keywords };
                match s.keyword_sets.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) {
                    Some(x) => *x = set,
                    None => s.keyword_sets.push(set),
                }
                s.keyword_set = Some(name);
                s.save_prefs()?;
                Ok(keyword_sets_json(s))
            }
        ),
        cmd!("keyword.deleteSet", "Delete Keyword Set", [], None, "{name}", always, |s, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("keyword.deleteSet", "missing `name`"))?;
            let before = s.keyword_sets.len();
            s.keyword_sets.retain(|x| !x.name.eq_ignore_ascii_case(name));
            if s.keyword_sets.len() == before {
                return Err(bad("keyword.deleteSet", format!("no keyword set `{name}`")));
            }
            if s.keyword_set.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name)) {
                s.keyword_set = None;
            }
            s.save_prefs()?;
            Ok(keyword_sets_json(s))
        }),
        cmd!(
            "keyword.toggleFromSet",
            "Toggle Keyword from Set",
            [],
            None,
            "{index: 1..9, ids?} — the set's keyword N: added to the target photos, or removed when they all have it",
            super::has_selection,
            |s, p| {
                let i = p
                    .get("index")
                    .and_then(Value::as_u64)
                    .filter(|i| (1..=9).contains(i))
                    .ok_or_else(|| bad("keyword.toggleFromSet", "`index` must be 1..9"))?;
                let Some(k) = current_keywords(s).get(i as usize - 1).cloned() else {
                    return Ok(json!({"changed": 0}));
                };
                let ids = s.targets(p);
                let all = !ids.is_empty()
                    && ids.iter().all(|id| s.catalog.photo(*id).is_some_and(|ph| ph.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k))));
                let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
                let key = if all { "removeKeywords" } else { "addKeywords" };
                // applying from Recent Keywords mustn't reshuffle the numbers under the keys
                let recent = s.recent_keywords.clone();
                let mut r = s.execute("photo.setMeta", &json!({"ids": ids, key: [k.clone()]}))?;
                if s.keyword_set.is_none() {
                    s.recent_keywords = recent;
                }
                r["keyword"] = json!(k);
                r["added"] = json!(!all);
                Ok(r)
            }
        ),
        cmd!(query "keyword.list", "Keywords", [], None, "{} → [{name, path, count, children}] keyword tree (`a|b|c` keywords are hierarchical)", always, |s, _| {
            Ok(serde_json::to_value(s.catalog.keyword_tree()).unwrap_or_default())
        }),
        cmd!(
            query "keyword.suggest",
            "Keyword Suggestions",
            [],
            None,
            "{prefix?: typed text, ids?, limit?: 12} → keywords to suggest for the photos (co-occurring / most used, or matching the prefix)",
            always,
            |s, p| {
                let mut current: Vec<String> = Vec::new();
                for id in s.targets(p) {
                    if let Some(ph) = s.catalog.photo(id) {
                        current.extend(ph.meta.keywords.iter().cloned());
                    }
                }
                let n = p.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize;
                Ok(json!(s.catalog.keyword_suggestions(&current, str_param(p, "prefix").unwrap_or(""), n)))
            }
        ),
        cmd!(
            "keyword.rename",
            "Rename Keyword",
            [],
            None,
            "{from, to} — on every photo, children included (`a` → `b` renames `a|x` to `b|x`); renaming onto an existing keyword merges them",
            always,
            |s, p| {
                let from = str_param(p, "from").ok_or_else(|| bad("keyword.rename", "missing `from`"))?.to_string();
                let to = str_param(p, "to").ok_or_else(|| bad("keyword.rename", "missing `to`"))?.to_string();
                let op = s.catalog.rename_keyword_ops(&from, &to).map_err(|e| bad("keyword.rename", e.to_string()))?;
                let (f, t) = (clean(&from), clean(&to));
                commit_keywords(s, "Rename Keyword", op, |k| {
                    Some(if is_under(k, &f) { format!("{t}{}", &k[f.len().min(k.len())..]) } else { k.to_string() })
                })
            }
        ),
        cmd!(
            "keyword.delete",
            "Delete Keyword",
            [],
            None,
            "{keyword} — removes it (and the keywords below it) from every photo",
            always,
            |s, p| {
                let k = str_param(p, "keyword").ok_or_else(|| bad("keyword.delete", "missing `keyword`"))?.to_string();
                let op = s.catalog.delete_keyword_ops(&k).map_err(|e| bad("keyword.delete", e.to_string()))?;
                let c = clean(&k);
                commit_keywords(s, "Delete Keyword", op, |f| (!is_under(f, &c)).then(|| f.to_string()))
            }
        ),
        cmd!(
            "keyword.merge",
            "Merge Keywords",
            [],
            None,
            "{from: [keyword], into: keyword} — replaces each `from` keyword (children included) with `into` on every photo",
            always,
            |s, p| {
                let from = strs(p, "from");
                let into = str_param(p, "into").ok_or_else(|| bad("keyword.merge", "missing `into`"))?.to_string();
                let op = s.catalog.merge_keywords_ops(&from, &into).map_err(|e| bad("keyword.merge", e.to_string()))?;
                let (from, into) = (from.iter().map(|f| clean(f)).collect::<Vec<_>>(), clean(&into));
                commit_keywords(s, "Merge Keywords", op, |k| {
                    Some(match from.iter().find(|f| is_under(k, f)) {
                        Some(f) => format!("{into}{}", &k[f.len().min(k.len())..]),
                        None => k.to_string(),
                    })
                })
            }
        ),
    ]
}
