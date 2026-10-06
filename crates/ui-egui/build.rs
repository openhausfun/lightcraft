//! Compile translated format strings so Rust checks both languages' placeholders.
use std::{collections::BTreeMap, env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=locales/ja-formats.json");
    let messages: BTreeMap<String, String> = serde_json::from_str(&fs::read_to_string("locales/ja-formats.json")?)?;
    let mut source = String::from("macro_rules! tr_format {\n");
    for (english, japanese) in messages {
        let en = serde_json::to_string(&english)?;
        let ja = serde_json::to_string(&japanese)?;
        source.push_str(&format!(
            "({en} $(, $($args:tt)*)?) => {{ if $crate::i18n::is_japanese() {{ format!({ja} $(, $($args)*)?) }} else {{ format!({en} $(, $($args)*)?) }} }};\n"
        ));
    }
    source.push_str("}\npub(crate) use tr_format;\n");
    fs::write(PathBuf::from(env::var("OUT_DIR")?).join("ja-formats.rs"), source)?;
    Ok(())
}
