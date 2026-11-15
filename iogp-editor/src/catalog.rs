//! Detection vocabulary: what VisionGuard can observe, and the ACE lexicon generated from it.
//!
//! The catalog is the single source of truth. It generates the user lexicon that APE parses
//! with (`ulextext`, with `noclex=on` so no other content word is accepted), and the lexicon of
//! the predictive editor, so users are only offered words the detectors can see.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Class { pub noun: String, pub plural: String, pub detector: String, #[serde(default)] pub human: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Named { pub noun: String, pub plural: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relation { pub verb: String, pub third: String, pub past_participle: String, pub detector: String, pub object_kinds: Vec<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spatial { pub prep: String, pub detector: String, #[serde(default)] pub default_metres: Option<f64> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signal { pub noun: String, pub plural: String, pub source: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert { pub noun: String, pub lsr: u8, pub rule: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unit { pub noun: String, pub plural: String, pub seconds: f64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Defaults { pub grace_seconds: f64, pub cooldown_seconds: f64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Template { pub lsr: u8, pub text: String }
/// A camera of the site, named as an ACE proper name (e.g. `CAM-03`); rules can scope a zone to it:
/// "a height-zone of CAM-03".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Camera { pub name: String, #[serde(default)] pub description: String }
/// A zone drawn on a camera, named so that rules can refer to it: "a worker is in Rig-Floor-East".
/// At runtime the zone object's `id` is this name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneInstance { pub name: String, pub zone: String, #[serde(default)] pub camera: Option<String>, #[serde(default)] pub description: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub agent: String,
    pub classes: Vec<Class>,
    pub zones: Vec<Named>,
    pub relations: Vec<Relation>,
    pub spatial: Vec<Spatial>,
    pub signals: Vec<Signal>,
    pub alerts: Vec<Alert>,
    pub severities: Vec<String>,
    pub units: Vec<Unit>,
    pub defaults: Defaults,
    #[serde(default)]
    pub templates: Vec<Template>,
    #[serde(default)]
    pub cameras: Vec<Camera>,
    #[serde(default)]
    pub zone_instances: Vec<ZoneInstance>,
}

pub use iogp_runtime::NounKind;

pub const BUILTIN: &str = include_str!("../data/vg360_catalog.json");

impl Catalog {
    pub fn builtin() -> Catalog { serde_json::from_str(BUILTIN).expect("built-in catalog is valid") }
    pub fn from_json(s: &str) -> anyhow::Result<Catalog> {
        let c: Catalog = serde_json::from_str(s)?;
        c.check()?;
        Ok(c)
    }
    /// Words must be ACE content words: letters, digits and hyphens, and each noun used once.
    pub fn check(&self) -> anyhow::Result<()> {
        let re = regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for (n, p) in self.nouns() {
            for w in [&n, &p] {
                if !re.is_match(w) { anyhow::bail!("catalog: '{w}' is not a valid content word (lower-case letters, digits, hyphens)"); }
            }
            if !seen.insert(n.clone()) { anyhow::bail!("catalog: noun '{n}' is defined twice"); }
        }
        if self.alerts.is_empty() { anyhow::bail!("catalog: no alerts defined"); }
        let pn = regex::Regex::new(r"^[A-Z][A-Za-z0-9]*(-[A-Za-z0-9]+)*$").unwrap();
        let mut names = std::collections::BTreeSet::new();
        for n in self.cameras.iter().map(|c| &c.name).chain(self.zone_instances.iter().map(|z| &z.name)) {
            if !pn.is_match(n) { anyhow::bail!("catalog: '{n}' is not a valid name (start with a capital letter; letters, digits, hyphens)"); }
            if !names.insert(n.clone()) || *n == self.agent { anyhow::bail!("catalog: name '{n}' is defined twice"); }
        }
        for z in &self.zone_instances {
            if !self.zones.iter().any(|x| x.noun == z.zone) { anyhow::bail!("catalog: zone instance '{}' has unknown zone type '{}'", z.name, z.zone); }
            if let Some(c) = &z.camera { if !self.cameras.iter().any(|x| &x.name == c) { anyhow::bail!("catalog: zone instance '{}' is on unknown camera '{c}'", z.name); } }
        }
        Ok(())
    }
    fn nouns(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.classes.iter().map(|c| (c.noun.clone(), c.plural.clone())).collect();
        v.extend(self.zones.iter().map(|z| (z.noun.clone(), z.plural.clone())));
        v.extend(self.signals.iter().map(|s| (s.noun.clone(), s.plural.clone())));
        v.extend(self.alerts.iter().map(|a| (a.noun.clone(), format!("{}s", a.noun))));
        v.extend(self.units.iter().map(|u| (u.noun.clone(), u.plural.clone())));
        v
    }
    pub fn noun(&self, lemma: &str) -> Option<NounKind> {
        if let Some(c) = self.classes.iter().find(|c| c.noun == lemma) { return Some(NounKind::Class { detector: c.detector.clone(), human: c.human }); }
        if self.zones.iter().any(|z| z.noun == lemma) { return Some(NounKind::Zone); }
        if let Some(s) = self.signals.iter().find(|s| s.noun == lemma) { return Some(NounKind::Signal { source: s.source.clone() }); }
        if let Some(a) = self.alerts.iter().find(|a| a.noun == lemma) { return Some(NounKind::Alert { lsr: a.lsr, rule: a.rule.clone() }); }
        if let Some(u) = self.units.iter().find(|u| u.noun == lemma) { return Some(NounKind::Unit { seconds: u.seconds }); }
        None
    }
    pub fn camera(&self, name: &str) -> Option<&Camera> { self.cameras.iter().find(|c| c.name == name) }
    pub fn zone_instance(&self, name: &str) -> Option<&ZoneInstance> { self.zone_instances.iter().find(|z| z.name == name) }
    /// Signal relations by their third-person form ("has" -> have), for "unless X has a Y".
    pub fn relation_by_third(&self, third: &str) -> Option<&Relation> { self.relations.iter().find(|r| r.third == third) }
    pub fn relation(&self, verb: &str) -> Option<&Relation> { self.relations.iter().find(|r| r.verb == verb) }
    pub fn spatial(&self, prep: &str) -> Option<&Spatial> { self.spatial.iter().find(|s| s.prep == prep) }

    /// ACE lexicon entries (APE `ulex` / ACE Editor format), one per line.
    pub fn lexicon(&self) -> Vec<String> {
        let mut v = Vec::new();
        let q = quote;
        v.push(format!("pn_sg({}, {}, neutr).", q(&self.agent), q(&self.agent)));
        for n in self.cameras.iter().map(|c| &c.name).chain(self.zone_instances.iter().map(|z| &z.name)) {
            v.push(format!("pn_sg({}, {}, neutr).", q(n), q(n)));
        }
        for c in &self.classes {
            let g = if c.human { "human" } else { "neutr" };
            v.push(format!("noun_sg({}, {}, {g}).", q(&c.noun), q(&c.noun)));
            v.push(format!("noun_pl({}, {}, {g}).", q(&c.plural), q(&c.noun)));
        }
        for (n, p) in self.zones.iter().map(|z| (&z.noun, &z.plural)).chain(self.signals.iter().map(|s| (&s.noun, &s.plural)))
            .chain(self.units.iter().map(|u| (&u.noun, &u.plural))) {
            v.push(format!("noun_sg({}, {}, neutr).", q(n), q(n)));
            v.push(format!("noun_pl({}, {}, neutr).", q(p), q(n)));
        }
        for a in &self.alerts { v.push(format!("noun_sg({}, {}, neutr).", q(&a.noun), q(&a.noun))); }
        for r in &self.relations {
            v.push(format!("tv_finsg({}, {}).", q(&r.third), q(&r.verb)));
            v.push(format!("tv_infpl({}, {}).", q(&r.verb), q(&r.verb)));
            v.push(format!("tv_pp({}, {}).", q(&r.past_participle), q(&r.verb)));
        }
        // the consequence verb: "VisionGuard raises a ... alert"
        v.push("tv_finsg(raises, raise).".into());
        v.push("tv_infpl(raise, raise).".into());
        v.push("tv_pp(raised, raise).".into());
        for s in &self.spatial { v.push(format!("prep({}, {}).", q(&s.prep), q(&s.prep))); }
        v.push("prep(for, for).".into());
        for s in &self.severities { v.push(format!("adj_itr({}, {}).", q(s), q(s))); }
        v
    }
    pub fn lexicon_text(&self) -> String { self.lexicon().join("\n") + "\n" }

    pub fn summary(&self) -> BTreeMap<&'static str, Vec<String>> {
        let mut m = BTreeMap::new();
        m.insert("objects", self.classes.iter().map(|c| format!("{} → {}", c.noun, c.detector)).collect());
        m.insert("zones", self.zones.iter().map(|z| z.noun.clone()).collect());
        m.insert("relations", self.relations.iter().map(|r| format!("{} → {}", r.verb, r.detector)).collect());
        m.insert("positions", self.spatial.iter().map(|s| format!("{} → {}", s.prep, s.detector)).collect());
        m.insert("signals", self.signals.iter().map(|s| format!("{} ← {}", s.noun, s.source)).collect());
        m.insert("alerts", self.alerts.iter().map(|a| format!("{} → LSR {} {}", a.noun, a.lsr, a.rule)).collect());
        m
    }
}

pub fn quote(w: &str) -> String {
    if regex::Regex::new(r"^[a-z][A-Za-z0-9_]*$").unwrap().is_match(w) { w.to_string() } else { format!("'{}'", w.replace('\'', "\\'")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_catalog_is_valid_and_complete() {
        let c = Catalog::builtin();
        c.check().unwrap();
        assert_eq!(c.alerts.len(), 9, "one alert per IOGP Life-Saving Rule");
        let lsrs: std::collections::BTreeSet<u8> = c.alerts.iter().map(|a| a.lsr).collect();
        assert_eq!(lsrs, (1..=9).collect());
        assert_eq!(c.templates.len(), 9);
        let lex = c.lexicon_text();
        assert!(lex.contains("noun_sg(worker, worker, human)."));
        assert!(lex.contains("noun_sg('suspended-load', 'suspended-load', neutr)."));
        assert!(lex.contains("pn_sg('VisionGuard', 'VisionGuard', neutr)."));
        assert_eq!(c.noun("height-zone"), Some(NounKind::Zone));
    }
}
