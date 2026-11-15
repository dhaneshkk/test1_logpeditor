//! AceWiki data model: ontology elements (words + article), statements, sentence text elements.
//! Word-form, serialization, tokenization and text-rendering rules are ported from
//! `ProperNameIndividual`, `NounConcept`, `OfRelation`, `VerbRelation`, `TrAdjRelation`,
//! `FileBasedStorage`, `ACESentence.tokenize/serialize`, `ACETextOperator`, `AbstractSentence.getText`.

use crate::chartparser::LexicalRule;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind { MainPage, ProperName, Noun, NounOf, TrVerb, TrAdj }

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "mainpage" => Kind::MainPage, "propername" => Kind::ProperName, "noun" => Kind::Noun,
            "nounof" => Kind::NounOf, "trverb" => Kind::TrVerb, "tradj" => Kind::TrAdj, _ => return None,
        })
    }
    pub fn internal(&self) -> &'static str {
        match self { Kind::MainPage => "mainpage", Kind::ProperName => "propername", Kind::Noun => "noun",
            Kind::NounOf => "nounof", Kind::TrVerb => "trverb", Kind::TrAdj => "tradj" }
    }
    pub fn label(&self) -> &'static str {
        match self { Kind::MainPage => "Main Page", Kind::ProperName => "Proper Name", Kind::Noun => "Noun",
            Kind::NounOf => "Of-Construct", Kind::TrVerb => "Verb", Kind::TrAdj => "Transitive Adjective" }
    }
    pub fn is_individual(&self) -> bool { *self == Kind::ProperName }
    pub fn is_concept(&self) -> bool { *self == Kind::Noun }
    pub fn is_relation(&self) -> bool { matches!(self, Kind::NounOf | Kind::TrVerb | Kind::TrAdj) }
}

/// Java `String.split(";")`: drops trailing empty strings.
pub fn java_split(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.split(';').map(|x| x.to_string()).collect();
    while v.last().is_some_and(|x| x.is_empty()) { v.pop(); }
    if v.is_empty() { v.push(String::new()); }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub enum Words {
    Main(String),
    ProperName { word: String, word_def: bool, abbrev: Option<String>, abbrev_def: bool },
    Noun { sg: String, pl: String },
    NounOf { word: String },
    TrVerb { third: String, inf: String, passive: Option<String> },
    TrAdj { word: String },
}

impl Words {
    pub fn parse(kind: &Kind, s: &str) -> Words {
        let w = java_split(s);
        let g = |i: usize| w.get(i).cloned().unwrap_or_default();
        match kind {
            Kind::MainPage => Words::Main(g(0)),
            Kind::ProperName => {
                let word;
                let (mut wd, mut ab, mut ad) = (false, None, false);
                if w.len() == 1 { word = g(0); }
                else if w.len() == 2 { word = g(1); wd = g(0).starts_with("the "); }
                else if w.len() < 3 || g(0) == g(2) { word = g(1); wd = g(0).starts_with("the "); }
                else { word = g(1); wd = g(0).starts_with("the "); ab = w.get(3).cloned(); ad = g(2).starts_with("the "); }
                if ab.as_deref() == Some("") { ab = None; }
                Words::ProperName { word, word_def: wd, abbrev: ab, abbrev_def: ad }
            }
            Kind::Noun => Words::Noun { sg: g(0), pl: g(1) },
            Kind::NounOf => { let mut x = g(0); if !x.ends_with(" of") { x += " of"; } Words::NounOf { word: x } }
            Kind::TrVerb => {
                let p = if w.len() < 3 { None } else if g(2).ends_with(" by") { Some(g(2)) } else { Some(g(2) + " by") };
                let p = p.filter(|x| !x.is_empty() && x != " by");
                Words::TrVerb { third: g(0), inf: g(1), passive: p }
            }
            Kind::TrAdj => Words::TrAdj { word: g(0) },
        }
    }
    /// Java `getWords()` (word numbers index into this).
    pub fn all(&self) -> Vec<Option<String>> {
        match self {
            Words::Main(w) => vec![Some(w.clone())],
            Words::ProperName { word, word_def, abbrev, abbrev_def } => {
                let the = |b: bool, x: &str| if b { format!("the {x}") } else { x.to_string() };
                match abbrev {
                    None => vec![Some(the(*word_def, word)), Some(word.clone()), Some(the(*word_def, word)), Some(word.clone())],
                    Some(a) => vec![Some(the(*word_def, word)), Some(word.clone()), Some(the(*abbrev_def, a)), Some(a.clone())],
                }
            }
            Words::Noun { sg, pl } => vec![Some(sg.clone()), Some(pl.clone())],
            Words::NounOf { word } => vec![Some(word.clone())],
            Words::TrVerb { third, inf, passive } => vec![Some(third.clone()), Some(inf.clone()), passive.clone()],
            Words::TrAdj { word } => vec![Some(word.clone())],
        }
    }
    pub fn word(&self, n: usize) -> Option<String> { self.all().get(n).cloned().flatten() }
    pub fn serialize(&self) -> String {
        match self {
            Words::Main(w) => format!("{w};"),
            Words::ProperName { .. } => { let a = self.all(); format!("{};{};{};{};", a[0].clone().unwrap(), a[1].clone().unwrap(), a[2].clone().unwrap(), a[3].clone().unwrap()) }
            Words::Noun { sg, pl } => format!("{sg};{pl};"),
            Words::NounOf { word } | Words::TrAdj { word } => format!("{word};"),
            Words::TrVerb { third, inf, passive } => format!("{third};{inf};{};", passive.clone().unwrap_or_default()),
        }
    }
    /// Words shown as article heading (Java `getHeadwords`).
    pub fn headwords(&self) -> Vec<String> {
        match self {
            Words::ProperName { abbrev: Some(_), .. } => vec![self.word(1).unwrap(), self.word(3).unwrap()],
            Words::ProperName { .. } => vec![self.word(1).unwrap()],
            _ => vec![self.word(0).unwrap_or_default()],
        }
    }
    pub fn iri_suffix(&self) -> String {
        match self {
            Words::Main(w) => w.clone(),
            Words::ProperName { word, .. } => word.clone(),
            Words::Noun { sg, .. } => sg.clone(),
            Words::NounOf { word } => word[..word.len().saturating_sub(3)].to_string(),
            Words::TrVerb { inf, .. } => inf.clone(),
            Words::TrAdj { word } => word.clone(),
        }
    }
    /// APE user-lexicon entries (`ACEOWLOntoElement.getLexiconEntries`), in the exact Java format.
    pub fn lexicon_entries(&self) -> Vec<String> {
        let e = |s: &str| prolog_quote(s);
        match self {
            Words::Main(_) => vec![],
            Words::ProperName { word, word_def, abbrev, abbrev_def } => {
                let pn = |def: bool| if def { "pndef_sg" } else { "pn_sg" };
                let mut v = vec![format!("{}({}, {}, undef).", pn(*word_def), e(word), e(word))];
                if let Some(a) = abbrev { v.push(format!("{}({}, {}, undef).", pn(*abbrev_def), e(a), e(word))); }
                v
            }
            Words::Noun { sg, pl } => vec![format!("noun_sg({}, {}, undef).", e(sg), e(sg)), format!("noun_pl({}, {}, undef).", e(pl), e(sg))],
            Words::NounOf { .. } => { let n = self.iri_suffix(); vec![format!("noun_sg({}, {}, undef).", e(&n), e(&n))] }
            Words::TrVerb { third, inf, passive } => {
                let mut v = vec![format!("tv_finsg({}, {}).", e(third), e(inf)), format!("tv_infpl({}, {}).", e(inf), e(inf))];
                if let Some(p) = passive { v.push(format!("tv_pp({}, {}).", e(&p[..p.len() - 3]), e(inf))); }
                v
            }
            Words::TrAdj { word } => vec![format!("adj_tr({}, {}, '').", e(word), e(word))],
        }
    }
    /// Dynamic-lexicon rules (`collectLexicalRules`); `cat == None` means "all categories".
    pub fn lexical_rules(&self, cat: Option<&str>, out: &mut Vec<LexicalRule>) {
        let want = |c: &str| cat.is_none() || cat == Some(c);
        let w = |n: usize| self.word(n).unwrap_or_default();
        match self {
            Words::Main(_) => {}
            Words::ProperName { abbrev, .. } => {
                if want("propername") {
                    out.push(LexicalRule::new("propername", &w(0), &[]));
                    if abbrev.is_some() { out.push(LexicalRule::new("propername", &w(2), &[])); }
                }
            }
            Words::Noun { .. } => {
                if want("noun") { out.push(LexicalRule::new("noun", &w(0), &[])); }
                if want("nounpl") { out.push(LexicalRule::new("nounpl", &w(1), &[])); }
                if want("defnoun") { out.push(LexicalRule::new("defnoun", &format!("the {}", w(0)), &[("noun", &w(0))])); }
            }
            Words::NounOf { .. } => if want("nounof") { out.push(LexicalRule::new("nounof", &w(0), &[])); },
            Words::TrVerb { passive, .. } => {
                if want("verbsg") { out.push(LexicalRule::new("verbsg", &w(0), &[])); }
                if want("verbinf") { out.push(LexicalRule::new("verbinf", &w(1), &[])); }
                if want("pverb") && passive.is_some() { out.push(LexicalRule::new("pverb", &w(2), &[])); }
            }
            Words::TrAdj { .. } => if want("tradj") { out.push(LexicalRule::new("tradj", &w(0), &[])); },
        }
    }
    pub fn has_definite_article(&self, n: usize) -> bool {
        match self { Words::ProperName { word_def, abbrev_def, .. } => (n == 0 && *word_def) || (n == 2 && *abbrev_def), _ => false }
    }
    pub fn proper_name_def(&self) -> bool { matches!(self, Words::ProperName { word_def: true, .. }) }
}

/// `PrologUtils.escape` + quoting, as used by `LexiconEntry.toString`.
pub fn prolog_quote(s: &str) -> String { format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")) }

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Sentence { serialized: String, integrated: bool },
    Comment(String),
}

#[derive(Clone, Debug)]
pub struct Element { pub id: u64, pub kind: Kind, pub words: Words, pub article: Vec<Statement> }

impl Element {
    pub fn word(&self) -> String { self.words.word(0).unwrap_or_default() }
    pub fn heading(&self) -> String {
        let h = self.words.headwords();
        let p = |s: &str| s.replace('_', " ");
        if h.len() > 1 { format!("{} ({})", p(&h[0]), h[1..].iter().map(|x| p(x)).collect::<Vec<_>>().join(", ")) } else { p(&h[0]) }
    }
}

/// A sentence token: plain word or a reference to an ontology element's word form.
#[derive(Clone, Debug, PartialEq)]
pub enum TextElement {
    Plain(String),
    Onto { id: u64, wn: usize, pre: String, post: String },
}

/// The wiki: elements by id + word index (`DefaultWordIndex`).
#[derive(Debug, Default)]
pub struct Wiki {
    pub name: String,
    pub elements: BTreeMap<u64, Element>,
    pub index: HashMap<String, u64>,
}

impl Wiki {
    pub fn reindex(&mut self) {
        self.index.clear();
        for (id, e) in &self.elements {
            for w in e.words.all().into_iter().flatten() { self.index.entry(w).or_insert(*id); }
        }
    }
    pub fn by_word(&self, w: &str) -> Option<&Element> { self.index.get(w).and_then(|i| self.elements.get(i)) }
    pub fn next_id(&self) -> u64 { self.elements.keys().max().map_or(1, |m| m + 1) }

    /// `ACETextOperator.createTextElement`.
    pub fn create_text_element(&self, text: &str, terminals: &std::collections::BTreeSet<String>) -> TextElement {
        if terminals.contains(text) { return TextElement::Plain(text.to_string()); }
        let lower = text.to_lowercase();
        if terminals.contains(&lower) { return TextElement::Plain(lower); }
        let t = regex_replace(text, r"[ _]+", "_");
        if let Some(te) = self.onto_te(&t) { return te; }
        let tl = t.to_lowercase();
        if tl.starts_with("the_") {
            if let Some(te) = self.onto_te(&format!("the {}", &t[4..])) { return te; }
            if let Some(TextElement::Onto { id, wn, post, .. }) = self.onto_te(&t[4..]) {
                return TextElement::Onto { id, wn, pre: "the ".into(), post };
            }
        }
        if tl.ends_with("_by") { if let Some(te) = self.onto_te(&format!("{} by", &t[..t.len() - 3])) { return te; } }
        if tl.ends_with("_of") { if let Some(te) = self.onto_te(&format!("{} of", &t[..t.len() - 3])) { return te; } }
        TextElement::Plain(text.to_string())
    }
    fn onto_te(&self, text: &str) -> Option<TextElement> {
        let e = self.by_word(text)?;
        let words = e.words.all();
        let mut wn = None;
        for (i, w) in words.iter().enumerate() { if w.as_deref() == Some(text) { wn = Some(i); } }
        wn.map(|wn| TextElement::Onto { id: e.id, wn, pre: String::new(), post: String::new() })
    }

    /// `ACESentence.tokenize`: serialized sentence -> text elements.
    pub fn tokenize(&self, serialized: &str, terminals: &std::collections::BTreeSet<String>) -> anyhow::Result<Vec<TextElement>> {
        let mut t = format!("&{serialized}&").replace(' ', "&").replace('.', "&.&").replace('?', "&?&");
        t = t.replace("&of&", " of&").replace("&by&", " by&");
        let mut out: Vec<TextElement> = Vec::new();
        let has_ref = serialized.contains('<');
        for s in t.split('&').filter(|x| !x.is_empty()) {
            if s.starts_with('<') {
                let inner = &s[1..s.find('>').ok_or_else(|| anyhow::anyhow!("bad link {s}"))?];
                let (a, b) = inner.split_once(',').ok_or_else(|| anyhow::anyhow!("bad link {s}"))?;
                let id: u64 = a.parse()?; let wn: usize = b.parse()?;
                let e = self.elements.get(&id).ok_or_else(|| anyhow::anyhow!("Could not resolve link: {s}"))?;
                if e.words.word(wn).is_none() { anyhow::bail!("{s}: no word number {wn}"); }
                out.push(TextElement::Onto { id, wn, pre: String::new(), post: String::new() });
            } else {
                let te = self.create_text_element(s, terminals);
                match te {
                    TextElement::Onto { id, mut wn, .. } if !has_ref => {
                        let e = &self.elements[&id];
                        if e.kind == Kind::ProperName && wn >= 1 && e.words.has_definite_article(wn - 1) {
                            if let Some(TextElement::Plain(p)) = out.last() {
                                if p == "the" || p == "The" { out.pop(); wn -= 1; }
                            }
                        }
                        out.push(TextElement::Onto { id, wn, pre: String::new(), post: String::new() });
                    }
                    te => out.push(te),
                }
            }
        }
        Ok(out)
    }
    pub fn original_text(&self, te: &TextElement) -> String {
        match te {
            TextElement::Plain(s) => s.clone(),
            TextElement::Onto { id, wn, pre, post } => format!("{pre}{}{post}", self.elements.get(id).and_then(|e| e.words.word(*wn)).unwrap_or_default()),
        }
    }
    /// `ACETextOperator.getTextInContext` (before PrettyTextElement's underscore replacement).
    fn text_in_context(&self, te: &TextElement, preceding: Option<&str>, following: Option<&str>) -> String {
        let text = self.original_text(te);
        let at_start = preceding.map_or(true, |p| matches!(p, "." | "?" | "!"));
        let cap = |t: &str| { let mut c = t.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } };
        match te {
            TextElement::Onto { id, .. } => {
                let capitalize = at_start && self.elements.get(id).map_or(true, |e| e.words.proper_name_def());
                if capitalize && !text.is_empty() { cap(&text) } else { text }
            }
            TextElement::Plain(_) => {
                let mut t = if at_start && !text.is_empty() { cap(&text) } else { text };
                if let Some(f) = following {
                    if t == "A" || t == "a" || t == "An" || t == "an" {
                        let first = t.chars().next().unwrap().to_string();
                        t = if use_an(f) { first + "n" } else { first };
                    }
                }
                t
            }
        }
    }
    /// Contextual display text of each element (`TextElement.getText`, incl. pretty printing).
    pub fn display_tokens(&self, tes: &[TextElement]) -> Vec<String> {
        (0..tes.len()).map(|i| {
            let p = if i > 0 { Some(self.original_text(&tes[i - 1])) } else { None };
            let f = tes.get(i + 1).map(|x| self.original_text(x));
            let t = self.text_in_context(&tes[i], p.as_deref(), f.as_deref());
            if matches!(tes[i], TextElement::Onto { .. }) { t.replace('_', " ") } else { t }
        }).collect()
    }
    /// `AbstractSentence.getText`: the ACE text sent to APE (ontology words keep underscores).
    pub fn sentence_text(&self, tes: &[TextElement]) -> String {
        let disp = self.display_tokens(tes);
        let mut s = String::new();
        for i in 0..tes.len() {
            if i > 0 {
                let punct = |x: &str| x.len() == 1 && ".,:;?!".contains(x);
                let leftp = |x: &str| x == "¿" || x == "¡";
                if !(punct(&disp[i]) || leftp(&disp[i - 1])) { s.push(' '); }
            }
            let p = if i > 0 { Some(self.original_text(&tes[i - 1])) } else { None };
            let f = tes.get(i + 1).map(|x| self.original_text(x));
            s += &self.text_in_context(&tes[i], p.as_deref(), f.as_deref());
        }
        s
    }
    /// `ACESentence.getTextElements`: the elements the predictive editor is fed when a sentence
    /// is edited — pre-text and the definite article of proper names become separate tokens.
    pub fn editor_elements(&self, tes: &[TextElement]) -> Vec<TextElement> {
        let disp = self.display_tokens(tes);
        let mut out = Vec::new();
        for (i, te) in tes.iter().enumerate() {
            match te {
                TextElement::Onto { id, wn, pre, post } => {
                    let bare = TextElement::Onto { id: *id, wn: *wn, pre: String::new(), post: String::new() };
                    let ote = if pre.is_empty() { te.clone() } else { out.push(TextElement::Plain(pre.clone())); bare };
                    let _ = post;
                    match self.elements.get(id) {
                        Some(e) if e.kind == Kind::ProperName && e.words.has_definite_article(*wn) => {
                            out.push(TextElement::Plain(disp[i].chars().take(3).collect()));
                            out.push(TextElement::Onto { id: *id, wn: wn + 1, pre: String::new(), post: String::new() });
                        }
                        _ => out.push(ote),
                    }
                }
                _ => out.push(te.clone()),
            }
        }
        out
    }
    /// `ACESentence.serialize`.
    pub fn serialize(&self, tes: &[TextElement]) -> String {
        let mut s = String::new();
        for te in tes {
            match te {
                TextElement::Onto { id, wn, pre, post } => { s += pre; s += &format!("<{id},{wn}> "); s += post; }
                TextElement::Plain(t) => { s += t; s.push(' '); }
            }
        }
        s.trim_end_matches(' ').to_string()
    }
    /// APE user lexicon for a sentence (order of first occurrence, de-duplicated like `Lexicon`).
    pub fn sentence_lexicon(&self, tes: &[TextElement]) -> String {
        let mut seen: Vec<String> = Vec::new();
        for te in tes {
            if let TextElement::Onto { id, .. } = te {
                if let Some(e) = self.elements.get(id) { for l in e.words.lexicon_entries() { if !seen.contains(&l) { seen.push(l); } } }
            }
        }
        seen.iter().map(|x| format!("{x} ")).collect()
    }
}

/// `ACEUtils.useIndefiniteArticleAn`.
pub fn use_an(word: &str) -> bool {
    if word.is_empty() { return false; }
    let w = word.to_lowercase();
    // ^[aeiou].*$ | ^[fhlmnrsx]$ | ^[fhlmnrsx]-.*$, minus ^u$ | ^u-.* | ^u[rtn]i.* | ^use.* | ^uk.*
    let c: Vec<char> = w.chars().collect();
    let fh = |x: char| "fhlmnrsx".contains(x);
    let mut an = "aeiou".contains(c[0]) || (c.len() == 1 && fh(c[0])) || (c.len() > 1 && fh(c[0]) && c[1] == '-');
    if w == "u" || w.starts_with("u-") || (c.len() > 2 && c[0] == 'u' && "rtn".contains(c[1]) && c[2] == 'i') || w.starts_with("use") || w.starts_with("uk") { an = false; }
    an
}

fn regex_replace(s: &str, re: &str, with: &str) -> String {
    use std::sync::{LazyLock, Mutex};
    static CACHE: LazyLock<Mutex<HashMap<String, regex::Regex>>> = LazyLock::new(Default::default);
    let mut c = CACHE.lock().unwrap();
    c.entry(re.to_string()).or_insert_with(|| regex::Regex::new(re).unwrap()).replace_all(s, with).into_owned()
}
