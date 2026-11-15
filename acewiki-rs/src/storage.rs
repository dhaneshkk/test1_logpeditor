//! `FileBasedStorage` port: reads and writes AceWiki's on-disk format unchanged, so existing
//! wikis are imported as-is and remain readable by Java AceWiki.
//!
//! * directory form: `<data>/<wiki>/<id>` — one file per ontology element
//! * single-file form: `<data>/<wiki>.acewikidata` — blank-line separated, `<id>` line first
//!
//! Element file:  `type:<kind>` / `words:<w1>;<w2>;…` / statements (`| ` integrated sentence,
//! `# ` non-integrated sentence, `c ` comment with `~n` = newline and `~t` = `~`).

use crate::model::{Element, Kind, Statement, Wiki, Words};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Storage { pub dir: PathBuf }

impl Storage {
    pub fn new(dir: impl Into<PathBuf>) -> Self { Storage { dir: dir.into() } }

    pub fn load(&self, name: &str) -> Result<Wiki> {
        let mut wiki = Wiki { name: name.to_string(), ..Default::default() };
        let d = self.dir.join(name);
        let f = self.dir.join(format!("{name}.acewikidata"));
        if d.is_dir() {
            for ent in std::fs::read_dir(&d)? {
                let p = ent?.path();
                let Some(id) = p.file_name().and_then(|x| x.to_str()).and_then(|x| x.parse::<u64>().ok()) else { continue };
                let s = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
                if let Some(e) = parse_element(&s, id) { wiki.elements.insert(e.id, e); }
            }
        } else if f.is_file() {
            let s = std::fs::read_to_string(&f)?;
            let (mut buf, mut id): (String, Option<u64>) = (String::new(), None);
            for line in s.lines().chain(std::iter::once("")) {
                if line.trim().is_empty() {
                    if !buf.is_empty() { if let Some(e) = parse_element(&buf, id.unwrap_or(u64::MAX)) { wiki.elements.insert(e.id, e); } }
                    buf.clear(); id = None;
                } else if buf.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
                    id = line.parse().ok();
                } else if line.starts_with('%') {
                } else { buf += line; buf.push('\n'); }
            }
        }
        if !wiki.elements.contains_key(&0) {
            wiki.elements.insert(0, Element { id: 0, kind: Kind::MainPage, words: Words::Main("acewiki_page_main".into()), article: vec![] });
        }
        wiki.reindex();
        Ok(wiki)
    }

    pub fn save_element(&self, wiki: &str, e: &Element) -> Result<()> {
        let d = self.dir.join(wiki);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(e.id.to_string()), serialize_element(e))?;
        Ok(())
    }
    pub fn delete_element(&self, wiki: &str, id: u64) -> Result<()> {
        let p = self.dir.join(wiki).join(id.to_string());
        if p.exists() { std::fs::remove_file(p)?; }
        Ok(())
    }
    pub fn save_all(&self, w: &Wiki) -> Result<()> { for e in w.elements.values() { self.save_element(&w.name, e)?; } Ok(()) }
    pub fn path(&self) -> &Path { &self.dir }
}

pub fn parse_element(s: &str, id: u64) -> Option<Element> {
    let mut lines: Vec<&str> = s.split('\n').collect();
    if lines.last() == Some(&"") { lines.pop(); }
    let first = lines.first()?;
    let ty = first.strip_prefix("type:")?;
    let kind = Kind::parse(ty)?;
    let mut i = 1;
    let (words, id) = if kind == Kind::MainPage {
        if lines.get(1).is_some_and(|l| l.starts_with("words:")) { i = 2; }
        (Words::Main("acewiki_page_main".into()), 0)
    } else {
        let w = lines.get(1)?.strip_prefix("words:")?;
        i = 2;
        (Words::parse(&kind, w), id)
    };
    let mut article = Vec::new();
    for l in &lines[i..] {
        if l.len() < 2 { continue; }
        let rest = &l[2..];
        match &l[..2] {
            "| " => article.push(Statement::Sentence { serialized: rest.to_string(), integrated: true }),
            "# " => article.push(Statement::Sentence { serialized: rest.to_string(), integrated: false }),
            "c " => article.push(Statement::Comment(rest.replace("~n", "\n").replace("~t", "~"))),
            _ => {}
        }
    }
    Some(Element { id, kind, words, article })
}

pub fn serialize_element(e: &Element) -> String {
    let mut s = format!("type:{}\n", e.kind.internal());
    let w = e.words.serialize();
    if !w.is_empty() { s += &format!("words:{w}\n"); }
    for st in &e.article {
        match st {
            Statement::Comment(c) => s += &format!("c {}\n", c.replace('~', "~t").replace('\n', "~n")),
            Statement::Sentence { serialized, integrated } => s += &format!("{} {serialized}\n", if *integrated { "|" } else { "#" }),
        }
    }
    s
}
