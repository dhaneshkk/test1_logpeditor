//! Codeco grammar templates (loaded from the JSON emitted by `grammar/generate_json.pl`).
//! A template is arena-independent; [`GrammarRule::instantiate`] plays the role of Java's
//! `GrammarRule.deepCopy()` (fresh variables, sharing preserved within the rule).

use super::{Cat, FeatureMap, Sym, VarMap, Vars};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum TV { Const(Sym), Var(u32) }

#[derive(Clone, Debug)]
pub enum CatT {
    Term(Sym),
    Pre(Sym, Vec<(Sym, TV)>),
    Non(Sym, Vec<(Sym, TV)>),
    Bref { pos: Vec<Vec<(Sym, TV)>>, neg: Vec<Vec<(Sym, TV)>> },
}

impl CatT {
    pub fn name(&self) -> &str { match self { CatT::Term(n) | CatT::Pre(n, _) | CatT::Non(n, _) => n, CatT::Bref { .. } => "<" } }
    fn inst_fm(fs: &[(Sym, TV)], vars: &mut Vars, m: &mut VarMap) -> FeatureMap {
        FeatureMap { f: fs.iter().map(|(n, v)| (n.clone(), match v {
            TV::Const(c) => vars.fresh(Some(c.clone())),
            TV::Var(i) => m.get_or_insert_with(*i, || vars.fresh(None)),
        })).collect() }
    }
    pub fn instantiate(&self, vars: &mut Vars, m: &mut VarMap) -> Cat {
        match self {
            CatT::Term(n) => Cat::Term(n.clone()),
            CatT::Pre(n, f) => Cat::Pre(n.clone(), Self::inst_fm(f, vars, m)),
            CatT::Non(n, f) => Cat::Non(n.clone(), Self::inst_fm(f, vars, m)),
            CatT::Bref { pos, neg } => Cat::Bref {
                pos: pos.iter().map(|f| Self::inst_fm(f, vars, m)).collect(),
                neg: neg.iter().map(|f| Self::inst_fm(f, vars, m)).collect(),
            },
        }
    }
    /// `category.isSimilar(rule.getHead())` without instantiating the template.
    pub fn similar_to(&self, c: &Cat, vars: &mut Vars) -> bool {
        match (c, self) {
            (Cat::Non(n, f), CatT::Non(m, g)) | (Cat::Pre(n, f), CatT::Pre(m, g)) => {
                if n != m { return false; }
                if f.f.is_empty() || g.is_empty() { return true; }
                for (name, v) in &f.f {
                    if let Some(val) = vars.get(*v) {
                        if let Some((_, TV::Const(k))) = g.iter().find(|(gn, _)| gn == name) { if *k != val { return false; } }
                    }
                }
                true
            }
            (Cat::Term(a), CatT::Term(b)) => a == b,
            (Cat::Bref { pos, .. }, CatT::Bref { pos: p2, .. }) => pos.len() == p2.len(),
            _ => false,
        }
    }
    fn feature_names(&self, out: &mut BTreeSet<Sym>) {
        match self {
            CatT::Term(_) => {}
            CatT::Pre(_, f) | CatT::Non(_, f) => out.extend(f.iter().map(|x| x.0.clone())),
            CatT::Bref { pos, neg } => for f in pos.iter().chain(neg) { out.extend(f.iter().map(|x| x.0.clone())) },
        }
    }
}

#[derive(Clone, Debug)]
pub struct GrammarRule { pub head: CatT, pub body: Vec<CatT>, pub scope_closing: bool }

impl GrammarRule {
    pub fn instantiate(&self, vars: &mut Vars) -> (Cat, Vec<Cat>) {
        let mut m = VarMap::new();
        let h = self.head.instantiate(vars, &mut m);
        let b = self.body.iter().map(|c| c.instantiate(vars, &mut m)).collect();
        (h, b)
    }
}

/// Lexical rule `$cat(features) => [word]`.
#[derive(Clone, Debug)]
pub struct LexicalRule { pub cat: CatT, pub word: Sym }

impl LexicalRule {
    /// Plain lexical rule with constant features (what AceWiki's dynamic lexicon creates).
    pub fn new(cat: &str, word: &str, feats: &[(&str, &str)]) -> Self {
        let mut f: Vec<(Sym, TV)> = feats.iter().map(|(n, v)| (Rc::from(*n), TV::Const(Rc::from(*v)))).collect();
        f.sort_by(|a, b| a.0.cmp(&b.0)); // sorted feature lists keep collect_vars allocation-free
        LexicalRule { cat: CatT::Pre(Rc::from(cat), f), word: Rc::from(word) }
    }
    pub fn cat_name(&self) -> &str { self.cat.name() }
}

#[derive(Debug, Default)]
pub struct Grammar {
    pub rules: Vec<GrammarRule>,
    pub lexrules: Vec<LexicalRule>,
    by_head: HashMap<String, Vec<usize>>,
    lex_by_cat: HashMap<String, Vec<usize>>,
    lex_by_word: HashMap<String, Vec<usize>>,
    /// Sorted feature names (Java `getFeatureNamesArray`, a TreeSet), used for identifiers.
    pub feature_names: Vec<Sym>,
    pub terminals: BTreeSet<String>,
}

impl Grammar {
    pub fn from_json(src: &str) -> anyhow::Result<Grammar> {
        let v: Value = serde_json::from_str(src)?;
        let mut g = Grammar::default();
        let mut interner: HashMap<String, Sym> = HashMap::new();
        let mut sym = |s: &str| interner.entry(s.to_string()).or_insert_with(|| Rc::from(s)).clone();
        let mut fnames = BTreeSet::new();
        let fm = |o: &Value, sym: &mut dyn FnMut(&str) -> Sym| -> Vec<(Sym, TV)> {
            let mut items: Vec<(Sym, TV)> = o.as_object().map(|m| m.iter().map(|(k, v)| {
                let tv = if let Some(n) = v.get("var") { TV::Var(n.as_u64().unwrap() as u32) }
                         else { TV::Const(sym(v["c"].as_str().unwrap())) };
                (sym(k), tv)
            }).collect()).unwrap_or_default();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            items
        };
        let cat = |c: &Value, sym: &mut dyn FnMut(&str) -> Sym| -> CatT {
            match c["t"].as_str().unwrap() {
                "term" => CatT::Term(sym(c["name"].as_str().unwrap())),
                "pre" => CatT::Pre(sym(c["name"].as_str().unwrap()), fm(&c["f"], sym)),
                "non" => CatT::Non(sym(c["name"].as_str().unwrap()), fm(&c["f"], sym)),
                "bref" => CatT::Bref {
                    pos: c["pos"].as_array().unwrap().iter().map(|f| fm(f, sym)).collect(),
                    neg: c["neg"].as_array().unwrap().iter().map(|f| fm(f, sym)).collect(),
                },
                t => panic!("unknown category type {t}"),
            }
        };
        for r in v["rules"].as_array().unwrap() {
            let cats: Vec<CatT> = r["cats"].as_array().unwrap().iter().map(|c| cat(c, &mut sym)).collect();
            for c in &cats { c.feature_names(&mut fnames); if let CatT::Term(t) = c { g.terminals.insert(t.to_string()); } }
            let rule = GrammarRule { head: cats[0].clone(), body: cats[1..].to_vec(), scope_closing: r["scope_closing"].as_bool().unwrap_or(false) };
            g.by_head.entry(rule.head.name().to_string()).or_default().push(g.rules.len());
            g.rules.push(rule);
        }
        for l in v["lexrules"].as_array().unwrap() {
            let c = cat(&l["cat"], &mut sym);
            c.feature_names(&mut fnames);
            let lr = LexicalRule { cat: c, word: sym(l["word"].as_str().unwrap()) };
            g.terminals.insert(lr.word.to_string());
            g.lex_by_cat.entry(lr.cat_name().to_string()).or_default().push(g.lexrules.len());
            g.lex_by_word.entry(lr.word.to_string()).or_default().push(g.lexrules.len());
            g.lexrules.push(lr);
        }
        g.feature_names = fnames.into_iter().collect();
        Ok(g)
    }
    pub fn rules_by_head(&self, name: &str) -> impl Iterator<Item = &GrammarRule> {
        self.by_head.get(name).into_iter().flatten().map(|i| &self.rules[*i])
    }
    pub fn lex_by_cat(&self, name: &str) -> impl Iterator<Item = &LexicalRule> {
        self.lex_by_cat.get(name).into_iter().flatten().map(|i| &self.lexrules[*i])
    }
    pub fn lex_by_word(&self, w: &str) -> impl Iterator<Item = &LexicalRule> {
        self.lex_by_word.get(w).into_iter().flatten().map(|i| &self.lexrules[*i])
    }
}
