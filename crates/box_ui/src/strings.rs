//! String tables: text by key, per language, with `{name}` placeholders.
//!
//! The [`Strings`] resource holds the active language (default `"en"`) and
//! a fallback language (default `"en"`). With an asset server, every
//! `*.strings.json` file of those two languages is loaded
//! ([`StringTable`]; the language comes from the file name,
//! `strings/de.strings.json` or `strings/dialogue.de.strings.json`) and
//! merged; files of one language are merged in path order. Hot reload
//! replaces a table's strings; adding a file is picked up when the asset
//! index changes.
//!
//! Lookup: the active language, then the fallback, then the key itself,
//! with a warning logged once per missing key (`box_ui` target, so the
//! editor shows it). Code can add strings directly with
//! [`Strings::insert`] (tests, generated text).

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use box_assets::{AssetServer, StringTable};
use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::world::World;

/// Resource: the string tables and the active language. See the module docs.
#[derive(Debug)]
pub struct Strings {
    language: String,
    fallback: String,
    /// Merged strings by language (assets, then code inserts on top).
    merged: BTreeMap<String, BTreeMap<String, String>>,
    /// Strings inserted by code, by language.
    inserted: BTreeMap<String, BTreeMap<String, String>>,
    /// Loaded table files: path, language, handle, version merged.
    files: Vec<(String, String, Handle<StringTable>, Option<u64>)>,
    /// (index generation, languages) the file list was built for.
    scanned: Option<(u64, String, String)>,
    revision: u64,
    warned: Mutex<HashSet<String>>,
}

impl Default for Strings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            fallback: "en".into(),
            merged: BTreeMap::new(),
            inserted: BTreeMap::new(),
            files: Vec::new(),
            scanned: None,
            revision: 1,
            warned: Mutex::new(HashSet::new()),
        }
    }
}

impl Strings {
    pub fn new(language: &str) -> Self {
        Self {
            language: language.into(),
            ..Default::default()
        }
    }

    /// The active language code (`"en"`).
    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn fallback_language(&self) -> &str {
        &self.fallback
    }

    /// Switches language; its tables load on the next frame (with an asset
    /// server) and UI text updates.
    pub fn set_language(&mut self, language: &str) {
        if self.language != language {
            self.language = language.to_string();
            self.bump();
        }
    }

    pub fn set_fallback_language(&mut self, language: &str) {
        if self.fallback != language {
            self.fallback = language.to_string();
            self.bump();
        }
    }

    /// Changes whenever any lookup result may have changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision += 1;
        if let Ok(mut w) = self.warned.lock() {
            w.clear();
        }
    }

    /// Adds or replaces one string (code-defined strings win over files).
    pub fn insert(&mut self, language: &str, key: &str, text: &str) {
        self.inserted
            .entry(language.to_string())
            .or_default()
            .insert(key.to_string(), text.to_string());
        self.merged
            .entry(language.to_string())
            .or_default()
            .insert(key.to_string(), text.to_string());
        self.bump();
    }

    /// The raw text for `key` (active language, then fallback), without
    /// warning.
    pub fn lookup(&self, key: &str) -> Option<&str> {
        [&self.language, &self.fallback]
            .into_iter()
            .find_map(|l| self.merged.get(l.as_str())?.get(key))
            .map(String::as_str)
    }

    /// True if `key` has text in the active or fallback language.
    pub fn has(&self, key: &str) -> bool {
        self.lookup(key).is_some()
    }

    /// The text for `key`; the key itself when missing (warned once).
    pub fn get(&self, key: &str) -> String {
        match self.lookup(key) {
            Some(t) => t.to_string(),
            None => {
                self.warn_missing(key);
                key.to_string()
            }
        }
    }

    /// The text for `key` with `{name}` placeholders filled from `args`.
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        fill(&self.get(key), |name| {
            args.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        })
    }

    fn warn_missing(&self, key: &str) {
        let Ok(mut w) = self.warned.lock() else {
            return;
        };
        if w.insert(key.to_string()) {
            tracing::warn!(
                target: "box_ui",
                "missing string `{key}` (language `{}`, fallback `{}`)",
                self.language,
                self.fallback
            );
        }
    }

    /// Keys looked up but missing in both languages since the last language
    /// change or reload (each was warned about once), sorted.
    pub fn missing_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .warned
            .lock()
            .map(|w| w.iter().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        keys
    }

    /// Every key of a language (merged), sorted.
    pub fn keys(&self, language: &str) -> Vec<&str> {
        self.merged
            .get(language)
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn remerge(&mut self, tables: &Assets<StringTable>) {
        let mut merged: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (_, lang, handle, _) in &self.files {
            if let Some(t) = tables.get(*handle) {
                merged
                    .entry(lang.clone())
                    .or_default()
                    .extend(t.strings.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        for (lang, m) in &self.inserted {
            merged
                .entry(lang.clone())
                .or_default()
                .extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.merged = merged;
        self.bump();
    }
}

/// Fills `{name}` placeholders through `arg`; `{{` and `}}` are literal
/// braces; unknown names stay as written.
pub fn fill(template: &str, arg: impl Fn(&str) -> Option<String>) -> String {
    if !template.contains(['{', '}']) {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    loop {
        let Some(i) = rest.find(['{', '}']) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let name = &tail[1..end];
            if !name.is_empty() && !name.contains(['{', ' ']) {
                match arg(name) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&tail[..=end]),
                }
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out
}

/// Loads the tables of the active and fallback languages and merges
/// changed ones (UiPlugin, Update). Idle cost: a few integer compares.
pub fn update_strings(world: &mut World) {
    let Some(server) = box_assets::loader(world) else {
        return;
    };
    let Some(mut strings) = world.remove_resource::<Strings>() else {
        return;
    };
    let generation = server.index_generation();
    let key = (
        generation,
        strings.language.clone(),
        strings.fallback.clone(),
    );
    if strings.scanned.as_ref() != Some(&key) {
        rescan(&mut strings, &server);
        strings.scanned = Some(key);
    }
    let mut changed = false;
    if let Some(tables) = world.get_resource::<Assets<StringTable>>() {
        for (_, _, handle, merged) in &mut strings.files {
            let v = tables.version(*handle);
            if v.is_some() && v != *merged {
                *merged = v;
                changed = true;
            }
        }
        if changed {
            strings.remerge(tables);
        }
    }
    world.insert_resource(strings);
}

fn rescan(strings: &mut Strings, server: &AssetServer) {
    let langs = [strings.language.clone(), strings.fallback.clone()];
    let mut wanted: Vec<(String, String)> = server
        .files()
        .into_iter()
        .filter_map(|(_, path)| {
            let lang = StringTable::language_of(&path)?.to_string();
            langs.contains(&lang).then_some((path, lang))
        })
        .collect();
    wanted.sort();
    let mut files = Vec::with_capacity(wanted.len());
    for (path, lang) in wanted {
        if let Some(existing) = strings.files.iter().find(|f| f.0 == path) {
            files.push(existing.clone());
            continue;
        }
        match server.load::<StringTable>(&AssetRef::path(&path)) {
            Ok(h) => files.push((path, lang, h, None)),
            Err(e) => tracing::warn!(target: "box_ui", "string table {path}: {e}"),
        }
    }
    // Dropped files (other languages, deleted) leave the merge.
    let removed = strings.files.len() != files.len()
        || strings.files.iter().zip(&files).any(|(a, b)| a.0 != b.0);
    strings.files = files;
    if removed {
        for f in &mut strings.files {
            f.3 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_falls_back_and_fills_placeholders() {
        let mut s = Strings::default();
        s.insert("en", "hud.coins", "Coins: {count}");
        s.insert("en", "only.en", "English");
        s.insert("de", "hud.coins", "Münzen: {count}");
        s.set_language("de");
        assert_eq!(s.format("hud.coins", &[("count", "3")]), "Münzen: 3");
        assert_eq!(s.get("only.en"), "English", "fallback");
        assert_eq!(s.get("no.such.key"), "no.such.key");
        assert!(!s.has("no.such.key"));
    }

    #[test]
    fn placeholders_escape_and_keep_unknown_names() {
        let f = |t: &str| fill(t, |n| (n == "a").then(|| "1".to_string()));
        assert_eq!(f("{a}+{b}"), "1+{b}");
        assert_eq!(f("{{a}} {a}"), "{a} 1");
        assert_eq!(f("{ a }"), "{ a }");
        assert_eq!(f("tail {"), "tail {");
        assert_eq!(f("no braces"), "no braces");
    }
}
