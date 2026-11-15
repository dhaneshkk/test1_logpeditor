//! `ChartParser`, `Edge`, `Chart`, `CPAbstractOption`, `CPConcreteOption` — ported 1:1.

use super::grammar::{Grammar, LexicalRule};
use super::{Cat, FeatureMap, FxBuild, Sym, Var, VarMap, Vars};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Edge {
    pub start: usize,
    pub end: usize,
    pub head: Cat,
    pub ext: Vec<Cat>,
    pub int: Vec<Cat>,
    pub scope_closing: bool,
    pub body: Vec<Cat>,
    pub progress: usize,
    pub links: Vec<Option<Rc<Edge>>>,
}

impl Edge {
    fn is_active(&self) -> bool { self.body.len() > self.progress }
    fn next_active(&self) -> Option<&Cat> { self.body.get(self.progress) }
    fn next_active_mut(&mut self) -> Option<&mut Cat> { self.body.get_mut(self.progress) }
    fn carries_antecedent_information(&self) -> bool { self.body.is_empty() || matches!(self.head, Cat::Pre(..)) }
    fn combined(&self) -> Vec<&Cat> { self.ext.iter().chain(self.int.iter()).collect() }
    fn combined_len(&self) -> usize { self.ext.len() + self.int.len() }
    fn combined_mut(&mut self, i: usize) -> &mut Cat {
        if i < self.ext.len() { &mut self.ext[i] } else { let k = i - self.ext.len(); &mut self.int[k] }
    }
    fn deep_copy(&self, vars: &mut Vars) -> Edge {
        let mut m = VarMap::new();
        Edge {
            start: self.start, end: self.end, scope_closing: self.scope_closing, progress: self.progress,
            head: self.head.deep_copy(vars, &mut m),
            body: self.body.iter().map(|c| c.deep_copy(vars, &mut m)).collect(),
            ext: self.ext.iter().map(|c| c.deep_copy(vars, &mut m)).collect(),
            int: self.int.iter().map(|c| c.deep_copy(vars, &mut m)).collect(),
            links: self.links.clone(),
        }
    }
    fn step_with(&mut self, pos: usize, edge: Rc<Edge>) {
        self.progress += 1;
        self.end = pos;
        let mut restr = false;
        for c in &edge.int {
            if !edge.scope_closing { self.int.push(c.clone()); }
            else if c.name() == "//" { restr = true; }
            else if !restr || c.name() == ">>" { self.int.push(c.clone()); }
        }
        self.links.push(Some(edge));
    }
    fn step(&mut self) { self.progress += 1; self.links.push(None); }

    fn identifier_into(&self, vars: &mut Vars, used: &[Sym], id: &mut String) {
        let (mut seen, mut mv) = (Vec::new(), Vec::new());
        self.head.collect_vars(vars, &mut seen, &mut mv);
        for c in self.ext.iter().chain(&self.int).chain(&self.body) { c.collect_vars(vars, &mut seen, &mut mv); }
        id.clear();
        super::push_num(id, self.start); id.push(','); super::push_num(id, self.end); id.push(' ');
        self.head.identifier_into(vars, &mv, used, id); id.push(' ');
        for c in &self.ext { c.identifier_into(vars, &mv, used, id); }
        id.push(if self.scope_closing { '~' } else { '=' });
        for c in &self.int { c.identifier_into(vars, &mv, used, id); }
        id.push(' '); super::push_num(id, self.progress);
        for c in &self.body { c.identifier_into(vars, &mv, used, id); id.push(' '); }
    }
}

/// Chart edges are immutable once added (unification works on deep copies), so an edge's
/// identifier is computed once and kept for removal.
#[derive(Clone)]
struct Chart { ids: HashSet<String, FxBuild>, by_end: HashMap<usize, Vec<Rc<Edge>>, FxBuild>, ids_at: HashMap<usize, Vec<String>, FxBuild>, size: usize, scratch: String }
impl Chart {
    fn new() -> Self { Chart { ids: HashSet::default(), by_end: HashMap::default(), ids_at: HashMap::default(), size: 0, scratch: String::with_capacity(256) } }
    fn add(&mut self, e: Edge, vars: &mut Vars, used: &[Sym]) -> bool {
        // identifier built in a reusable buffer: duplicates (the common case) allocate nothing
        let mut buf = std::mem::take(&mut self.scratch);
        e.identifier_into(vars, used, &mut buf);
        let dup = self.ids.contains(buf.as_str());
        let id = if dup { None } else { Some(buf.clone()) };
        self.scratch = buf;
        let Some(id) = id else { return false };
        self.ids.insert(id.clone());
        self.ids_at.entry(e.end).or_default().push(id);
        self.by_end.entry(e.end).or_default().push(Rc::new(e));
        self.size += 1;
        true
    }
    fn at(&self, end: usize) -> Vec<Rc<Edge>> { self.by_end.get(&end).cloned().unwrap_or_default() }
    fn len_at(&self, end: usize) -> usize { self.by_end.get(&end).map_or(0, |l| l.len()) }
    fn get(&self, end: usize, i: usize) -> Rc<Edge> { self.by_end[&end][i].clone() }
    fn remove_end(&mut self, end: usize, vars: &mut Vars, used: &[Sym]) {
        let _ = (vars, used);
        if let Some(l) = self.by_end.remove(&end) { self.size -= l.len(); }
        if let Some(ids) = self.ids_at.remove(&end) { for id in ids { self.ids.remove(&id); } }
    }
}

/// Java `CPAbstractOption`: a category the next token must fulfil, minus exceptions.
#[derive(Clone, Debug)]
pub struct AbstractOption { pub category: Cat, pub exceptions: Vec<Cat>, pub id: String }

impl AbstractOption {
    fn new(category: Cat, exceptions: Vec<Cat>, vars: &mut Vars, used: &[Sym]) -> Self {
        let mut id = String::new();
        let (mut s, mut m) = (Vec::new(), Vec::new());
        category.collect_vars(vars, &mut s, &mut m);
        id += &category.identifier(vars, &m, used); id += " / ";
        for x in &exceptions {
            let (mut s, mut m) = (Vec::new(), Vec::new());
            x.collect_vars(vars, &mut s, &mut m);
            id += &x.identifier(vars, &m, used); id += " ";
        }
        AbstractOption { category, exceptions, id }
    }
    pub fn category_name(&self) -> &str { self.category.name() }
    pub fn is_fulfilled_by(&self, c: &Cat, vars: &mut Vars) -> bool {
        if !self.category.subsumes(c, vars) { return false; }
        !self.exceptions.iter().any(|x| x.subsumes(c, vars))
    }
    /// Java toString(), with variable ids renumbered canonically (for oracle parity).
    pub fn display(&self, vars: &mut Vars) -> String {
        let mut s = self.category.display(vars);
        if !self.exceptions.is_empty() {
            s += " except";
            for x in &self.exceptions { s += " "; s += &x.display(vars); }
        }
        s
    }
}

/// Java `CPConcreteOption`: a concrete next word, with its preterminal (if lexical).
#[derive(Clone, Debug)]
pub struct ConcreteOption { pub word: String, pub category: Option<Cat>, pub id: String }

/// AceWiki's dynamic lexicon (words of the wiki ontology, variables, numbers).
/// Lexical rules per category (grammar rules + dynamic lexicon), shareable between parsers that use
/// the same lexicon. Must be cleared (or replaced) when the lexicon changes.
pub type RuleCache = Rc<std::cell::RefCell<LexCache>>;

#[derive(Default)]
pub struct LexCache {
    rules: HashMap<Sym, Rc<Vec<LexicalRule>>, FxBuild>,
    /// abstract-option identifier -> (rule index, concrete-option identifier) of the rules that fulfil it.
    /// The identifier canonically encodes the category's bound values, co-references and exceptions,
    /// which is all `is_fulfilled_by` depends on for a lexical-rule template.
    fulfil: HashMap<String, Rc<Vec<(usize, String)>>, FxBuild>,
}
impl LexCache { pub fn clear(&mut self) { self.rules.clear(); self.fulfil.clear(); } }

pub trait DynamicLexicon {
    fn lex_rules_for_category(&self, category_name: &str) -> Vec<LexicalRule>;
    fn lex_rules_for_word(&self, word: &str) -> Vec<LexicalRule>;
}

#[derive(Clone, Debug)]
pub struct NextTokenOptions { pub abs: Vec<AbstractOption>, pub con: Vec<ConcreteOption> }

/// Cheap to clone ("fork"): edges are shared `Rc`s and the lexicon is shared, so a prepared parser
/// (e.g. the empty prefix with its lexicon set) can be reused for every new sentence.
#[derive(Clone)]
pub struct ChartParser {
    grammar: Arc<Grammar>,
    start: String,
    used: Vec<Sym>,
    pub vars: Vars,
    chart: Chart,
    tokens: Vec<String>,
    options: Vec<Option<NextTokenOptions>>,
    backrefs: Vec<Vec<FeatureMap>>,
    progress: Option<HashMap<String, usize>>,
    dyn_lex: Option<Rc<dyn DynamicLexicon>>,
    pos_prefix: String,
    rule_cache: RuleCache,
}

impl ChartParser {
    pub fn new(grammar: Arc<Grammar>, start: &str) -> Self {
        let used = grammar.feature_names.clone();
        let mut p = ChartParser {
            grammar, start: start.to_string(), used, vars: Vars::default(), chart: Chart::new(),
            tokens: vec![], options: vec![None], backrefs: vec![], progress: None, dyn_lex: None, pos_prefix: "#".into(),
            rule_cache: RuleCache::default(),
        };
        p.init();
        p.run();
        p
    }
    /// Share a per-category rule cache with other parsers over the same lexicon (see [`RuleCache`]).
    pub fn set_rule_cache(&mut self, c: RuleCache) { self.rule_cache = c; }
    fn rules_for(&self, name: &Sym) -> Rc<Vec<LexicalRule>> {
        if let Some(r) = self.rule_cache.borrow().rules.get(name) { return r.clone(); }
        let mut rules: Vec<LexicalRule> = self.grammar.lex_by_cat(name).cloned().collect();
        if let Some(d) = &self.dyn_lex { rules.extend(d.lex_rules_for_category(name)); }
        let r = Rc::new(rules);
        self.rule_cache.borrow_mut().rules.insert(name.clone(), r.clone());
        r
    }
    pub fn set_dynamic_lexicon(&mut self, d: Box<dyn DynamicLexicon>) {
        self.dyn_lex = Some(Rc::from(d));
        self.rule_cache.borrow_mut().clear();
        let n = self.tokens.len();
        self.update_concrete(n);
    }
    pub fn tokens(&self) -> &[String] { &self.tokens }

    fn init(&mut self) {
        let g = self.grammar.clone();
        for r in g.rules_by_head(&self.start) {
            let (head, body) = r.instantiate(&mut self.vars);
            let e = Edge { start: 0, end: 0, head, ext: vec![], int: vec![], scope_closing: r.scope_closing, body, progress: 0, links: vec![] };
            self.chart.add(e, &mut self.vars, &self.used);
        }
    }

    pub fn add_token(&mut self, token: &str) {
        let n = self.tokens.len();
        let t: Sym = Rc::from(token);
        self.chart.add(Edge { start: n, end: n + 1, head: Cat::Term(t.clone()), ext: vec![], int: vec![], scope_closing: false, body: vec![], progress: 0, links: vec![] }, &mut self.vars, &self.used);
        let mut lex: Vec<LexicalRule> = self.grammar.lex_by_word(token).cloned().collect();
        if let Some(d) = &self.dyn_lex { lex.extend(d.lex_rules_for_word(token)); }
        for lr in lex {
            let mut m = VarMap::new();
            let head = lr.cat.instantiate(&mut self.vars, &mut m);
            let e = Edge { start: n, end: n, head, ext: vec![], int: vec![], scope_closing: false, body: vec![Cat::Term(lr.word.clone())], progress: 0, links: vec![] };
            self.chart.add(e, &mut self.vars, &self.used);
        }
        self.run();
        self.tokens.push(token.to_string());
        self.options.push(None);
        self.backrefs.push(vec![]);
        self.progress = None;
        self.run();
    }
    pub fn add_tokens<S: AsRef<str>>(&mut self, ts: &[S]) { for t in ts { self.add_token(t.as_ref()); } }
    pub fn remove_token(&mut self) {
        let n = self.tokens.len();
        self.chart.remove_end(n, &mut self.vars, &self.used);
        self.backrefs.pop();
        self.options.pop();
        self.tokens.pop();
        self.progress = None;
        let n = self.tokens.len();
        self.update_concrete(n);
    }

    pub fn is_complete(&self) -> bool {
        self.chart.at(self.tokens.len()).iter().any(|e| e.start == 0 && !e.is_active() && e.head.name() == self.start)
    }
    /// Java `getReference()`: position of the antecedent the last token refers back to, or -1.
    pub fn reference(&mut self) -> i64 {
        if self.tokens.is_empty() { return -1; }
        let pos = self.tokens.len() - 1;
        let mut r = -1i64;
        let star: Sym = Rc::from("*pos");
        let fms = self.backrefs[pos].clone();
        for mut f in fms {
            let v = f.get(&mut self.vars, &star);
            if let Some(s) = self.vars.get(v) { if let Ok(i) = s.parse::<i64>() { if i - 1 > -1 { r = i - 1; } } }
        }
        r
    }

    pub fn next_token_options(&mut self) -> &NextTokenOptions {
        let n = self.tokens.len();
        self.create_options(n);
        self.options[n].as_ref().unwrap()
    }
    pub fn abstract_option_strings(&mut self) -> Vec<String> {
        let opts = self.next_token_options().abs.clone();
        opts.iter().map(|o| canonical_vars(&o.display(&mut self.vars))).collect()
    }
    pub fn is_possible_next_token(&mut self, token: &str) -> bool {
        if self.next_token_options().con.iter().any(|c| c.word == token) { return true; }
        let rules = self.dyn_lex.as_ref().map(|d| d.lex_rules_for_word(token)).unwrap_or_default();
        let abs = self.next_token_options().abs.clone();
        for lr in rules {
            if &*lr.word != token { continue; }
            let c = lr.cat.instantiate(&mut self.vars, &mut VarMap::new());
            if abs.iter().any(|o| matches!(o.category, Cat::Pre(..)) && o.category.name() == c.name() && o.is_fulfilled_by(&c, &mut self.vars)) { return true; }
        }
        false
    }

    fn create_options(&mut self, pos: usize) {
        if self.options[pos].is_none() {
            let a = self.create_abstract(pos);
            let c = self.create_concrete(&a);
            self.options[pos] = Some(NextTokenOptions { abs: a, con: c });
        }
    }
    fn update_concrete(&mut self, pos: usize) {
        match self.options[pos].take() {
            None => self.create_options(pos),
            Some(o) => { let c = self.create_concrete(&o.abs); self.options[pos] = Some(NextTokenOptions { abs: o.abs, con: c }); }
        }
    }

    fn push_abs(&mut self, set: &mut Vec<AbstractOption>, seen: &mut HashSet<String>, cat: Cat, exc: Vec<Cat>) {
        let o = AbstractOption::new(cat, exc, &mut self.vars, &self.used);
        if seen.insert(o.id.clone()) { set.push(o); }
    }

    fn create_abstract(&mut self, pos: usize) -> Vec<AbstractOption> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for e in self.chart.at(pos) {
            if !e.is_active() { continue; }
            if e.next_active().unwrap().is_nonterminal() { continue; }
            let p = e.progress;
            let (mut backref, mut negbackref, mut refpos) = (false, false, 0usize);
            for i in p + 1..e.body.len() {
                let c = &e.body[i];
                if !c.is_nonterminal() { continue; }
                if matches!(c, Cat::Bref { .. }) { backref = true; refpos = i; }
                else if i == p + 1 && c.name() == "/<" { negbackref = true; refpos = i; }
                break;
            }
            if backref {
                let (npos, nneg) = match &e.body[refpos] { Cat::Bref { pos, neg } => (pos.len(), neg.len()), _ => unreachable!() };
                for i in (0..e.combined_len()).rev() {
                    if e.combined()[i].name() == "//" { continue; }
                    let mut exceptions: Option<Vec<Cat>> = None;
                    let mut make = true;
                    if refpos == p + 1 {
                        let mut ex = Vec::new();
                        for j in 0..nneg {
                            let mut ec = e.deep_copy(&mut self.vars);
                            let mut fm = match &ec.body[refpos] { Cat::Bref { neg, .. } => neg[j].clone(), _ => unreachable!() };
                            let ok = match ec.combined_mut(i).fm_mut() { Some(a) => a.unify(&mut fm, &mut self.vars).is_ok(), None => false };
                            if ok {
                                if matches!(ec.next_active(), Some(Cat::Term(_))) { make = false; break; }
                                ex.push(ec.next_active().unwrap().clone());
                            }
                        }
                        exceptions = Some(ex);
                    }
                    if !make { break; }
                    for j in 0..npos {
                        let mut ec = e.deep_copy(&mut self.vars);
                        let mut fm = match &ec.body[refpos] { Cat::Bref { pos, .. } => pos[j].clone(), _ => unreachable!() };
                        let ok = match ec.combined_mut(i).fm_mut() { Some(a) => a.unify(&mut fm, &mut self.vars).is_ok(), None => false };
                        if ok {
                            let cat = ec.next_active().unwrap().clone();
                            let exc = match &exceptions { Some(x) => x.iter().map(|c| c.copy1(&mut self.vars)).collect(), None => vec![] };
                            self.push_abs(&mut out, &mut seen, cat, exc);
                        }
                    }
                }
            } else if negbackref {
                let mut exceptions = Vec::new();
                let mut make = true;
                for i in 0..e.combined_len() {
                    if e.combined()[i].name() == "//" { continue; }
                    let mut ec = e.deep_copy(&mut self.vars);
                    let mut fm = ec.body[refpos].fm().cloned().unwrap_or_default();
                    let ok = match ec.combined_mut(i).fm_mut() { Some(a) => a.unify(&mut fm, &mut self.vars).is_ok(), None => false };
                    if ok {
                        if matches!(ec.next_active(), Some(Cat::Term(_))) { make = false; break; }
                        exceptions.push(ec.next_active().unwrap().clone());
                    }
                }
                if make { let c = e.next_active().unwrap().copy1(&mut self.vars); self.push_abs(&mut out, &mut seen, c, exceptions); }
            } else {
                let c = e.next_active().unwrap().copy1(&mut self.vars);
                self.push_abs(&mut out, &mut seen, c, vec![]);
            }
        }
        out
    }

    fn create_concrete(&mut self, abs: &[AbstractOption]) -> Vec<ConcreteOption> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for ao in abs {
            match &ao.category {
                Cat::Pre(name, _) => {
                    let rules = self.rules_for(name);
                    let hit = self.rule_cache.borrow().fulfil.get(&ao.id).cloned();
                    let list = match hit {
                        Some(l) => l,
                        None => {
                            let mut l = Vec::new();
                            for (i, lr) in rules.iter().enumerate() {
                                // cheap necessary condition for subsumption before instantiating the rule
                                if !lr.cat.similar_to(&ao.category, &mut self.vars) { continue; }
                                let c = lr.cat.instantiate(&mut self.vars, &mut VarMap::new());
                                if ao.is_fulfilled_by(&c, &mut self.vars) {
                                    let (mut s, mut m) = (Vec::new(), Vec::new());
                                    c.collect_vars(&mut self.vars, &mut s, &mut m);
                                    l.push((i, format!("'{}' <- {}", lr.word, c.identifier(&mut self.vars, &m, &self.used))));
                                }
                            }
                            let l = Rc::new(l);
                            self.rule_cache.borrow_mut().fulfil.insert(ao.id.clone(), l.clone());
                            l
                        }
                    };
                    for (i, id) in list.iter() {
                        if seen.insert(id.clone()) {
                            let lr = &rules[*i];
                            let c = lr.cat.instantiate(&mut self.vars, &mut VarMap::new());
                            out.push(ConcreteOption { word: lr.word.to_string(), category: Some(c), id: id.clone() });
                        }
                    }
                }
                Cat::Term(t) => {
                    let id = format!("'{t}' <-");
                    if seen.insert(id.clone()) { out.push(ConcreteOption { word: t.to_string(), category: None, id }); }
                }
                _ => {}
            }
        }
        out
    }

    fn run(&mut self) {
        if self.progress.is_none() {
            self.progress = Some(HashMap::from([("prediction".into(), 0), ("completion".into(), 0), ("resolution".into(), 0)]));
        }
        let (mut step, mut idle) = (0, 0);
        loop {
            step += 1;
            let size = self.chart.size;
            if step == 1 { self.predict(); } else if step == 2 { self.resolve(); } else { self.complete(); step = 0; }
            if size == self.chart.size { idle += 1; } else { idle = 0; }
            if idle > 2 { break; }
        }
    }
    fn prog(&self, k: &str) -> usize { self.progress.as_ref().unwrap().get(k).copied().unwrap_or(0) }
    fn set_prog(&mut self, k: String, v: usize) { self.progress.as_mut().unwrap().insert(k, v); }

    fn predict(&mut self) {
        let n = self.tokens.len();
        let g = self.grammar.clone();
        let mut i = self.prog("prediction");
        while i < self.chart.len_at(n) {
            let existing = self.chart.get(n, i);
            i += 1;
            let Some(cat) = existing.next_active() else { continue };
            if matches!(cat, Cat::Term(_)) || cat.is_special() { continue; }
            let name = cat.name().to_string();
            for rule in g.rules_by_head(&name) {
                if !rule.head.similar_to(cat, &mut self.vars) { continue; }
                let mut ec = existing.deep_copy(&mut self.vars);
                let (mut head, body) = rule.instantiate(&mut self.vars);
                if ec.next_active_mut().unwrap().unify(&mut head, &mut self.vars).is_err() { continue; }
                let ante: Vec<Cat> = if body.is_empty() { vec![] } else { ec.ext.iter().chain(ec.int.iter()).cloned().collect() };
                let e = Edge { start: n, end: n, head, ext: ante, int: vec![], scope_closing: rule.scope_closing, body, progress: 0, links: vec![] };
                self.chart.add(e, &mut self.vars, &self.used);
            }
        }
        let l = self.chart.len_at(n);
        self.set_prog("prediction".into(), l);
    }

    fn complete(&mut self) {
        let n = self.tokens.len();
        let mut i1 = 0;
        while i1 < self.chart.len_at(n) {
            let passive = self.chart.get(n, i1);
            if passive.is_active() { i1 += 1; continue; }
            let startp = passive.start;
            let start = if i1 < self.prog("completion") { self.prog(&format!("completion {i1}")) } else { 0 };
            let mut i2 = start;
            while i2 < self.chart.len_at(startp) {
                let edge = self.chart.get(startp, i2);
                i2 += 1;
                if !edge.is_active() { continue; }
                if edge.next_active().unwrap().name() != passive.head.name() { continue; }
                if !passive.head.is_similar(edge.next_active().unwrap(), &mut self.vars) { continue; }
                let mut pc = passive.deep_copy(&mut self.vars);
                let mut ec = edge.deep_copy(&mut self.vars);
                if pc.head.unify(ec.next_active_mut().unwrap(), &mut self.vars).is_err() { continue; }
                if !passive.carries_antecedent_information() {
                    let l1 = ec.combined_len();
                    if l1 != pc.ext.len() { continue; }
                    let mut ok = true;
                    for k in 0..l1 {
                        let mut b = pc.ext[k].clone();
                        if ec.combined_mut(k).unify(&mut b, &mut self.vars).is_err() { ok = false; break; }
                        pc.ext[k] = b;
                    }
                    if !ok { continue; }
                }
                ec.step_with(n, Rc::new(pc));
                self.chart.add(ec, &mut self.vars, &self.used);
            }
            let l2 = self.chart.len_at(startp);
            self.set_prog(format!("completion {i1}"), l2);
            i1 += 1;
        }
        let l1 = self.chart.len_at(n);
        self.set_prog("completion".into(), l1);
    }

    fn resolve(&mut self) {
        let n = self.tokens.len();
        let mut i1 = self.prog("resolution");
        let star: Sym = Rc::from("*pos");
        let posf: Sym = Rc::from("pos");
        while i1 < self.chart.len_at(n) {
            let edge = self.chart.get(n, i1);
            i1 += 1;
            if !edge.is_active() { continue; }
            let name = edge.next_active().unwrap().name().to_string();
            let mut new_edges: Vec<Edge> = Vec::new();
            match name.as_str() {
                "#" => {
                    let mut ec = edge.deep_copy(&mut self.vars);
                    let id = format!("{}{}", self.pos_prefix, n);
                    let k = self.vars.fresh(Some(Rc::from(id)));
                    let fm = ec.next_active_mut().unwrap().fm_mut().unwrap();
                    let v = fm.get(&mut self.vars, &posf);
                    if self.vars.unify(v, k).is_ok() { new_edges.push(ec); }
                }
                ">" | ">>" | "//" => {
                    let mut ec = edge.deep_copy(&mut self.vars);
                    let k = self.vars.fresh(Some(Rc::from(n.to_string())));
                    ec.next_active_mut().unwrap().fm_mut().unwrap().set(star.clone(), k);
                    let c = ec.next_active().unwrap().clone();
                    ec.int.push(c);
                    new_edges.push(ec);
                }
                "<" => {
                    let (posfms, negfms) = match edge.next_active().unwrap() { Cat::Bref { pos, neg } => (pos.clone(), neg.clone()), _ => unreachable!() };
                    let ante: Vec<Cat> = edge.combined().into_iter().cloned().collect();
                    for i in (0..ante.len()).rev() {
                        if ante[i].name() == "//" { continue; }
                        let afm = ante[i].fm().cloned().unwrap_or_default();
                        if negfms.iter().any(|nf| afm.can_unify(nf, &mut self.vars)) { continue; }
                        let mut pos_match = false;
                        for j in 0..posfms.len() {
                            if afm.can_unify(&posfms[j], &mut self.vars) {
                                let mut ec = edge.deep_copy(&mut self.vars);
                                let mut target = match ec.next_active().unwrap() { Cat::Bref { pos, .. } => pos[j].clone(), _ => unreachable!() };
                                // Java: edgeC.getExternalAnteList()[i] (index into the *external* list)
                                if i >= ec.ext.len() { continue; }
                                let ok = ec.ext[i].fm_mut().map(|a| a.unify(&mut target, &mut self.vars).is_ok()).unwrap_or(false);
                                if ok {
                                    let fmc = ec.ext[i].fm().unwrap().deep_copy(&mut self.vars, &mut VarMap::new());
                                    if n > 0 { self.backrefs[n - 1].push(fmc); }
                                    new_edges.push(ec);
                                    pos_match = true;
                                }
                            }
                        }
                        if pos_match { break; }
                    }
                }
                "/<" => {
                    let nfm = edge.next_active().unwrap().fm().cloned().unwrap_or_default();
                    let blocked = edge.combined().iter().any(|c| c.name() != "//" && c.fm().map_or(false, |f| f.can_unify(&nfm, &mut self.vars)));
                    if !blocked { new_edges.push(edge.deep_copy(&mut self.vars)); }
                }
                _ => continue,
            }
            for mut ne in new_edges { ne.step(); self.chart.add(ne, &mut self.vars, &self.used); }
        }
        let l = self.chart.len_at(n);
        self.set_prog("resolution".into(), l);
    }

    /// Parse tree of the complete text (start category spanning all tokens).
    pub fn parse_tree(&self) -> Option<ParseTree> {
        self.chart.at(self.tokens.len()).into_iter()
            .find(|e| e.start == 0 && !e.is_active() && e.head.name() == self.start)
            .map(|e| ParseTree::from_edge(&e))
    }
}

/// Minimal parse tree (category name + span), enough for AceWiki's sentence splitting.
#[derive(Debug, Clone)]
pub struct ParseTree { pub cat: String, pub start: usize, pub end: usize, pub children: Vec<ParseTree> }
impl ParseTree {
    fn from_edge(e: &Edge) -> ParseTree {
        ParseTree { cat: e.head.name().to_string(), start: e.start, end: e.end,
            children: e.links.iter().flatten().map(|l| ParseTree::from_edge(l)).collect() }
    }
    pub fn subtrees(&self, cat: &str) -> Vec<&ParseTree> {
        let mut out = vec![];
        if self.cat == cat { out.push(self); }
        for c in &self.children { out.extend(c.subtrees(cat)); }
        out
    }
}

/// Renumber unbound-variable ids in a Java-style category string (`name:123`) canonically,
/// so option strings from the JVM and from Rust compare equal when structurally identical.
pub fn canonical_vars(s: &str) -> String {
    // equivalent to replacing /:(\d+)([,)])/ by ":_V<k><sep>", k = first-occurrence index (no regex:
    // this runs once per abstract option in the editor)
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 8);
    let mut map: Vec<&str> = Vec::new();
    let (mut i, mut last) = (0, 0);
    while i < b.len() {
        if b[i] == b':' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            if j > i + 1 && j < b.len() && (b[j] == b',' || b[j] == b')') {
                let id = &s[i + 1..j];
                let k = match map.iter().position(|x| *x == id) { Some(k) => k, None => { map.push(id); map.len() - 1 } };
                out.push_str(&s[last..i]);
                out.push_str(":_V");
                out.push_str(&k.to_string());
                last = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

#[allow(dead_code)]
fn _assert_var(_: Var) {}

#[cfg(test)]
mod canon_tests {
    use super::canonical_vars;
    #[test]
    fn matches_regex_semantics() {
        let re = regex::Regex::new(r":(\d+)([,)])").unwrap();
        let reference = |s: &str| {
            let mut map: std::collections::HashMap<String, usize> = Default::default();
            re.replace_all(s, |c: &regex::Captures| { let n = map.len(); let k = *map.entry(c[1].to_string()).or_insert(n); format!(":_V{k}{}", &c[2]) }).into_owned()
        };
        for s in ["np(case:12,num:7,gender:12)", "a:1)b:22,c:1,d:3x", "x:,y:5", "noun(text:boat)", ":9)", "v(a:12a,b:0)", ""] {
            assert_eq!(canonical_vars(s), reference(s), "{s}");
        }
    }
}
