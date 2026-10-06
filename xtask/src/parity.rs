//! `cargo xtask parity`: the Lightroom parity tracker (`docs/parity.md`).
//!
//! - **Reference check** (in `ci`): every `` `cmd:<id>` `` must be a registered command (engine command specs, as
//!   listed by `lightcraft-cli commands --json`, or a UI command from `UI_COMMANDS`), every `` `ctl:<id>` `` a develop
//!   control (`lightcraft-cli controls --json`), and every repository path in backticks must exist (with `:line`
//!   inside the file). A trailing `*` matches an id prefix. Row ids must be unique and statuses valid.
//! - **Summary**: per section done / partial / missing / out of scope and the share of P0 and P1 rows done;
//!   `--write` refreshes the table between the `parity:summary` markers in the document.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

pub const DOC: &str = "docs/parity.md";
const BEGIN: &str = "<!-- parity:summary -->";
const END: &str = "<!-- /parity:summary -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Done,
    Partial,
    Missing,
    OutOfScope,
}

impl Status {
    fn parse(cell: &str) -> Option<Status> {
        let c = cell.trim_start();
        [("✅", Status::Done), ("🟡", Status::Partial), ("⬜", Status::Missing), ("🚫", Status::OutOfScope)]
            .into_iter()
            .find(|(e, _)| c.starts_with(e))
            .map(|(_, s)| s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    Cmd(String),
    Ctl(String),
    /// Repository-relative path, optional 1-based line.
    Path(String, Option<usize>),
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub tier: String,
    pub status: Status,
    pub section: String,
}

#[derive(Debug, Default)]
pub struct Doc {
    pub rows: Vec<Row>,
    /// (line, reference) for every reference anywhere in the document.
    pub refs: Vec<(usize, Ref)>,
    pub errors: Vec<String>,
}

fn is_row_id(s: &str) -> bool {
    let Some((prefix, rest)) = s.split_once('-') else { return false };
    matches!(prefix, "LR" | "LRC" | "MENU" | "KEY" | "KEYC")
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

/// Directories whose paths are checked (others, e.g. the local-only `plan/`, are ignored).
const CHECKED_DIRS: &[&str] = &["crates/", "apps/", "docs/", "xtask/", "assets/"];

/// A concrete id (not a placeholder such as `cmd:<id>` in prose).
fn concrete(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '*'))
}

fn classify(span: &str) -> Option<Ref> {
    if let Some(id) = span.strip_prefix("cmd:").filter(|id| concrete(id)) {
        return Some(Ref::Cmd(id.to_string()));
    }
    if let Some(id) = span.strip_prefix("ctl:").filter(|id| concrete(id)) {
        return Some(Ref::Ctl(id.to_string()));
    }
    if CHECKED_DIRS.iter().any(|d| span.starts_with(d)) && !span.contains(' ') {
        return Some(match span.rsplit_once(':') {
            Some((p, l)) if l.parse::<usize>().is_ok() => Ref::Path(p.to_string(), l.parse().ok()),
            _ => Ref::Path(span.to_string(), None),
        });
    }
    None
}

/// Inline-code spans of one line (single backticks).
fn code_spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

pub fn parse(md: &str) -> Doc {
    let mut doc = Doc::default();
    let mut section = String::new();
    let mut seen = HashSet::new();
    let mut in_comment = false;
    for (i, line) in md.lines().enumerate() {
        let n = i + 1;
        let t = line.trim();
        if t.starts_with("<!--") && !t.contains("-->") {
            in_comment = true;
        }
        if in_comment {
            in_comment = !t.contains("-->");
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            section = h.trim().to_string();
            continue;
        }
        for span in code_spans(line) {
            if let Some(r) = classify(span) {
                doc.refs.push((n, r));
            }
        }
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        let Some(id) = cells.first().copied().filter(|c| is_row_id(c)) else { continue };
        if cells.len() < 6 {
            doc.errors.push(format!("{DOC}:{n}: {id}: expected 6 columns (Id | Feature | Tier | Status | Evidence | Notes)"));
            continue;
        }
        let Some(status) = Status::parse(cells[3]) else {
            doc.errors.push(format!("{DOC}:{n}: {id}: status `{}` is not one of ✅ 🟡 ⬜ 🚫", cells[3]));
            continue;
        };
        if !seen.insert(id.to_string()) {
            doc.errors.push(format!("{DOC}:{n}: duplicate id {id}"));
        }
        doc.rows.push(Row { id: id.to_string(), tier: cells[2].to_string(), status, section: section.clone() });
    }
    doc
}

/// Ids in `UI_COMMANDS` (`("view.detail", "Detail", Some("D"), "View"),` lines).
pub fn ui_command_ids(menus_rs: &str) -> Vec<String> {
    let Some(start) = menus_rs.find("pub const UI_COMMANDS") else { return Vec::new() };
    let body = &menus_rs[start..];
    let body = &body[..body.find("];").unwrap_or(body.len())];
    body.lines().filter_map(|l| l.trim().strip_prefix("(\"")).filter_map(|l| l.split_once('"').map(|(id, _)| id.to_string())).collect()
}

fn known(set: &BTreeSet<String>, id: &str) -> bool {
    match id.strip_suffix('*') {
        Some(prefix) => set.iter().any(|s| s.starts_with(prefix)),
        None => set.contains(id),
    }
}

/// Reference problems (empty = OK). `file_lines` returns a repository file's line count (None = missing).
pub fn check(doc: &Doc, commands: &BTreeSet<String>, controls: &BTreeSet<String>, file_lines: impl Fn(&str) -> Option<usize>) -> Vec<String> {
    let mut out = doc.errors.clone();
    for (n, r) in &doc.refs {
        match r {
            Ref::Cmd(id) if !known(commands, id) => out.push(format!("{DOC}:{n}: unknown command `cmd:{id}`")),
            Ref::Ctl(id) if !known(controls, id) => out.push(format!("{DOC}:{n}: unknown develop control `ctl:{id}`")),
            Ref::Path(p, line) => match (file_lines(p), line) {
                (None, _) => out.push(format!("{DOC}:{n}: missing path `{p}`")),
                (Some(len), Some(l)) if *l == 0 || *l > len => out.push(format!("{DOC}:{n}: `{p}:{l}` is past the end ({len} lines)")),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub done: usize,
    pub partial: usize,
    pub missing: usize,
    pub oos: usize,
    pub p0: (usize, usize),
    pub p1: (usize, usize),
}

impl Counts {
    fn add(&mut self, r: &Row) {
        match r.status {
            Status::Done => self.done += 1,
            Status::Partial => self.partial += 1,
            Status::Missing => self.missing += 1,
            Status::OutOfScope => self.oos += 1,
        }
        let done = usize::from(r.status == Status::Done);
        match r.tier.as_str() {
            "P0" => self.p0 = (self.p0.0 + done, self.p0.1 + 1),
            "P1" => self.p1 = (self.p1.0 + done, self.p1.1 + 1),
            _ => {}
        }
    }
}

/// Per-section counts in document order, plus the total.
pub fn summarize(doc: &Doc) -> (Vec<(String, Counts)>, Counts) {
    let mut sections: Vec<(String, Counts)> = Vec::new();
    let mut total = Counts::default();
    for r in &doc.rows {
        if sections.last().is_none_or(|(s, _)| *s != r.section) {
            sections.push((r.section.clone(), Counts::default()));
        }
        sections.last_mut().expect("pushed").1.add(r);
        total.add(r);
    }
    (sections, total)
}

fn pct((done, n): (usize, usize)) -> String {
    match (done * 100 + n / 2).checked_div(n) {
        Some(p) => format!("{done}/{n} ({p}%)"),
        None => "—".into(),
    }
}

pub fn summary_table(doc: &Doc) -> String {
    let (sections, total) = summarize(doc);
    let mut s = String::from("| Section | ✅ | 🟡 | ⬜ | 🚫 | P0 done | P1 done |\n|---|---:|---:|---:|---:|---:|---:|\n");
    let row =
        |name: &str, c: &Counts| format!("| {name} | {} | {} | {} | {} | {} | {} |\n", c.done, c.partial, c.missing, c.oos, pct(c.p0), pct(c.p1));
    for (name, c) in &sections {
        s += &row(name, c);
    }
    s += &row("**Total**", &total);
    s += &format!("\n{}\n", weighted_line(doc));
    s
}

/// Weighted completion of the in-scope rows (✅ = 1, 🟡 = ½, ⬜ = 0; 🚫 left out) for `tier`
/// (`None` = all): (percent, rows).
pub fn weighted(doc: &Doc, tier: Option<&str>) -> (f64, usize) {
    let rows: Vec<&Row> = doc.rows.iter().filter(|r| r.status != Status::OutOfScope && tier.is_none_or(|t| r.tier == t)).collect();
    let score: f64 = rows
        .iter()
        .map(|r| match r.status {
            Status::Done => 1.0,
            Status::Partial => 0.5,
            _ => 0.0,
        })
        .sum();
    (if rows.is_empty() { 0.0 } else { score * 100.0 / rows.len() as f64 }, rows.len())
}

/// "Weighted completion: 65.7% of 500 rows (P0 …, P1 …, P2 …)".
pub fn weighted_line(doc: &Doc) -> String {
    let (all, n) = weighted(doc, None);
    let tiers: Vec<String> = ["P0", "P1", "P2"]
        .iter()
        .filter_map(|t| Some(weighted(doc, Some(t))).filter(|w| w.1 > 0).map(|(p, n)| format!("{t} {p:.1}% of {n}")))
        .collect();
    format!("Weighted completion (✅ = 1, 🟡 = ½, 🚫 left out): **{all:.1}%** of {n} in-scope rows — {}.", tiers.join(" · "))
}

/// Replace the summary between the markers.
pub fn with_summary(md: &str, table: &str) -> Option<String> {
    let b = md.find(BEGIN)? + BEGIN.len();
    let e = md[b..].find(END)? + b;
    Some(format!("{}\n{}{}", &md[..b], table, &md[e..]))
}

fn registry(what: &str) -> Result<BTreeSet<String>, String> {
    let out = crate::cargo()
        .args(["run", "-q", "-p", "lightcraft-cli", "--", what, "--json"])
        .output()
        .map_err(|e| format!("lightcraft-cli {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!("lightcraft-cli {what} --json failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("lightcraft-cli {what}: bad JSON: {e}"))?;
    Ok(v.as_array().into_iter().flatten().filter_map(|c| c["id"].as_str().map(str::to_string)).collect())
}

pub fn run(root: &Path, write: bool) -> Result<(), String> {
    let path = root.join(DOC);
    let md = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = parse(&md);
    let menus = std::fs::read_to_string(root.join("crates/ui-egui/src/menus.rs")).map_err(|e| format!("menus.rs: {e}"))?;
    let mut commands = registry("commands")?;
    commands.extend(ui_command_ids(&menus));
    let controls = registry("controls")?;
    let problems = check(&doc, &commands, &controls, |p| {
        let full = root.join(p);
        if full.is_dir() { Some(0) } else { std::fs::read_to_string(full).ok().map(|s| s.lines().count()) }
    });

    let table = summary_table(&doc);
    println!("Lightroom parity ({DOC}): {} rows, {} references\n", doc.rows.len(), doc.refs.len());
    print!("{table}");
    if write {
        let new = with_summary(&md, &table).ok_or_else(|| format!("{DOC}: summary markers `{BEGIN}` … `{END}` not found"))?;
        if new != md {
            std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("\nupdated the summary in {DOC}");
        }
    } else if with_summary(&md, &table).is_some_and(|new| new != md) {
        println!("\nnote: the summary in {DOC} is stale — run `cargo xtask parity --write`");
    }
    // Ids from the local Lightroom reference that have no row yet (plan/ is local-only, so only a warning).
    let catalog = root.join("plan/lightroom/03-feature-catalog.md");
    if let Ok(cat) = std::fs::read_to_string(&catalog) {
        let have: HashSet<&str> = doc.rows.iter().map(|r| r.id.as_str()).collect();
        let missing: Vec<&str> = cat
            .lines()
            .filter_map(|l| l.trim().strip_prefix("| ")?.split(['|', ' ']).next())
            .filter(|id| id.starts_with("LR-") && !have.contains(id))
            .collect();
        if !missing.is_empty() {
            println!("\nwarning: catalog ids without a row: {}", missing.join(", "));
        }
    }
    if problems.is_empty() {
        println!("\nOK: every command, control and path referenced in {DOC} exists.");
        Ok(())
    } else {
        println!();
        for p in &problems {
            println!("  - {p}");
        }
        Err(format!("{} stale reference(s) in {DOC}", problems.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Tracker

Prose mentioning `cmd:edit.undo`, placeholders `cmd:<id>` / `ctl:` and `plan/lightroom/03.md` (not checked) and `rating:3`.

<!-- parity:summary -->
old
<!-- /parity:summary -->

<!--
| LR-IGNORED-ROW | in a comment | P0 | ✅ | `cmd:nope` | |
-->

## A. Import (IMP)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-IMP-ONE | One | P0 | ✅ | `cmd:library.import`, `crates/engine/src/import.rs:12` | |
| LR-IMP-TWO | Two | P1 | 🟡 | `ctl:light.*` | partial |
| LR-IMP-THREE | Three | OOS | 🚫 | | |

| Action | Ours | Theirs |
|---|---|---|
| Pick flag | P | Z |

## B. Library (LIB)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-LIB-ONE | One | P0 | ⬜ | `cmd:album.nope` | |
| LR-LIB-ONE | Dup | P1 | done | | |
| KEY-SHORT | too | few |
";

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_rows_sections_and_statuses() {
        let d = parse(SAMPLE);
        let ids: Vec<&str> = d.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["LR-IMP-ONE", "LR-IMP-TWO", "LR-IMP-THREE", "LR-LIB-ONE"]);
        assert_eq!(d.rows[0].section, "A. Import (IMP)");
        assert_eq!(d.rows[3].section, "B. Library (LIB)");
        assert_eq!(d.rows[1].status, Status::Partial);
        assert_eq!(d.rows[2].status, Status::OutOfScope);
        assert_eq!(d.rows[1].tier, "P1");
        // the bad-status duplicate and the short row are errors; the comment row is ignored
        assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
        assert!(d.errors.iter().any(|e| e.contains("status `done`")));
        assert!(d.errors.iter().any(|e| e.contains("KEY-SHORT")));
    }

    #[test]
    fn collects_references_everywhere() {
        let d = parse(SAMPLE);
        let refs: Vec<&Ref> = d.refs.iter().map(|(_, r)| r).collect();
        assert!(refs.contains(&&Ref::Cmd("edit.undo".into())));
        assert!(refs.contains(&&Ref::Path("crates/engine/src/import.rs".into(), Some(12))));
        assert!(refs.contains(&&Ref::Ctl("light.*".into())));
        assert!(!refs.iter().any(|r| matches!(r, Ref::Cmd(id) if id == "nope")), "comment rows are skipped");
        assert!(!refs.iter().any(|r| matches!(r, Ref::Path(p, _) if p.starts_with("plan/"))));
        assert_eq!(refs.len(), 5);
    }

    #[test]
    fn check_reports_unknown_ids_and_paths() {
        let d = parse(SAMPLE);
        let cmds = set(&["edit.undo", "library.import"]);
        let ctls = set(&["light.exposure"]);
        let problems = check(&d, &cmds, &ctls, |p| (p == "crates/engine/src/import.rs").then_some(100));
        let unknown: Vec<&String> = problems.iter().filter(|p| p.contains("unknown")).collect();
        assert_eq!(unknown.len(), 1, "{problems:?}");
        assert!(unknown[0].contains("cmd:album.nope"));
        // a line past the end of the file is stale too
        let short = check(&d, &cmds, &ctls, |_| Some(5));
        assert!(short.iter().any(|p| p.contains("import.rs:12")));
        // missing files and unknown controls
        let none = check(&d, &cmds, &set(&[]), |_| None);
        assert!(none.iter().any(|p| p.contains("missing path")));
        assert!(none.iter().any(|p| p.contains("ctl:light.*")));
    }

    #[test]
    fn summary_counts_and_rewrites_between_markers() {
        let d = parse(SAMPLE);
        let (sections, total) = summarize(&d);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].1, Counts { done: 1, partial: 1, missing: 0, oos: 1, p0: (1, 1), p1: (0, 1) });
        assert_eq!(total.p0, (1, 2));
        let table = summary_table(&d);
        assert!(table.contains("| A. Import (IMP) | 1 | 1 | 0 | 1 | 1/1 (100%) | 0/1 (0%) |"), "{table}");
        assert!(table.contains("| **Total** | 1 | 1 | 1 | 1 | 1/2 (50%) | 0/1 (0%) |"), "{table}");
        // three in-scope rows: 1 + ½ + 0
        assert_eq!(weighted(&d, None), (50.0, 3));
        assert_eq!(weighted(&d, Some("P0")), (50.0, 2));
        assert!(table.contains("**50.0%** of 3 in-scope rows — P0 50.0% of 2 · P1 50.0% of 1."), "{table}");
        let new = with_summary(SAMPLE, &table).unwrap();
        assert!(!new.contains("\nold\n"));
        assert!(new.contains(&format!("{BEGIN}\n{table}{END}")));
        assert_eq!(with_summary(&new, &table).unwrap(), new, "idempotent");
    }

    #[test]
    fn reads_ui_command_ids() {
        let src = "pub const UI_COMMANDS: &[UiCommand] = &[\n    (\"view.detail\", \"Detail\", Some(\"D\"), \"View\"),\n    (\"app.about\", \"About\", None, \"\"),\n];\nfn x() { (\"not.this\", 1); }";
        assert_eq!(ui_command_ids(src), ["view.detail", "app.about"]);
    }
}
