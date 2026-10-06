//! `cargo xtask bench [FILE] [--strict] [--threshold PCT]`: run the end-to-end render benchmark, record
//! the results in `target/bench/history.jsonl` (one JSON object per run, with git revision and load
//! average) and compare with the previous run.
//!
//! Comparisons use **process CPU time**, which reflects the work done even on a busy shared machine;
//! wall-clock is recorded for reference. `--strict` exits non-zero when a CPU metric regressed by more
//! than the threshold (default 20 %).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Parse `render_bench` output lines such as
/// `loupe draft 1152×768 cold:  cpu: 379.5 ms wall, 139.5 ms cpu  |  gpu: 19.9 ms wall, 17.2 ms cpu`
/// or `export JPEG encode:  63.9 ms wall, 278.0 ms cpu` into `name/backend/{wall,cpu}` → ms.
pub fn parse(output: &str) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for line in output.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if !rest.contains(" ms ") {
            continue;
        }
        let name = name.trim();
        for seg in rest.split('|') {
            let seg = seg.trim();
            let (backend, body) = match seg.split_once(':') {
                Some((b, body)) if matches!(b.trim(), "cpu" | "gpu") => (b.trim(), body),
                _ => ("cpu", seg),
            };
            for part in body.split(',') {
                let toks: Vec<&str> = part.split_whitespace().collect();
                if let [v, "ms", kind] = toks[..]
                    && let Ok(v) = v.parse::<f64>()
                    && matches!(kind, "wall" | "cpu")
                {
                    out.insert(format!("{name}/{backend}/{kind}"), v);
                }
            }
        }
    }
    out
}

/// CPU-time metrics that got slower than `threshold` (fraction) vs `prev`: (key, prev, now).
pub fn regressions(prev: &BTreeMap<String, f64>, now: &BTreeMap<String, f64>, threshold: f64) -> Vec<(String, f64, f64)> {
    now.iter()
        .filter(|(k, _)| k.ends_with("/cpu"))
        .filter_map(|(k, &v)| prev.get(k).map(|&p| (k.clone(), p, v)))
        .filter(|(_, p, v)| *p > 1.0 && *v > p * (1.0 + threshold))
        .collect()
}

fn load_avg() -> Option<f64> {
    let out = Command::new("uptime").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let tail = s.rsplit_once("load average")?.1;
    tail.trim_start_matches(['s', ':', ' ']).split([',', ' ']).find(|t| !t.is_empty())?.parse().ok()
}

fn git_rev(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let strict = args.contains(&"--strict");
    let threshold =
        args.iter().position(|a| *a == "--threshold").and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(20.0) / 100.0;
    let file = args.iter().find(|a| !a.starts_with("--") && a.parse::<f64>().is_err()).map(|s| s.to_string());
    let file = file.or_else(|| {
        let default = root.join("corpus/raw/arw-sony-a7m3-compressed.arw");
        default.exists().then(|| default.to_string_lossy().to_string())
    });
    let mut cmd = crate::cargo();
    cmd.args(["run", "--release", "-q", "-p", "lightcraft-engine", "--example", "render_bench", "--"]);
    if let Some(f) = &file {
        cmd.arg(f);
    }
    eprintln!("$ cargo run --release -p lightcraft-engine --example render_bench -- {}", file.as_deref().unwrap_or("(procedural 24 MP source)"));
    let out = cmd.output().map_err(|e| format!("render_bench: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    print!("{text}");
    if !out.status.success() {
        return Err(format!("render_bench failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let metrics = parse(&text);
    if metrics.is_empty() {
        return Err("no metrics parsed from render_bench output".into());
    }
    let dir = root.join("target/bench");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let hist = dir.join("history.jsonl");
    let prev: Option<BTreeMap<String, f64>> = std::fs::read_to_string(&hist)
        .ok()
        .and_then(|s| s.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string))
        .and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
        .and_then(|v| serde_json::from_value(v["metrics"].clone()).ok());
    let rec = serde_json::json!({
        "rev": git_rev(root),
        "time": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "load": load_avg(),
        "input": file,
        "metrics": metrics,
    });
    let mut line = rec.to_string();
    line.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new().create(true).append(true).open(&hist).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| e.to_string())?;
    eprintln!("bench: {} metrics recorded in {}", metrics.len(), hist.display());
    let Some(prev) = prev else {
        eprintln!("bench: first run — nothing to compare");
        return Ok(());
    };
    let reg = regressions(&prev, &metrics, threshold);
    if reg.is_empty() {
        eprintln!("bench: no CPU-time regressions > {:.0}% vs previous run", threshold * 100.0);
        return Ok(());
    }
    for (k, p, v) in &reg {
        eprintln!("bench: REGRESSION {k}: {p:.1} → {v:.1} ms (+{:.0}%)", (v / p - 1.0) * 100.0);
    }
    if strict { Err(format!("{} CPU-time regression(s)", reg.len())) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "gpu: Apple M4 Pro (Metal) (device + kernels: 322 ms)
gpu vs cpu, loupe 1920×1280 typical: max 1 LSB, mean 0.0001 LSB
loupe draft 1152×768 cold:             cpu: 379.5 ms wall, 139.5 ms cpu  |  gpu: 19.9 ms wall, 17.2 ms cpu
export JPEG encode:                    63.9 ms wall, 278.0 ms cpu
";

    #[test]
    fn parses_cpu_gpu_and_plain_lines() {
        let m = parse(OUT);
        assert_eq!(m["loupe draft 1152×768 cold/cpu/wall"], 379.5);
        assert_eq!(m["loupe draft 1152×768 cold/gpu/cpu"], 17.2);
        assert_eq!(m["export JPEG encode/cpu/cpu"], 278.0);
        assert_eq!(m.len(), 6, "{m:?}");
    }

    #[test]
    fn flags_only_cpu_time_regressions_over_threshold() {
        let prev = parse(OUT);
        let mut now = prev.clone();
        *now.get_mut("export JPEG encode/cpu/cpu").unwrap() = 400.0; // +44 %
        *now.get_mut("export JPEG encode/cpu/wall").unwrap() = 400.0; // wall ignored
        *now.get_mut("loupe draft 1152×768 cold/gpu/cpu").unwrap() = 18.0; // +5 %
        let r = regressions(&prev, &now, 0.2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "export JPEG encode/cpu/cpu");
    }
}
