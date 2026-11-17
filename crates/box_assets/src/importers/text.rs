//! The `font` (`.ttf`, `.otf`, `.ttc` → [`FontAsset`]), `strings`
//! (`.strings.json` → [`StringTable`]) and `data` (`.data.json` →
//! [`DataTable`]) importers.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value as Json;

use crate::error::ImportError;
use crate::importer::{ImportContext, Importer};

/// A TrueType or OpenType font file (the first face of a collection), as
/// the in-game UI's theme references it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontAsset {
    /// The family name from the font's `name` table (the file name when it
    /// has none).
    pub family: String,
    /// The file's bytes, shared.
    pub bytes: Arc<Vec<u8>>,
}

/// `.ttf` / `.otf` / `.ttc` → [`FontAsset`]. The file is parsed once to
/// check that it is a usable font and to read its family name.
pub struct FontImporter;

impl Importer for FontImporter {
    fn name(&self) -> &str {
        "font"
    }

    fn extensions(&self) -> &[&str] {
        &["ttf", "otf", "ttc"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let bytes = ctx.bytes().to_vec();
        let family = {
            let face = ttf_parser::Face::parse(&bytes, 0)
                .map_err(|e| ImportError::Decode(format!("not a usable font: {e}")))?;
            if face.number_of_glyphs() == 0 {
                return Err(ImportError::Decode("the font has no glyphs".into()));
            }
            face.names()
                .into_iter()
                .filter(|n| {
                    n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
                        || n.name_id == ttf_parser::name_id::FAMILY
                })
                .find_map(|n| n.to_string())
        };
        let family = family.unwrap_or_else(|| {
            let file = ctx.path().rsplit('/').next().unwrap_or(ctx.path());
            file.rsplit_once('.').map_or(file, |(s, _)| s).to_string()
        });
        ctx.add(
            None,
            FontAsset {
                family,
                bytes: Arc::new(bytes),
            },
        );
        Ok(())
    }
}

/// A string table: text by key for one language.
///
/// File: `<name>.<language>.strings.json` or `<language>.strings.json`
/// (`strings/en.strings.json`, `strings/dialogue.de.strings.json`). The
/// content is a JSON object whose values are strings or nested objects;
/// nested keys are joined with dots:
///
/// ```json
/// { "menu": { "start": "Start", "quit": "Quit" }, "hud.coins": "Coins: {count}" }
/// ```
///
/// gives `menu.start`, `menu.quit` and `hud.coins`. `{name}` placeholders
/// are filled in by the reader (`box_ui::Strings`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StringTable {
    /// From the file name.
    pub language: String,
    pub strings: BTreeMap<String, String>,
}

impl StringTable {
    /// The language a string table file is for, from its name: the segment
    /// before `.strings.json` (`dialogue.de.strings.json` → `de`).
    pub fn language_of(path: &str) -> Option<&str> {
        let file = path.rsplit('/').next()?;
        let stem = file.strip_suffix(".strings.json")?;
        let lang = stem.rsplit('.').next()?;
        (!lang.is_empty()).then_some(lang)
    }

    /// Parses a string table's JSON.
    pub fn parse(language: &str, bytes: &[u8]) -> Result<Self, String> {
        let json: Json = serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        let Json::Object(map) = json else {
            return Err("a string table must be a JSON object".into());
        };
        let mut strings = BTreeMap::new();
        flatten("", &map, &mut strings)?;
        Ok(Self {
            language: language.to_string(),
            strings,
        })
    }
}

fn flatten(
    prefix: &str,
    map: &serde_json::Map<String, Json>,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for (k, v) in map {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Json::String(s) => {
                out.insert(key, s.clone());
            }
            Json::Object(inner) => flatten(&key, inner, out)?,
            other => {
                return Err(format!(
                    "`{key}`: expected a string or an object, got {other}"
                ));
            }
        }
    }
    Ok(())
}

/// `.strings.json` → [`StringTable`].
pub struct StringTableImporter;

impl Importer for StringTableImporter {
    fn name(&self) -> &str {
        "strings"
    }

    fn extensions(&self) -> &[&str] {
        &["strings.json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let language = StringTable::language_of(ctx.path())
            .ok_or_else(|| {
                ImportError::Decode(
                    "name the file <language>.strings.json (for example en.strings.json)".into(),
                )
            })?
            .to_string();
        let table = StringTable::parse(&language, ctx.bytes()).map_err(ImportError::Decode)?;
        ctx.add(None, table);
        Ok(())
    }
}

/// A data table: any JSON (`.data.json`), with helpers for the common
/// shapes. Games read it typed with [`deserialize`](Self::deserialize) /
/// [`row_as`](Self::row_as) or untyped with [`get`](Self::get) and
/// [`rows`](Self::rows).
///
/// Rows are either an array of objects (each row's id is its `"id"` field,
/// else its index) or an object of objects (the key is the id):
///
/// ```json
/// [ { "id": "slime", "hp": 3 }, { "id": "bat", "hp": 1 } ]
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DataTable(pub Json);

impl DataTable {
    /// The whole JSON value.
    pub fn json(&self) -> &Json {
        &self.0
    }

    /// The value at a dotted path: object keys and array indices
    /// (`"waves.0.enemies"`). The empty path is the whole value.
    pub fn get(&self, path: &str) -> Option<&Json> {
        if path.is_empty() {
            return Some(&self.0);
        }
        path.split('.').try_fold(&self.0, |v, seg| match v {
            Json::Object(m) => m.get(seg),
            Json::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        })
    }

    /// The rows with their ids, in file order (empty when the value is
    /// neither an array nor an object).
    pub fn rows(&self) -> Vec<(String, &Json)> {
        match &self.0 {
            Json::Array(a) => a
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let id = match r.get("id") {
                        Some(Json::String(s)) => s.clone(),
                        Some(Json::Number(n)) => n.to_string(),
                        _ => i.to_string(),
                    };
                    (id, r)
                })
                .collect(),
            Json::Object(m) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
            _ => Vec::new(),
        }
    }

    /// The row with this id.
    pub fn row(&self, id: &str) -> Option<&Json> {
        match &self.0 {
            Json::Object(m) => m.get(id),
            _ => self
                .rows()
                .into_iter()
                .find(|(i, _)| i == id)
                .map(|(_, r)| r),
        }
    }

    /// The whole table as `T`.
    pub fn deserialize<T: DeserializeOwned>(&self) -> Result<T, String> {
        T::deserialize(&self.0).map_err(|e| e.to_string())
    }

    /// One row as `T`.
    pub fn row_as<T: DeserializeOwned>(&self, id: &str) -> Result<T, String> {
        let row = self.row(id).ok_or_else(|| format!("no row `{id}`"))?;
        T::deserialize(row).map_err(|e| format!("row `{id}`: {e}"))
    }
}

/// `.data.json` → [`DataTable`]. Only the syntax is checked.
pub struct DataTableImporter;

impl Importer for DataTableImporter {
    fn name(&self) -> &str {
        "data"
    }

    fn extensions(&self) -> &[&str] {
        &["data.json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let json: Json = serde_json::from_slice(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("invalid JSON: {e}")))?;
        ctx.add(None, DataTable(json));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_tables_flatten_and_take_the_language_from_the_name() {
        assert_eq!(
            StringTable::language_of("strings/en.strings.json"),
            Some("en")
        );
        assert_eq!(
            StringTable::language_of("strings/dialogue.de.strings.json"),
            Some("de")
        );
        assert_eq!(StringTable::language_of("strings/x.json"), None);
        let t = StringTable::parse("en", br#"{"menu": {"start": "Start"}, "a.b": "c"}"#).unwrap();
        assert_eq!(t.strings["menu.start"], "Start");
        assert_eq!(t.strings["a.b"], "c");
        let e = StringTable::parse("en", br#"{"menu": {"start": 3}}"#).unwrap_err();
        assert!(e.contains("menu.start"), "{e}");
    }

    #[test]
    fn data_tables_read_rows_and_paths() {
        let t = DataTable(serde_json::json!([
            {"id": "slime", "hp": 3},
            {"hp": 1}
        ]));
        assert_eq!(t.get("0.hp"), Some(&serde_json::json!(3)));
        assert_eq!(
            t.row("slime").and_then(|r| r.get("hp")),
            Some(&serde_json::json!(3))
        );
        assert_eq!(t.rows()[1].0, "1");
        #[derive(serde::Deserialize)]
        struct Enemy {
            hp: u32,
        }
        assert_eq!(t.row_as::<Enemy>("slime").unwrap().hp, 3);
        assert!(t.row_as::<Enemy>("nope").is_err());
        let m = DataTable(serde_json::json!({"bat": {"hp": 1}}));
        assert_eq!(m.rows()[0].0, "bat");
        assert!(m.row("bat").is_some());
    }
}
