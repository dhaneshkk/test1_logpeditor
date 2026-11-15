//! OWLlink (HTTP/XML binding) client — the fallback reasoner for ontologies outside the native
//! profile (any OWLlink server: HermiT/Pellet via the OWLlink API server, Konclude, FaCT++).

use super::reasoner::{Reasoner, Signature};
use super::{w_axiom, w_ce, Axiom, CE};
use anyhow::{anyhow, Result};
use std::collections::BTreeSet;

const NS: &str = "http://www.owllink.org/owllink#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

pub struct OwlLink { url: String, kb: String, agent: ureq::Agent, sig: Signature }

impl OwlLink {
    pub fn new(url: &str, kb: &str, axioms: Vec<Axiom>) -> Result<Self> {
        let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(120)).build();
        let o = OwlLink { url: url.to_string(), kb: format!("{kb}#kb"), agent, sig: Signature::of(&axioms) };
        o.request(&format!("<ReleaseKB kb=\"{}\"/>", o.kb)).ok();
        o.request(&format!("<CreateKB kb=\"{}\"/>", o.kb))?;
        let mut tell = format!("<Tell kb=\"{}\">", o.kb);
        for a in &axioms {
            let mut s = String::new();
            w_axiom(&mut s, a);
            if s.is_empty() { continue; }
            if let Some(i) = s.find(|c: char| c == '>' || c == '/') { s.insert_str(i, &format!(" xmlns=\"{OWL}\"")); }
            tell += &s;
        }
        tell += "</Tell>";
        let r = o.request(&tell)?;
        if r.contains("<Error") { return Err(anyhow!("OWLlink Tell failed: {r}")); }
        Ok(o)
    }
    fn request(&self, body: &str) -> Result<String> {
        let msg = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><RequestMessage xmlns=\"{NS}\" xmlns:owl=\"{OWL}\">{body}</RequestMessage>");
        let r = self.agent.post(&self.url).set("Content-Type", "text/xml").send_string(&msg).map_err(|e| anyhow!("OWLlink: {e}"))?;
        Ok(r.into_string()?)
    }
    fn iris(resp: &str, tag: &str) -> BTreeSet<String> {
        let re = regex::Regex::new(&format!(r#"<(?:owl:)?{tag}[^>]*IRI="([^"]+)""#)).unwrap();
        re.captures_iter(resp).map(|c| c[1].to_string()).collect()
    }
    fn boolean(resp: &str) -> bool { resp.contains("result=\"true\"") }
    fn ce_xml(c: &CE) -> String {
        let mut s = String::new();
        w_ce(&mut s, c);
        if let Some(i) = s.find(|c: char| c == '>' || c == '/') { s.insert_str(i, &format!(" xmlns=\"{OWL}\"")); }
        s
    }
}

impl Reasoner for OwlLink {
    fn name(&self) -> &'static str { "OWLlink" }
    fn unsupported(&self) -> Vec<String> { vec![] }
    fn is_consistent(&mut self) -> bool {
        self.request(&format!("<IsKBSatisfiable kb=\"{}\"/>", self.kb)).map(|r| Self::boolean(&r)).unwrap_or(false)
    }
    fn types(&mut self, ind: &str) -> BTreeSet<String> {
        let q = format!("<GetFlattenedTypes kb=\"{}\" direct=\"false\"><owl:NamedIndividual IRI=\"{ind}\"/></GetFlattenedTypes>", self.kb);
        self.request(&q).map(|r| Self::iris(&r, "Class")).unwrap_or_default()
    }
    fn instances(&mut self, c: &CE) -> BTreeSet<String> {
        let q = format!("<GetFlattenedInstances kb=\"{}\" direct=\"false\">{}</GetFlattenedInstances>", self.kb, Self::ce_xml(c));
        self.request(&q).map(|r| Self::iris(&r, "NamedIndividual")).unwrap_or_default()
    }
    fn super_classes(&mut self, c: &str) -> BTreeSet<String> {
        let q = format!("<GetSuperClasses kb=\"{}\" direct=\"false\"><owl:Class IRI=\"{c}\"/></GetSuperClasses>", self.kb);
        let mut s = self.request(&q).map(|r| Self::iris(&r, "Class")).unwrap_or_default();
        s.remove(c); s
    }
    fn sub_classes(&mut self, c: &str) -> BTreeSet<String> {
        let q = format!("<GetSubClasses kb=\"{}\" direct=\"false\"><owl:Class IRI=\"{c}\"/></GetSubClasses>", self.kb);
        let mut s = self.request(&q).map(|r| Self::iris(&r, "Class")).unwrap_or_default();
        s.remove(c); s
    }
    fn is_satisfiable(&mut self, c: &str) -> bool {
        if !self.sig.classes.contains(c) { return true; }
        let q = format!("<IsClassSatisfiable kb=\"{}\"><owl:Class IRI=\"{c}\"/></IsClassSatisfiable>", self.kb);
        self.request(&q).map(|r| Self::boolean(&r)).unwrap_or(true)
    }
}
