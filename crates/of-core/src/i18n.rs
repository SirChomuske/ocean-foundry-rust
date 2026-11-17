//! Localisation. Every string shown to the user comes from a language table
//! embedded in the executable, so the app needs no files beside it to speak
//! either language.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::ZhCn];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::ZhCn => "zh-CN",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let c = code.to_ascii_lowercase();
        if c == "en" || c.starts_with("en-") {
            Some(Lang::En)
        } else if c == "zh" || c.starts_with("zh-") || c == "cn" {
            Some(Lang::ZhCn)
        } else {
            None
        }
    }

    /// Name of the language in that language, for the picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::ZhCn => "简体中文",
        }
    }

    fn source(self) -> &'static str {
        match self {
            // Written by build.rs: the package leaves out lava's strings
            // and those of the coast and cliffs lands.
            Lang::En => include_str!(concat!(env!("OUT_DIR"), "/en.json")),
            Lang::ZhCn => include_str!(concat!(env!("OUT_DIR"), "/zh-CN.json")),
        }
    }
}

pub type Table = BTreeMap<String, String>;

pub fn load_table(lang: Lang) -> Table {
    serde_json::from_str(lang.source()).unwrap_or_else(|e| panic!("language table {} is invalid: {e}", lang.code()))
}

#[derive(Debug, Clone)]
pub struct I18n {
    lang: Lang,
    table: Table,
    fallback: Table,
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        Self { lang, table: load_table(lang), fallback: load_table(Lang::En) }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) {
        if lang != self.lang {
            self.lang = lang;
            self.table = load_table(lang);
        }
    }

    /// Look up a string. Falls back to English, then to the key itself, so a
    /// missing translation is visible instead of blank.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.table.get(key).or_else(|| self.fallback.get(key)).map(String::as_str).unwrap_or(key)
    }

    /// Look up a string and substitute `{name}` placeholders.
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut s = self.t(key).to_string();
        for (name, value) in args {
            s = s.replace(&format!("{{{name}}}"), value);
        }
        s
    }

    pub fn has(&self, key: &str) -> bool {
        self.table.contains_key(key)
    }
}

/// Every key the parameter registry needs: labels, help, groups and options.
pub fn registry_keys() -> Vec<String> {
    use crate::params::{Kind, Mode, Stage, REGISTRY};
    let mut keys = Vec::new();
    for p in REGISTRY {
        keys.push(p.label_key());
        keys.push(p.help_key());
        keys.push(format!("group.{}", p.group));
        if let Kind::Choice { options, .. } = p.kind {
            keys.extend(options.iter().map(|o| p.option_key(o)));
        }
    }
    keys.extend(Mode::ALL.iter().map(|m| format!("mode.{}", m.key())));
    keys.extend(Stage::ALL.iter().map(|s| format!("stage.{}", s.key())));
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_define_exactly_the_same_keys() {
        let en = load_table(Lang::En);
        let zh = load_table(Lang::ZhCn);
        let missing_zh: Vec<_> = en.keys().filter(|k| !zh.contains_key(*k)).collect();
        let missing_en: Vec<_> = zh.keys().filter(|k| !en.contains_key(*k)).collect();
        assert!(missing_zh.is_empty(), "missing in zh-CN: {missing_zh:?}");
        assert!(missing_en.is_empty(), "missing in en: {missing_en:?}");
    }

    #[test]
    fn no_string_is_empty_and_placeholders_match() {
        let en = load_table(Lang::En);
        let zh = load_table(Lang::ZhCn);
        let placeholders = |s: &str| {
            let mut v: Vec<String> = s
                .split('{')
                .skip(1)
                .filter_map(|part| part.split_once('}').map(|(name, _)| name.to_string()))
                .collect();
            v.sort();
            v
        };
        for (k, e) in &en {
            let z = &zh[k];
            assert!(!e.trim().is_empty(), "empty en string {k}");
            assert!(!z.trim().is_empty(), "empty zh-CN string {k}");
            assert_eq!(placeholders(e), placeholders(z), "placeholders differ for {k}");
        }
    }

    #[test]
    fn every_parameter_has_label_help_group_and_options() {
        for lang in Lang::ALL {
            let table = load_table(lang);
            let missing: Vec<_> = registry_keys().into_iter().filter(|k| !table.contains_key(k)).collect();
            assert!(missing.is_empty(), "{} lacks {missing:?}", lang.code());
        }
    }

    #[test]
    fn chinese_table_is_actually_translated() {
        let en = load_table(Lang::En);
        let zh = load_table(Lang::ZhCn);
        for (k, z) in &zh {
            if k.ends_with(".help") {
                assert_ne!(z, &en[k], "help text {k} is not translated");
                assert!(z.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "{k} has no Chinese text");
            }
        }
    }

    #[test]
    fn missing_key_is_shown_not_hidden() {
        let i = I18n::new(Lang::ZhCn);
        assert_eq!(i.t("no.such.key"), "no.such.key");
        assert_eq!(i.tf("job.eta", &[("time", "5 s")]).contains("5 s"), true);
    }
}
