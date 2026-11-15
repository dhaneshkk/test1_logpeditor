//! OWL 2 structural model (the subset APE emits), OWL/XML reader/writer, profile checks.

pub mod profile;
pub mod reasoner;
pub mod owllink;

use anyhow::{anyhow, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeSet;
use std::fmt::Write;

pub const THING: &str = "http://www.w3.org/2002/07/owl#Thing";
pub const NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Prop { Named(String), Inv(String) }
impl Prop {
    pub fn inverse(&self) -> Prop { match self { Prop::Named(p) => Prop::Inv(p.clone()), Prop::Inv(p) => Prop::Named(p.clone()) } }
    pub fn base(&self) -> &str { match self { Prop::Named(p) | Prop::Inv(p) => p } }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lit { Int(i64), Str(String), Other(String, String) }

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CE {
    Class(String),
    And(BTreeSet<CE>),
    Or(BTreeSet<CE>),
    Not(Box<CE>),
    OneOf(BTreeSet<String>),
    Some(Prop, Box<CE>),
    All(Prop, Box<CE>),
    HasValue(Prop, String),
    HasSelf(Prop),
    Min(u32, Prop, Box<CE>),
    Max(u32, Prop, Box<CE>),
    Exact(u32, Prop, Box<CE>),
    /// Data-property restrictions (kept opaque: AceWiki's ACE subset rarely produces them).
    Data(String),
}
impl CE {
    pub fn is_thing(&self) -> bool { matches!(self, CE::Class(c) if c == THING) }
    pub fn is_nothing(&self) -> bool { matches!(self, CE::Class(c) if c == NOTHING) }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Axiom {
    Declaration(String, String),               // (entity type, IRI)
    SubClassOf(CE, CE),
    EquivalentClasses(BTreeSet<CE>),
    DisjointClasses(BTreeSet<CE>),
    DisjointUnion(String, BTreeSet<CE>),
    ClassAssertion(CE, String),
    PropertyAssertion(Prop, String, String),
    NegativePropertyAssertion(Prop, String, String),
    SameIndividual(BTreeSet<String>),
    DifferentIndividuals(BTreeSet<String>),
    SubPropertyOf(Prop, Prop),
    SubPropertyChain(Vec<Prop>, Prop),
    EquivalentProperties(BTreeSet<Prop>),
    DisjointProperties(BTreeSet<Prop>),
    InverseProperties(Prop, Prop),
    Domain(Prop, CE),
    Range(Prop, CE),
    Functional(Prop),
    InverseFunctional(Prop),
    Reflexive(Prop),
    Irreflexive(Prop),
    Symmetric(Prop),
    Asymmetric(Prop),
    Transitive(Prop),
    /// Anything else (data axioms, SWRL rules, annotations): name + raw XML.
    Other(String, String),
}

// ------------------------------------------------------------------------------------- reader

#[derive(Debug, Clone)]
struct Node { name: String, attrs: Vec<(String, String)>, kids: Vec<Node>, text: String }
impl Node {
    fn attr(&self, k: &str) -> Option<&str> { self.attrs.iter().find(|(a, _)| a == k).map(|x| x.1.as_str()) }
    fn iri(&self, base: &str) -> String {
        if let Some(i) = self.attr("IRI") { return resolve(i, base); }
        if let Some(a) = self.attr("abbreviatedIRI") { return a.to_string(); }
        String::new()
    }
}
fn resolve(iri: &str, base: &str) -> String {
    if iri.contains("://") { iri.to_string() } else if let Some(f) = iri.strip_prefix('#') { format!("{}#{}", base.trim_end_matches('#'), f) } else { format!("{base}{iri}") }
}

fn parse_tree(xml: &str) -> Result<Node> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(true);
    let mut stack: Vec<Node> = vec![Node { name: "#root".into(), attrs: vec![], kids: vec![], text: String::new() }];
    loop {
        match r.read_event()? {
            Event::Start(e) => stack.push(node(&e)?),
            Event::Empty(e) => { let n = node(&e)?; stack.last_mut().unwrap().kids.push(n); }
            Event::End(_) => { let n = stack.pop().unwrap(); stack.last_mut().ok_or_else(|| anyhow!("xml"))?.kids.push(n); }
            Event::Text(t) => { let s = t.unescape()?.into_owned(); stack.last_mut().unwrap().text += &s; }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack.pop().unwrap();
    root.kids.into_iter().find(|k| k.name == "Ontology").ok_or_else(|| anyhow!("no Ontology element"))
}
fn node(e: &quick_xml::events::BytesStart) -> Result<Node> {
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut attrs = vec![];
    for a in e.attributes().flatten() {
        attrs.push((String::from_utf8_lossy(a.key.as_ref()).into_owned(), a.unescape_value()?.into_owned()));
    }
    Ok(Node { name, attrs, kids: vec![], text: String::new() })
}

/// Parse an OWL/XML ontology document (APE `owlxml` output) into axioms.
pub fn parse_owlxml(xml: &str) -> Result<(String, Vec<Axiom>)> {
    let ont = parse_tree(xml)?;
    let base = ont.attr("ontologyIRI").unwrap_or("").to_string();
    let mut out = vec![];
    for k in &ont.kids {
        if k.name == "Prefix" || k.name == "Import" || k.name == "Annotation" { continue; }
        out.push(axiom(k, &base).unwrap_or_else(|_| Axiom::Other(k.name.clone(), format!("{k:?}"))));
    }
    Ok((base, out))
}

fn prop(n: &Node, b: &str) -> Result<Prop> {
    match n.name.as_str() {
        "ObjectProperty" => Ok(Prop::Named(n.iri(b))),
        "ObjectInverseOf" => Ok(prop(&n.kids[0], b)?.inverse()),
        x => Err(anyhow!("not an object property: {x}")),
    }
}
fn ind(n: &Node, b: &str) -> Result<String> {
    match n.name.as_str() { "NamedIndividual" => Ok(n.iri(b)), "AnonymousIndividual" => Ok(format!("_:{}", n.attr("nodeID").unwrap_or(""))), x => Err(anyhow!("not an individual: {x}")) }
}
fn ce(n: &Node, b: &str) -> Result<CE> {
    let k = &n.kids;
    let set = |xs: &[Node]| -> Result<BTreeSet<CE>> { xs.iter().map(|x| ce(x, b)).collect() };
    let card = |n: &Node| -> u32 { n.attr("cardinality").and_then(|c| c.parse().ok()).unwrap_or(0) };
    let filler = |xs: &[Node]| -> Result<Box<CE>> { Ok(Box::new(if xs.len() > 1 { ce(&xs[1], b)? } else { CE::Class(THING.into()) })) };
    Ok(match n.name.as_str() {
        "Class" => CE::Class(n.iri(b)),
        "ObjectIntersectionOf" => CE::And(set(k)?),
        "ObjectUnionOf" => CE::Or(set(k)?),
        "ObjectComplementOf" => CE::Not(Box::new(ce(&k[0], b)?)),
        "ObjectOneOf" => CE::OneOf(k.iter().map(|x| ind(x, b)).collect::<Result<_>>()?),
        "ObjectSomeValuesFrom" => CE::Some(prop(&k[0], b)?, Box::new(ce(&k[1], b)?)),
        "ObjectAllValuesFrom" => CE::All(prop(&k[0], b)?, Box::new(ce(&k[1], b)?)),
        "ObjectHasValue" => CE::HasValue(prop(&k[0], b)?, ind(&k[1], b)?),
        "ObjectHasSelf" => CE::HasSelf(prop(&k[0], b)?),
        "ObjectMinCardinality" => CE::Min(card(n), prop(&k[0], b)?, filler(k)?),
        "ObjectMaxCardinality" => CE::Max(card(n), prop(&k[0], b)?, filler(k)?),
        "ObjectExactCardinality" => CE::Exact(card(n), prop(&k[0], b)?, filler(k)?),
        x if x.starts_with("Data") => CE::Data(format!("{n:?}")),
        x => return Err(anyhow!("unsupported class expression {x}")),
    })
}
fn axiom(n: &Node, b: &str) -> Result<Axiom> {
    let k = &n.kids;
    let ces = |xs: &[Node]| -> Result<BTreeSet<CE>> { xs.iter().map(|x| ce(x, b)).collect() };
    Ok(match n.name.as_str() {
        "Declaration" => Axiom::Declaration(k[0].name.clone(), k[0].iri(b)),
        "SubClassOf" => Axiom::SubClassOf(ce(&k[0], b)?, ce(&k[1], b)?),
        "EquivalentClasses" => Axiom::EquivalentClasses(ces(k)?),
        "DisjointClasses" => Axiom::DisjointClasses(ces(k)?),
        "DisjointUnion" => Axiom::DisjointUnion(k[0].iri(b), ces(&k[1..])?),
        "ClassAssertion" => Axiom::ClassAssertion(ce(&k[0], b)?, ind(&k[1], b)?),
        "ObjectPropertyAssertion" => Axiom::PropertyAssertion(prop(&k[0], b)?, ind(&k[1], b)?, ind(&k[2], b)?),
        "NegativeObjectPropertyAssertion" => Axiom::NegativePropertyAssertion(prop(&k[0], b)?, ind(&k[1], b)?, ind(&k[2], b)?),
        "SameIndividual" => Axiom::SameIndividual(k.iter().map(|x| ind(x, b)).collect::<Result<_>>()?),
        "DifferentIndividuals" => Axiom::DifferentIndividuals(k.iter().map(|x| ind(x, b)).collect::<Result<_>>()?),
        "SubObjectPropertyOf" => {
            if k[0].name == "ObjectPropertyChain" {
                Axiom::SubPropertyChain(k[0].kids.iter().map(|x| prop(x, b)).collect::<Result<_>>()?, prop(&k[1], b)?)
            } else { Axiom::SubPropertyOf(prop(&k[0], b)?, prop(&k[1], b)?) }
        }
        "EquivalentObjectProperties" => Axiom::EquivalentProperties(k.iter().map(|x| prop(x, b)).collect::<Result<_>>()?),
        "DisjointObjectProperties" => Axiom::DisjointProperties(k.iter().map(|x| prop(x, b)).collect::<Result<_>>()?),
        "InverseObjectProperties" => Axiom::InverseProperties(prop(&k[0], b)?, prop(&k[1], b)?),
        "ObjectPropertyDomain" => Axiom::Domain(prop(&k[0], b)?, ce(&k[1], b)?),
        "ObjectPropertyRange" => Axiom::Range(prop(&k[0], b)?, ce(&k[1], b)?),
        "FunctionalObjectProperty" => Axiom::Functional(prop(&k[0], b)?),
        "InverseFunctionalObjectProperty" => Axiom::InverseFunctional(prop(&k[0], b)?),
        "ReflexiveObjectProperty" => Axiom::Reflexive(prop(&k[0], b)?),
        "IrreflexiveObjectProperty" => Axiom::Irreflexive(prop(&k[0], b)?),
        "SymmetricObjectProperty" => Axiom::Symmetric(prop(&k[0], b)?),
        "AsymmetricObjectProperty" => Axiom::Asymmetric(prop(&k[0], b)?),
        "TransitiveObjectProperty" => Axiom::Transitive(prop(&k[0], b)?),
        x => Axiom::Other(x.to_string(), format!("{n:?}")),
    })
}

// ------------------------------------------------------------------------------------- writer

fn w_prop(s: &mut String, p: &Prop) {
    match p { Prop::Named(i) => { let _ = write!(s, "<ObjectProperty IRI=\"{}\"/>", esc(i)); }
              Prop::Inv(i) => { let _ = write!(s, "<ObjectInverseOf><ObjectProperty IRI=\"{}\"/></ObjectInverseOf>", esc(i)); } }
}
fn w_ind(s: &mut String, i: &str) { let _ = write!(s, "<NamedIndividual IRI=\"{}\"/>", esc(i)); }
pub fn w_ce(s: &mut String, c: &CE) {
    let tag = |s: &mut String, t: &str, kids: &dyn Fn(&mut String)| { let _ = write!(s, "<{t}>"); kids(s); let _ = write!(s, "</{t}>"); };
    match c {
        CE::Class(i) => { let _ = write!(s, "<Class IRI=\"{}\"/>", esc(i)); }
        CE::And(xs) => tag(s, "ObjectIntersectionOf", &|s| for x in xs { w_ce(s, x) }),
        CE::Or(xs) => tag(s, "ObjectUnionOf", &|s| for x in xs { w_ce(s, x) }),
        CE::Not(x) => tag(s, "ObjectComplementOf", &|s| w_ce(s, x)),
        CE::OneOf(is) => tag(s, "ObjectOneOf", &|s| for i in is { w_ind(s, i) }),
        CE::Some(p, x) => tag(s, "ObjectSomeValuesFrom", &|s| { w_prop(s, p); w_ce(s, x) }),
        CE::All(p, x) => tag(s, "ObjectAllValuesFrom", &|s| { w_prop(s, p); w_ce(s, x) }),
        CE::HasValue(p, i) => tag(s, "ObjectHasValue", &|s| { w_prop(s, p); w_ind(s, i) }),
        CE::HasSelf(p) => tag(s, "ObjectHasSelf", &|s| w_prop(s, p)),
        CE::Min(n, p, x) | CE::Max(n, p, x) | CE::Exact(n, p, x) => {
            let t = match c { CE::Min(..) => "ObjectMinCardinality", CE::Max(..) => "ObjectMaxCardinality", _ => "ObjectExactCardinality" };
            let _ = write!(s, "<{t} cardinality=\"{n}\">"); w_prop(s, p); w_ce(s, x); let _ = write!(s, "</{t}>");
        }
        CE::Data(_) => { let _ = write!(s, "<Class IRI=\"{THING}\"/>"); }
    }
}
/// OWL/XML for one axiom (used for OWLlink `Tell` requests and the OWL export).
pub fn w_axiom(s: &mut String, a: &Axiom) {
    let open = |s: &mut String, t: &str| { let _ = write!(s, "<{t}>"); };
    let close = |s: &mut String, t: &str| { let _ = write!(s, "</{t}>"); };
    macro_rules! wrap { ($t:expr, $body:block) => {{ open(s, $t); $body; close(s, $t); }} }
    match a {
        Axiom::Declaration(t, i) => { let _ = write!(s, "<Declaration><{t} IRI=\"{}\"/></Declaration>", esc(i)); }
        Axiom::SubClassOf(x, y) => wrap!("SubClassOf", { w_ce(s, x); w_ce(s, y); }),
        Axiom::EquivalentClasses(xs) => wrap!("EquivalentClasses", { for x in xs { w_ce(s, x) } }),
        Axiom::DisjointClasses(xs) => wrap!("DisjointClasses", { for x in xs { w_ce(s, x) } }),
        Axiom::DisjointUnion(c, xs) => wrap!("DisjointUnion", { w_ce(s, &CE::Class(c.clone())); for x in xs { w_ce(s, x) } }),
        Axiom::ClassAssertion(c, i) => wrap!("ClassAssertion", { w_ce(s, c); w_ind(s, i); }),
        Axiom::PropertyAssertion(p, a1, b1) => wrap!("ObjectPropertyAssertion", { w_prop(s, p); w_ind(s, a1); w_ind(s, b1); }),
        Axiom::NegativePropertyAssertion(p, a1, b1) => wrap!("NegativeObjectPropertyAssertion", { w_prop(s, p); w_ind(s, a1); w_ind(s, b1); }),
        Axiom::SameIndividual(is) => wrap!("SameIndividual", { for i in is { w_ind(s, i) } }),
        Axiom::DifferentIndividuals(is) => wrap!("DifferentIndividuals", { for i in is { w_ind(s, i) } }),
        Axiom::SubPropertyOf(p, q) => wrap!("SubObjectPropertyOf", { w_prop(s, p); w_prop(s, q); }),
        Axiom::SubPropertyChain(ps, q) => wrap!("SubObjectPropertyOf", { open(s, "ObjectPropertyChain"); for p in ps { w_prop(s, p) } close(s, "ObjectPropertyChain"); w_prop(s, q); }),
        Axiom::EquivalentProperties(ps) => wrap!("EquivalentObjectProperties", { for p in ps { w_prop(s, p) } }),
        Axiom::DisjointProperties(ps) => wrap!("DisjointObjectProperties", { for p in ps { w_prop(s, p) } }),
        Axiom::InverseProperties(p, q) => wrap!("InverseObjectProperties", { w_prop(s, p); w_prop(s, q); }),
        Axiom::Domain(p, c) => wrap!("ObjectPropertyDomain", { w_prop(s, p); w_ce(s, c); }),
        Axiom::Range(p, c) => wrap!("ObjectPropertyRange", { w_prop(s, p); w_ce(s, c); }),
        Axiom::Functional(p) => wrap!("FunctionalObjectProperty", { w_prop(s, p); }),
        Axiom::InverseFunctional(p) => wrap!("InverseFunctionalObjectProperty", { w_prop(s, p); }),
        Axiom::Reflexive(p) => wrap!("ReflexiveObjectProperty", { w_prop(s, p); }),
        Axiom::Irreflexive(p) => wrap!("IrreflexiveObjectProperty", { w_prop(s, p); }),
        Axiom::Symmetric(p) => wrap!("SymmetricObjectProperty", { w_prop(s, p); }),
        Axiom::Asymmetric(p) => wrap!("AsymmetricObjectProperty", { w_prop(s, p); }),
        Axiom::Transitive(p) => wrap!("TransitiveObjectProperty", { w_prop(s, p); }),
        Axiom::Other(..) => {}
    }
}
pub fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;") }

/// A complete OWL/XML document for the given axioms.
pub fn owlxml_document(iri: &str, axioms: &[Axiom]) -> String {
    let mut s = format!("<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"{}\">\n", esc(iri));
    for a in axioms { w_axiom(&mut s, a); s.push('\n'); }
    s + "</Ontology>\n"
}
