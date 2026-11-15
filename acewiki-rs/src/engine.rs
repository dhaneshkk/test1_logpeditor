//! The AceWiki engine (`ACEOWLEngine` + `AceWikiOWLReasoner` + `ACESentence.update`).

use crate::chartparser::{ChartParser, DynamicLexicon, Grammar, LexicalRule};
use crate::model::{Element, Kind, Statement, TextElement, Wiki, Words};
use crate::owl::owllink::OwlLink;
use crate::owl::profile::{self, Profile};
use crate::owl::reasoner::{ElReasoner, Reasoner, RlReasoner};
use crate::owl::{parse_owlxml, Axiom, CE};
use ape_rs::Ape;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub const ACEWIKI_GRAMMAR: &str = include_str!("../grammar/acewiki_grammar.json");
pub const ACEEDITOR_GRAMMAR: &str = include_str!("../grammar/aceeditor_grammar.json");

#[derive(Clone, Debug)]
pub struct Config {
    pub base_uri: String,
    pub profile: Profile,
    /// "native" (default), "owllink", "none"
    pub reasoner: String,
    pub owllink_url: Option<String>,
    /// AceWiki `global_restrictions_policy`: "no_chains" (default) or "unchecked"
    pub no_chains: bool,
}
impl Default for Config {
    fn default() -> Self { Config { base_uri: "http://attempto.ifi.uzh.ch/acewiki/".into(), profile: Profile::RL, reasoner: "native".into(), owllink_url: None, no_chains: true } }
}

/// APE analysis of one sentence (what `ACESentence.update` computes).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Analysis {
    pub text: String,
    pub lexicon: String,
    pub paraphrase: String,
    pub syntax: String,
    pub syntaxpp: String,
    pub drspp: String,
    pub owlfsspp: String,
    pub owlxml: String,
    pub messages: Vec<(String, String, String)>, // (importance, type, value)
    pub is_owlswrl: bool,
    pub is_owl: bool,
    /// `isReasonable()` as seen by the wiki/reasoner (always false for questions, like `ACEQuestion`)
    pub reasonable: bool,
    /// the internal `reasonable` field of `ACESentence.update` (decides demotion of integrated sentences)
    pub integrable: bool,
    #[serde(skip)]
    pub axioms: Vec<Axiom>,
    #[serde(skip)]
    pub question_class: Option<CE>,
    pub question_individual: Option<String>,
    pub profile_violations: Vec<String>,
}

pub struct Engine {
    pub wiki: Wiki,
    pub cfg: Config,
    pub grammar: Arc<Grammar>,
    ape: Ape,
    cache: HashMap<(String, String), Analysis>,
    disk: HashMap<String, String>,
    disk_file: Option<std::path::PathBuf>,
    disk_dirty: bool,
    reasoner: Option<Box<dyn Reasoner>>,
    pub reasoner_note: String,
    /// prepared empty-prefix parser (lexicon set, shared rule cache); forked per editor request
    parser_tpl: std::cell::RefCell<Option<ChartParser>>,
}

impl Engine {
    pub fn new(wiki: Wiki, cfg: Config, ape: Ape) -> anyhow::Result<Self> {
        let grammar = Arc::new(Grammar::from_json(ACEWIKI_GRAMMAR)?);
        Ok(Engine { wiki, cfg, grammar, ape, cache: HashMap::new(), disk: HashMap::new(), disk_file: None, disk_dirty: false, reasoner: None, reasoner_note: String::new(), parser_tpl: Default::default() })
    }
    pub fn uri(&self) -> String { format!("{}{}", if self.cfg.base_uri.ends_with('/') { self.cfg.base_uri.clone() } else { format!("{}/", self.cfg.base_uri) }, self.wiki.name) }
    pub fn iri(&self, e: &Element) -> String { format!("{}#{}", self.uri(), e.words.iri_suffix()) }
    pub fn element_by_iri(&self, iri: &str) -> Option<&Element> {
        let u = self.uri();
        let name = iri.strip_prefix(&u)?.strip_prefix('#')?;
        self.wiki.elements.values().find(|e| e.kind != Kind::MainPage && e.words.iri_suffix() == name)
    }
    /// All elements denoted by an IRI (class, individual and property readings of a punned IRI).
    pub fn elements_by_iri<'a>(&'a self, iri: &str) -> Vec<&'a Element> {
        let u = self.uri();
        let Some(name) = iri.strip_prefix(&u).and_then(|x| x.strip_prefix('#')) else { return vec![] };
        self.wiki.elements.values().filter(|e| e.kind != Kind::MainPage && e.words.iri_suffix() == name).collect()
    }
    pub fn tokenize(&self, serialized: &str) -> anyhow::Result<Vec<TextElement>> { self.wiki.tokenize(serialized, &self.grammar.terminals) }
    pub fn is_question(serialized: &str) -> bool { serialized.trim().ends_with('?') }

    /// `ACESentence.update`.
    pub fn analyze(&mut self, serialized: &str) -> anyhow::Result<Analysis> {
        let tes = self.tokenize(serialized)?;
        let text = self.wiki.sentence_text(&tes);
        let lexicon = self.wiki.sentence_lexicon(&tes);
        let key = (text.clone(), lexicon.clone());
        if let Some(a) = self.cache.get(&key) { return Ok(a.clone()); }
        if let Some(raw) = self.disk.get(&format!("{text}\u{1}{lexicon}")).cloned() {
            let a = self.finish(raw, text, lexicon, serialized);
            self.cache.insert(key, a.clone());
            return Ok(a);
        }
        let uri = self.uri();
        let r = self.ape.get_results(&[
            ("text", &text), ("ulextext", &lexicon), ("uri", &uri), ("guess", "off"), ("noclex", "on"),
            ("cparaphrase1", "on"), ("csyntax", "on"), ("csyntaxpp", "on"), ("cowlxml", "on"), ("cowlfsspp", "on"), ("cdrspp", "on"),
        ])?;
        self.disk.insert(format!("{text}\u{1}{lexicon}"), r.content.clone());
        self.disk_dirty = true;
        let a = self.finish(r.content, text, lexicon, serialized);
        self.cache.insert(key, a.clone());
        Ok(a)
    }

    fn finish(&self, xml: String, text: String, lexicon: String, serialized: &str) -> Analysis {
        let mut a = parse_ape_result(&xml);
        a.text = text; a.lexicon = lexicon;
        a.is_owlswrl = !a.messages.iter().any(|m| m.1 == "owl") && !a.owlxml.is_empty();
        a.is_owl = a.is_owlswrl && !a.owlxml.contains("<swrl:Imp>") && !a.owlxml.contains("<DLSafeRule>");
        a.reasonable = if a.is_owl && self.cfg.no_chains {
            !a.owlxml.contains("<TransitiveObjectProperty>") && !a.owlxml.contains("<ObjectPropertyChain>")
        } else { a.is_owl };
        if a.is_owl {
            match parse_owlxml(&a.owlxml) {
                Ok((_, ax)) if ax.is_empty() => { a.reasonable = false; a.is_owl = false; a.is_owlswrl = false; }
                Ok((_, ax)) => a.axioms = ax,
                Err(_) => {}
            }
        }
        let question = Self::is_question(serialized);
        if a.reasonable && !a.axioms.is_empty() && !question && self.cfg.profile != Profile::Full {
            a.profile_violations = profile::check(self.cfg.profile, &a.axioms);
            if !a.profile_violations.is_empty() { a.reasonable = false; }
        }
        a.integrable = a.reasonable;
        if question {
            a.reasonable = false;
            if let Some(Axiom::SubClassOf(q, _)) = a.axioms.iter().find(|x| matches!(x, Axiom::SubClassOf(..))) {
                a.question_class = Some(q.clone());
                if let CE::OneOf(is) = q { if is.len() == 1 { a.question_individual = is.iter().next().cloned(); } }
            }
        }
        a
    }

    /// Persist raw APE results (keyed by text + lexicon) so restarts skip re-parsing.
    pub fn set_cache_file(&mut self, p: std::path::PathBuf) {
        if let Ok(s) = std::fs::read_to_string(&p) { if let Ok(m) = serde_json::from_str(&s) { self.disk = m; } }
        self.disk_file = Some(p);
    }
    pub fn flush_cache(&mut self) {
        if !self.disk_dirty { return; }
        if let Some(p) = &self.disk_file { if let Ok(s) = serde_json::to_string(&self.disk) { let _ = std::fs::write(p, s); } }
        self.disk_dirty = false;
    }

    /// Analyse every sentence; demote unreasonable integrated sentences (as Java does on load).
    pub fn load_all(&mut self) -> anyhow::Result<()> {
        let ids: Vec<u64> = self.wiki.elements.keys().copied().collect();
        for id in ids {
            let n = self.wiki.elements[&id].article.len();
            for i in 0..n {
                let Statement::Sentence { serialized, integrated } = self.wiki.elements[&id].article[i].clone() else { continue };
                let a = match self.analyze(&serialized) { Ok(a) => a, Err(_) => continue };
                if integrated && !a.integrable {
                    if let Statement::Sentence { integrated, .. } = &mut self.wiki.elements.get_mut(&id).unwrap().article[i] { *integrated = false; }
                }
            }
        }
        self.reasoner = None;
        self.flush_cache();
        Ok(())
    }

    pub fn declarations(&self) -> Vec<Axiom> {
        let mut v = Vec::new();
        let mut inds = BTreeSet::new();
        for e in self.wiki.elements.values() {
            let iri = self.iri(e);
            match e.kind {
                Kind::Noun => v.push(Axiom::Declaration("Class".into(), iri)),
                Kind::ProperName => { inds.insert(iri.clone()); v.push(Axiom::Declaration("NamedIndividual".into(), iri)); }
                Kind::NounOf | Kind::TrVerb | Kind::TrAdj => v.push(Axiom::Declaration("ObjectProperty".into(), iri)),
                Kind::MainPage => {}
            }
        }
        if inds.len() > 1 { v.push(Axiom::DifferentIndividuals(inds)); }
        v
    }
    /// All axioms the reasoner sees: declarations, unique names, integrated reasonable sentences.
    pub fn ontology_axioms(&mut self) -> Vec<Axiom> {
        let mut set: BTreeSet<Axiom> = self.declarations().into_iter().collect();
        let sents: Vec<String> = self.wiki.elements.values().flat_map(|e| e.article.iter().filter_map(|s| match s {
            Statement::Sentence { serialized, integrated: true } => Some(serialized.clone()), _ => None })).collect();
        for s in sents {
            if let Ok(a) = self.analyze(&s) { if a.reasonable { set.extend(a.axioms.into_iter()); } }
        }
        set.into_iter().collect()
    }

    fn reasoner(&mut self) -> &mut Box<dyn Reasoner> {
        if self.reasoner.is_none() {
            let axioms = self.ontology_axioms();
            let native: Box<dyn Reasoner> = match self.cfg.profile {
                Profile::EL => Box::new(ElReasoner::new(axioms.clone())),
                _ => Box::new(RlReasoner::new(axioms.clone())),
            };
            let unsupported = native.unsupported();
            let use_owllink = self.cfg.reasoner == "owllink" || (!unsupported.is_empty() && self.cfg.owllink_url.is_some());
            self.reasoner = Some(if use_owllink {
                let url = self.cfg.owllink_url.clone().unwrap_or_else(|| "http://localhost:8080".into());
                self.reasoner_note = format!("OWLlink at {url} ({} axioms outside the native profile)", unsupported.len());
                match OwlLink::new(&url, &self.uri(), axioms) {
                    Ok(o) => Box::new(o),
                    Err(e) => { self.reasoner_note = format!("OWLlink unavailable ({e}); using native engine"); native }
                }
            } else {
                self.reasoner_note = if unsupported.is_empty() { native.name().to_string() } else { format!("{} — {} axioms not supported natively: {:?}", native.name(), unsupported.len(), &unsupported[..unsupported.len().min(3)]) };
                native
            });
        }
        self.reasoner.as_mut().unwrap()
    }
    /// Call after any change to the wiki (sentences or words): drops the reasoner and the prepared
    /// parser (whose lexicon and rule cache depend on the words).
    pub fn invalidate(&mut self) { self.reasoner = None; *self.parser_tpl.borrow_mut() = None; }

    // ---- AceWikiReasoner contract, mapped back to wiki elements ----
    fn to_elements(&self, iris: BTreeSet<String>, want: fn(&Kind) -> bool) -> Vec<u64> {
        // kind-aware: a noun and an of-construct can share one IRI (OWL 2 punning, e.g. `capital`)
        let mut v: Vec<u64> = iris.iter().filter(|i| !i.starts_with("http://attempto.ifi.uzh.ch/ace#"))
            .flat_map(|i| self.elements_by_iri(i)).filter(|e| want(&e.kind)).map(|e| e.id).collect();
        v.sort(); v.dedup(); v
    }
    pub fn is_consistent(&mut self) -> bool { self.reasoner().is_consistent() }
    pub fn concepts_of(&mut self, ind: u64) -> Vec<u64> {
        let iri = self.iri(&self.wiki.elements[&ind]);
        let t = self.reasoner().types(&iri);
        self.to_elements(t, Kind::is_concept)
    }
    pub fn individuals_of(&mut self, c: u64) -> Vec<u64> {
        let iri = self.iri(&self.wiki.elements[&c]);
        let t = self.reasoner().instances(&CE::Class(iri));
        self.to_elements(t, Kind::is_individual)
    }
    pub fn super_concepts(&mut self, c: u64) -> Vec<u64> {
        let iri = self.iri(&self.wiki.elements[&c]);
        let t = self.reasoner().super_classes(&iri);
        self.to_elements(t, Kind::is_concept)
    }
    pub fn sub_concepts(&mut self, c: u64) -> Vec<u64> {
        let iri = self.iri(&self.wiki.elements[&c]);
        let t = self.reasoner().sub_classes(&iri);
        self.to_elements(t, Kind::is_concept)
    }
    pub fn is_satisfiable(&mut self, c: u64) -> bool {
        let iri = self.iri(&self.wiki.elements[&c]);
        self.reasoner().is_satisfiable(&iri)
    }
    /// `getAnswer(question)`: concepts of the asked individual, or instances of the class.
    pub fn answer(&mut self, serialized: &str) -> anyhow::Result<Option<Vec<u64>>> {
        let a = self.analyze(serialized)?;
        if let Some(i) = &a.question_individual {
            let t = self.reasoner().types(i);
            return Ok(Some(self.to_elements(t, Kind::is_concept)));
        }
        if let Some(q) = &a.question_class {
            let t = self.reasoner().instances(q);
            return Ok(Some(self.to_elements(t, Kind::is_individual)));
        }
        Ok(Some(vec![]))
    }

    /// Check a new sentence against the ontology (AceWiki rejects sentences that make it inconsistent).
    pub fn would_be_consistent(&mut self, serialized: &str) -> anyhow::Result<bool> {
        let a = self.analyze(serialized)?;
        if !a.reasonable { return Ok(true); }
        let mut ax = self.ontology_axioms();
        ax.extend(a.axioms);
        let mut r: Box<dyn Reasoner> = match self.cfg.profile { Profile::EL => Box::new(ElReasoner::new(ax)), _ => Box::new(RlReasoner::new(ax)) };
        Ok(r.is_consistent())
    }

    /// Predictive parser with AceWiki's dynamic lexicon over the current wiki words.
    /// A predictive parser at the empty prefix: a cheap fork of a prepared parser, so the lexicon
    /// scan, initial chart and per-category rule/option caches are shared across requests.
    pub fn parser(&self) -> ChartParser {
        let mut t = self.parser_tpl.borrow_mut();
        if t.is_none() {
            let mut p = ChartParser::new(self.grammar.clone(), "text");
            p.set_dynamic_lexicon(Box::new(WikiLexicon::from(&self.wiki)));
            p.set_rule_cache(Default::default());
            *t = Some(p);
        }
        t.as_ref().unwrap().clone()
    }
    pub fn ape(&mut self) -> &mut Ape { &mut self.ape }
}

/// `^[XYZ][0-9]*$`
pub fn is_variable(w: &str) -> bool {
    let b = w.as_bytes();
    !b.is_empty() && matches!(b[0], b'X' | b'Y' | b'Z') && b[1..].iter().all(u8::is_ascii_digit)
}
/// `^(?:[1-9][0-9]+|[2-9])$`
pub fn is_number(w: &str) -> bool {
    let b = w.as_bytes();
    match b.len() {
        0 => false,
        1 => (b'2'..=b'9').contains(&b[0]),
        _ => (b'1'..=b'9').contains(&b[0]) && b[1..].iter().all(u8::is_ascii_digit),
    }
}

/// `ACEOWLLexicon` (dynamic lexicon for the chart parser).
#[derive(Clone)]
pub struct WikiLexicon { words: Vec<Words>, index: HashMap<String, usize> }
impl WikiLexicon {
    pub fn from(w: &Wiki) -> Self {
        let mut words = vec![]; let mut index = HashMap::new();
        for e in w.elements.values() {
            if e.kind == Kind::MainPage { continue; }
            for f in e.words.all().into_iter().flatten() { index.entry(f).or_insert(words.len()); }
            words.push(e.words.clone());
        }
        WikiLexicon { words, index }
    }
    fn with_text(mut v: Vec<LexicalRule>) -> Vec<LexicalRule> {
        for r in &mut v {
            if let crate::chartparser::grammar::CatT::Pre(_, f) = &mut r.cat {
                f.retain(|(n, _)| &**n != "text");
                f.push((std::rc::Rc::from("text"), crate::chartparser::grammar::TV::Const(r.word.clone())));
                f.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        v
    }
}
impl DynamicLexicon for WikiLexicon {
    fn lex_rules_for_category(&self, cat: &str) -> Vec<LexicalRule> {
        let mut v = vec![];
        match cat {
            "variable" | "reference" => for x in ["X", "Y", "Z"] { v.push(LexicalRule::new(cat, x, &[])) },
            "number" => for i in 2..100 { v.push(LexicalRule::new("number", &i.to_string(), &[])) },
            _ => for w in &self.words { w.lexical_rules(Some(cat), &mut v) },
        }
        Self::with_text(v)
    }
    fn lex_rules_for_word(&self, word: &str) -> Vec<LexicalRule> {
        let mut v = vec![];
        if is_variable(word) { v.push(LexicalRule::new("variable", word, &[])); v.push(LexicalRule::new("reference", word, &[])); }
        else if is_number(word) { v.push(LexicalRule::new("number", word, &[])); }
        else {
            let mut ix = self.index.get(word);
            if let Some(rest) = word.strip_prefix("the ") { ix = self.index.get(rest); }
            if let Some(i) = ix { self.words[*i].lexical_rules(None, &mut v); }
        }
        Self::with_text(v)
    }
}

/// Parse APE's `<apeResult>` XML (multi-mode output).
pub fn parse_ape_result(xml: &str) -> Analysis {
    let mut a = Analysis::default();
    let get = |tag: &str| -> String {
        let (o, c) = (format!("<{tag}>"), format!("</{tag}>"));
        match (xml.find(&o), xml.find(&c)) { (Some(i), Some(j)) if j > i => unescape(&xml[i + o.len()..j]), _ => String::new() }
    };
    a.paraphrase = get("paraphrase1"); a.syntax = get("syntax"); a.syntaxpp = get("syntaxpp");
    a.drspp = get("drspp"); a.owlfsspp = get("owlfsspp"); a.owlxml = get("owlxml");
    let re = regex::Regex::new(r#"(?s)<message\s+importance="([^"]*)"\s+type="([^"]*)"\s+sentence="[^"]*"\s+token="[^"]*"\s+value="([^"]*)""#).unwrap();
    for c in re.captures_iter(xml) { a.messages.push((c[1].to_string(), c[2].to_string(), unescape(&c[3]))); }
    a
}
pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#xA;", "\n").replace("&#xD;", "\r").replace("&#x9;", "\t").replace("&amp;", "&")
}

#[allow(dead_code)]
fn _unused(_: BTreeMap<u8, u8>) {}
