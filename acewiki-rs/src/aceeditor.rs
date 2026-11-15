//! The ACE Editor application (`ch.uzh.ifi.attempto.aceeditor`): a predictive editor for
//! full ACE texts over a user lexicon file, with APE analysis of each text entry.

use crate::chartparser::{ChartParser, DynamicLexicon, Grammar, LexicalRule};
use crate::engine::{unescape, ACEEDITOR_GRAMMAR};
use ape_rs::Ape;
use std::collections::HashMap;
use std::sync::Arc;

/// One lexicon entry (`aceeditor.Word`).
#[derive(Clone, Debug, PartialEq)]
pub struct EditorWord {
    pub form: String,
    pub symbol: String,
    pub entry: String,
    pub cat: String,
    pub feats: Vec<(String, String)>,
}

impl EditorWord {
    /// Java `Word(String lexiconEntry)`; returns None for comments, blanks and invalid lines.
    pub fn parse(line: &str) -> Option<EditorWord> {
        let full = regex::Regex::new(r"^[a-z_]+\('?[A-Za-z0-9\-_]+'?,\s*'?[A-Za-z0-9\-_]+'?.*\)\.\s*$").unwrap();
        if !full.is_match(line) { return None; }
        let entry = regex::Regex::new(r"^(.*)\.\s*$").unwrap().replace(line, "$1").into_owned();
        let mut cat = regex::Regex::new(r"^([a-z_]+)\(.*$").unwrap().replace(line, "$1").into_owned();
        if cat == "pn_sg" { cat = "prop_sg".into(); }
        if cat == "pndef_sg" { cat = "propdef_sg".into(); }
        let form = regex::Regex::new(r"^[a-z_]+\('?([A-Za-z0-9\-_]+)'?,.*$").unwrap().replace(line, "$1").into_owned();
        let symbol = regex::Regex::new(r"^[a-z_]+\('?[A-Za-z0-9\-_]+'?,\s*'?([A-Za-z0-9\-_]+)'?.*$").unwrap().replace(line, "$1").into_owned();
        let rest = regex::Regex::new(r"^[a-z_]+\('?[A-Za-z0-9\-_]+'?,\s*'?[A-Za-z0-9\-_]+'?(.*)\)\.\s*$").unwrap().replace(line, "$1").into_owned();
        // readFeatures
        let mut f = regex::Regex::new(r"\s+").unwrap().replace_all(&rest, "").into_owned();
        f = f.strip_prefix(',').map(str::to_string).unwrap_or(f);
        f = f.strip_prefix('\'').map(str::to_string).unwrap_or(f);
        f = f.strip_suffix('\'').map(str::to_string).unwrap_or(f);
        f = f.replace(",'", "").replace("',", "");
        let s: Vec<&str> = f.split(',').collect();
        let s0 = s.first().copied().unwrap_or("");
        let mut feats: Vec<(String, String)> = vec![];
        match cat.as_str() {
            "adj_tr" | "adj_tr_comp" => feats.push(("prep".into(), s0.into())),
            "noun_sg" | "noun_pl" | "prop_sg" | "propdef_sg" => match s0 {
                "masc" | "fem" => { feats.push(("human".into(), "plus".into())); feats.push(("gender".into(), s0.into())); }
                "human" => feats.push(("human".into(), "plus".into())),
                "neutr" => feats.push(("human".into(), "minus".into())),
                _ => {}
            },
            _ => {}
        }
        feats.push(("text".into(), form.clone()));
        feats.sort();
        Some(EditorWord { form, symbol, entry, cat, feats })
    }
    pub fn token_text(&self) -> String { if self.cat == "propdef_sg" { format!("the {}", self.form) } else { self.form.clone() } }
    pub fn lexical_rule(&self) -> LexicalRule {
        let f: Vec<(&str, &str)> = self.feats.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        LexicalRule::new(&self.cat, &self.form, &f)
    }
    pub fn feat(&self, n: &str) -> Option<&str> { self.feats.iter().find(|x| x.0 == n).map(|x| x.1.as_str()) }
}

/// `aceeditor.LexiconHandler`.
#[derive(Clone, Debug, Default)]
pub struct EditorLexicon {
    pub words: Vec<EditorWord>,
    by_cat: HashMap<String, Vec<usize>>,
    by_text: HashMap<String, Vec<usize>>,
}

impl EditorLexicon {
    pub fn parse(src: &str) -> Self {
        let mut l = EditorLexicon::default();
        for line in src.lines() { l.add_line(line); }
        l
    }
    pub fn add_line(&mut self, line: &str) -> bool {
        match EditorWord::parse(line) { Some(w) => { self.add(w); true } None => false }
    }
    pub fn add(&mut self, w: EditorWord) {
        if self.words.iter().any(|x| x.entry == w.entry) { return; } // Lexicon is a set of entries
        let i = self.words.len();
        self.by_cat.entry(w.cat.clone()).or_default().push(i);
        self.by_text.entry(w.form.clone()).or_default().push(i);
        self.words.push(w);
    }
    pub fn by_category(&self, c: &str) -> impl Iterator<Item = &EditorWord> { self.by_cat.get(c).into_iter().flatten().map(|i| &self.words[*i]) }
    pub fn by_text(&self, t: &str) -> impl Iterator<Item = &EditorWord> { self.by_text.get(t).into_iter().flatten().map(|i| &self.words[*i]) }
    /// `getLexiconFileContent` (also what is sent to APE as `ulextext`).
    pub fn file_content(&self) -> String { self.words.iter().map(|w| format!("{}.\n", w.entry)).collect() }

    fn var_entries(out: &mut Vec<LexicalRule>, cat: &str) {
        for s in ["X", "Y", "Z"] {
            for i in 0..=2 {
                let t = if i == 0 { s.to_string() } else { format!("{s}{i}") };
                out.push(LexicalRule::new(cat, &t, &[("text", &t)]));
            }
        }
    }
    fn def_noun(w: &EditorWord) -> LexicalRule {
        let t = format!("the {}", w.form);
        let mut f = vec![("noun", w.form.as_str()), ("text", t.as_str())];
        f.sort();
        LexicalRule::new("def_noun_sg", &t, &f)
    }
}

impl DynamicLexicon for EditorLexicon {
    fn lex_rules_for_category(&self, cat: &str) -> Vec<LexicalRule> {
        let mut v = vec![];
        match cat {
            "def_noun_sg" => for w in self.by_category("noun_sg") { v.push(Self::def_noun(w)) },
            "var" | "ref" => Self::var_entries(&mut v, cat),
            "num" => for i in 2..100 { v.push(LexicalRule::new("num", &i.to_string(), &[])) },
            "adj_prep" => for w in self.by_category("adj_tr") {
                let p = w.feat("prep").unwrap_or("").to_string();
                v.push(LexicalRule::new("adj_prep", &p, &[("prep", &p)]));
            },
            _ => for w in self.by_category(cat) { v.push(w.lexical_rule()) },
        }
        v
    }
    fn lex_rules_for_word(&self, word: &str) -> Vec<LexicalRule> {
        let mut v = vec![];
        if let Some(rest) = word.strip_prefix("the ") {
            for w in self.by_text(rest) { if w.cat == "noun_sg" { v.push(Self::def_noun(w)); } }
        } else if regex::Regex::new(r"^[XYZ][0-9]*$").unwrap().is_match(word) {
            v.push(LexicalRule::new("var", word, &[("text", word)]));
            v.push(LexicalRule::new("ref", word, &[("text", word)]));
        } else if regex::Regex::new(r"^(?:[1-9][0-9]+|[2-9])$").unwrap().is_match(word) {
            v.push(LexicalRule::new("num", word, &[]));
        } else {
            for w in self.by_text(word) { v.push(w.lexical_rule()); }
        }
        v.push(LexicalRule::new("adj_prep", word, &[("prep", word)]));
        v
    }
}

/// Menu groups (`ACEEditorMenuCreator.cats` / `menuGroupOrdering`).
pub fn menu_group(cat: Option<&str>) -> &'static str {
    match cat {
        Some("noun_sg" | "noun_pl") => "noun",
        Some("prop_sg" | "propdef_sg") => "proper name",
        Some("iv_finsg" | "iv_infpl") => "intransitive verb",
        Some("tv_finsg" | "tv_infpl") => "transitive verb",
        Some("tv_pp") => "passive verb",
        Some("adj_itr" | "adj_itr_comp" | "adj_itr_sup") => "adjective",
        Some("adj_tr" | "adj_tr_comp") => "transitive adjective",
        Some("adv" | "adv_comp" | "adv_sup") => "adverb",
        Some("prep" | "adj_prep") => "preposition",
        Some("pron" | "def_noun_sg" | "ref") => "reference",
        Some("var") => "new variable",
        _ => "function word",
    }
}
pub const MENU_ORDER: &[&str] = &["function word", "proper name", "noun", "adjective", "transitive adjective", "intransitive verb",
    "transitive verb", "passive verb", "adverb", "preposition", "new variable", "reference"];
/// Extensible categories, i.e. those for which "new..." creates a lexicon entry.
pub const EXT_CATS: &[&str] = &["noun_sg", "noun_pl", "prop_sg", "propdef_sg", "iv_finsg", "iv_infpl", "tv_finsg", "tv_infpl", "tv_pp",
    "adj_itr", "adj_itr_comp", "adj_itr_sup", "adj_tr", "adj_tr_comp", "adv", "adv_comp", "adv_sup", "prep"];

/// One analysed text entry (`TextEntry`): the result items of the ACE Editor.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct EntryResult {
    pub text: String,
    pub outputs: Vec<(String, String)>,
    pub messages: Vec<(String, String, String)>,
    pub error: bool,
}

pub const OUTPUTS: &[(&str, &str)] = &[
    ("paraphrase", "Paraphrase"), ("sentences", "Sentences"), ("tokens", "Tokens"), ("syntax", "Syntax List"),
    ("syntaxpp", "Syntax Tree"), ("drs", "DRS"), ("drspp", "Pretty-Printed DRS"), ("drsxml", "DRS XML"),
    ("fol", "FOL"), ("pnf", "PNF"), ("owlfsspp", "OWL FSS"), ("owlxml", "OWL XML"),
];

pub struct AceEditor {
    tpl: std::cell::RefCell<Option<ChartParser>>,
    pub grammar: Arc<Grammar>,
    pub lexicon: EditorLexicon,
    pub lexicon_file: Option<std::path::PathBuf>,
    pub entries: Vec<String>,
}

impl AceEditor {
    pub fn new(lexicon_src: &str, lexicon_file: Option<std::path::PathBuf>) -> anyhow::Result<Self> {
        Ok(AceEditor { tpl: Default::default(), grammar: Arc::new(Grammar::from_json(ACEEDITOR_GRAMMAR)?), lexicon: EditorLexicon::parse(lexicon_src), lexicon_file, entries: vec![] })
    }
    pub fn parser(&self) -> ChartParser {
        let mut t = self.tpl.borrow_mut();
        if t.is_none() {
            let mut p = ChartParser::new(self.grammar.clone(), "text");
            p.set_dynamic_lexicon(Box::new(self.lexicon.clone()));
            p.set_rule_cache(Default::default());
            *t = Some(p);
        }
        t.as_ref().unwrap().clone()
    }
    /// Call after changing the lexicon.
    pub fn lexicon_changed(&self) { *self.tpl.borrow_mut() = None; }
    pub fn save_lexicon(&self) -> anyhow::Result<()> {
        if let Some(f) = &self.lexicon_file { std::fs::write(f, self.lexicon.file_content())?; }
        Ok(())
    }
    /// Join editor tokens into ACE text (capitalising sentence-initial function words).
    pub fn text_of(&self, tokens: &[String]) -> String {
        let mut s = String::new();
        let mut start = true;
        for t in tokens {
            let punct = t.len() == 1 && ".,:;?!".contains(t.as_str());
            if !s.is_empty() && !punct { s.push(' '); }
            let in_lex = self.lexicon.by_text(t).next().is_some();
            if start && !in_lex {
                let mut c = t.chars();
                if let Some(f) = c.next() { s += &(f.to_uppercase().collect::<String>() + c.as_str()); }
            } else { s += t; }
            start = t == "." || t == "?" || t == "!";
        }
        s
    }
    /// `TextEntry` analysis: APE multi-output with the editor lexicon.
    pub fn analyze(&self, ape: &mut Ape, text: &str) -> anyhow::Result<EntryResult> {
        let lex = self.lexicon.file_content();
        let mut params: Vec<(&str, &str)> = vec![("text", text), ("ulextext", &lex), ("uri", "http://attempto.ifi.uzh.ch/aceeditor/default"),
            ("noclex", "on"), ("guess", "off")];
        let names: Vec<String> = OUTPUTS.iter().map(|(k, _)| format!("c{k}")).collect();
        for n in &names { params.push((n.as_str(), "on")); }
        let r = ape.get_results(&params)?;
        let xml = r.content;
        let get = |tag: &str| -> String {
            let (o, c) = (format!("<{tag}>"), format!("</{tag}>"));
            match (xml.find(&o), xml.find(&c)) { (Some(i), Some(j)) if j > i => unescape(&xml[i + o.len()..j]), _ => String::new() }
        };
        let outputs: Vec<(String, String)> = OUTPUTS.iter().map(|(k, _)| (k.to_string(), get(k))).collect();
        let a = crate::engine::parse_ape_result(&xml);
        let drs = outputs.iter().find(|x| x.0 == "drs").map(|x| x.1.trim().to_string()).unwrap_or_default();
        Ok(EntryResult { text: text.to_string(), error: drs.is_empty() || drs == "drs([], [])", outputs, messages: a.messages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_parse_matches_java() {
        let w = EditorWord::parse("noun_sg(man, man, masc).").unwrap();
        assert_eq!((w.cat.as_str(), w.form.as_str(), w.symbol.as_str()), ("noun_sg", "man", "man"));
        assert_eq!(w.feat("gender"), Some("masc"));
        assert_eq!(w.feat("human"), Some("plus"));
        let a = EditorWord::parse("adj_tr('fond-of', 'fond-of', of).").unwrap();
        assert_eq!(a.feat("prep"), Some("of"));
        let p = EditorWord::parse("pndef_sg('Nile', 'Nile', neutr).").unwrap();
        assert_eq!(p.cat, "propdef_sg");
        assert_eq!(p.token_text(), "the Nile");
        assert!(EditorWord::parse("% comment").is_none());
        assert!(EditorWord::parse("").is_none());
    }
    #[test]
    fn editor_parser_predicts() {
        let ed = AceEditor::new(include_str!("../grammar/aceeditor_lexicon.pl"), None).unwrap();
        assert!(ed.lexicon.words.len() > 800);
        let mut p = ed.parser();
        for t in ["every", "man", "is", "a"] { assert!(p.is_possible_next_token(t), "{t}"); p.add_token(t); }
        assert!(p.next_token_options().con.iter().any(|c| c.word == "woman"));
        p.add_token("woman"); p.add_token(".");
        assert!(p.is_complete());
    }
}
