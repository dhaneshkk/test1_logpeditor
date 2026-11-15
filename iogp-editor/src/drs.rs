//! Parser for APE's DRS output (a Prolog term), e.g.
//! `drs([],[=>(drs([A,B,C],[object(A,worker,countable,na,eq,1)-1/3, ...]),drs([...],[...]))])`.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Atom(String),
    Var(String),
    Num(f64),
    Str(String),
    Compound(String, Vec<Term>),
    List(Vec<Term>),
}

impl Term {
    pub fn functor(&self) -> Option<(&str, usize)> {
        match self { Term::Compound(f, a) => Some((f, a.len())), Term::Atom(a) => Some((a, 0)), _ => None }
    }
    pub fn args(&self) -> &[Term] { match self { Term::Compound(_, a) => a, _ => &[] } }
    pub fn atom(&self) -> Option<&str> { match self { Term::Atom(a) => Some(a), _ => None } }
    pub fn var(&self) -> Option<&str> { match self { Term::Var(v) => Some(v), _ => None } }
    pub fn num(&self) -> Option<f64> { match self { Term::Num(n) => Some(*n), _ => None } }
    /// Strip APE's sentence/token annotation `Cond-S/T`.
    pub fn strip_index(&self) -> &Term {
        match self { Term::Compound(f, a) if f == "-" && a.len() == 2 && matches!(a[1].functor(), Some(("/", 2))) => &a[0], t => t }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Atom(a) => if a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && a.starts_with(|c: char| c.is_ascii_lowercase()) { write!(f, "{a}") } else { write!(f, "'{a}'") },
            Term::Var(v) => write!(f, "{v}"),
            Term::Num(n) => if n.fract() == 0.0 { write!(f, "{}", *n as i64) } else { write!(f, "{n}") },
            Term::Str(s) => write!(f, "\"{s}\""),
            Term::List(l) => { write!(f, "[")?; for (i, x) in l.iter().enumerate() { if i > 0 { write!(f, ",")?; } write!(f, "{x}")?; } write!(f, "]") }
            Term::Compound(n, a) => { write!(f, "{n}(")?; for (i, x) in a.iter().enumerate() { if i > 0 { write!(f, ",")?; } write!(f, "{x}")?; } write!(f, ")") }
        }
    }
}

struct P<'a> { s: &'a [u8], i: usize }

impl<'a> P<'a> {
    fn ws(&mut self) { while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() { self.i += 1; } }
    fn peek(&mut self) -> Option<u8> { self.ws(); self.s.get(self.i).copied() }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) { self.i += 1; Ok(()) } else { Err(format!("expected '{}' at {}", c as char, self.i)) }
    }
    fn args(&mut self) -> Result<Vec<Term>, String> {
        let mut v = vec![self.term()?];
        while self.peek() == Some(b',') { self.i += 1; v.push(self.term()?); }
        Ok(v)
    }
    /// term := primary { ('-' | '/') primary }   (APE only uses these two infix operators)
    fn term(&mut self) -> Result<Term, String> {
        let mut t = self.primary()?;
        loop {
            match self.peek() {
                Some(b'/') => { self.i += 1; let r = self.primary()?; t = Term::Compound("/".into(), vec![t, r]); }
                Some(b'-') if self.s.get(self.i + 1) != Some(&b'(') && !matches!(self.s.get(self.i + 1), Some(b'>')) => {
                    self.i += 1; let r = self.term_no_minus()?; t = Term::Compound("-".into(), vec![t, r]);
                }
                _ => return Ok(t),
            }
        }
    }
    fn term_no_minus(&mut self) -> Result<Term, String> {
        let mut t = self.primary()?;
        while self.peek() == Some(b'/') { self.i += 1; let r = self.primary()?; t = Term::Compound("/".into(), vec![t, r]); }
        Ok(t)
    }
    fn primary(&mut self) -> Result<Term, String> {
        let c = self.peek().ok_or("unexpected end of DRS")?;
        if c == b'[' {
            self.i += 1;
            if self.peek() == Some(b']') { self.i += 1; return Ok(Term::List(vec![])); }
            let v = self.args()?; self.eat(b']')?; return Ok(Term::List(v));
        }
        if c == b'(' { self.i += 1; let t = self.term()?; self.eat(b')')?; return Ok(t); }
        let start = self.i;
        let name: String = if c == b'\'' {
            self.i += 1; let mut s = String::new();
            loop {
                let ch = *self.s.get(self.i).ok_or("unterminated quoted atom")?;
                self.i += 1;
                if ch == b'\'' { if self.s.get(self.i) == Some(&b'\'') { s.push('\''); self.i += 1; continue; } break; }
                if ch == b'\\' { if let Some(&n) = self.s.get(self.i) { s.push(n as char); self.i += 1; continue; } }
                s.push(ch as char);
            }
            s
        } else if c == b'"' {
            self.i += 1; let st = self.i;
            while self.s.get(self.i) != Some(&b'"') { if self.i >= self.s.len() { return Err("unterminated string".into()); } self.i += 1; }
            let s = String::from_utf8_lossy(&self.s[st..self.i]).into_owned(); self.i += 1;
            return Ok(Term::Str(s));
        } else if c.is_ascii_digit() {
            while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || (self.s[self.i] == b'.' && self.s.get(self.i + 1).map_or(false, |d| d.is_ascii_digit()))) { self.i += 1; }
            let n: f64 = std::str::from_utf8(&self.s[start..self.i]).unwrap().parse().map_err(|e| format!("{e}"))?;
            return Ok(Term::Num(n));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') { self.i += 1; }
            let w = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
            if (c.is_ascii_uppercase() || c == b'_') && self.s.get(self.i) != Some(&b'(') { return Ok(Term::Var(w)); }
            w
        } else {
            // symbolic functor: =>, -, v is alphabetic, ~ etc.
            while self.i < self.s.len() && b"=>-~<+*\\".contains(&self.s[self.i]) { self.i += 1; }
            if self.i == start { return Err(format!("unexpected '{}' at {}", c as char, self.i)); }
            String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
        };
        if self.s.get(self.i) == Some(&b'(') {
            self.i += 1; let a = self.args()?; self.eat(b')')?;
            return Ok(Term::Compound(name, a));
        }
        Ok(Term::Atom(name))
    }
}

pub fn parse(s: &str) -> Result<Term, String> {
    let mut p = P { s: s.trim().trim_end_matches('.').as_bytes(), i: 0 };
    let t = p.term()?;
    if p.peek().is_some() { return Err(format!("trailing input at {}", p.i)); }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ape_drs() {
        let s = "drs([],[=>(drs([A,B,C],[object(A,worker,countable,na,eq,1)-1/3,object(B,'height-zone',countable,na,eq,1)-1/7,predicate(C,be,A)-1/4,modifier_pp(C,in,B)-1/5,-(drs([D,E,F],[object(D,harness,countable,na,eq,1)-1/13,object(E,second,countable,na,geq,3)-1/18,predicate(F,wear,A,D)-1/11,modifier_pp(F,for,E)-1/14]))]),drs([G,H],[object(G,'working-at-height-alert',countable,na,eq,1)-1/24,property(G,critical,pos)-1/23,predicate(H,raise,named('VisionGuard'),G)-1/21,modifier_pp(H,for,A)-1/25]))])";
        let t = parse(s).unwrap();
        assert_eq!(t.functor(), Some(("drs", 2)));
        let conds = match &t.args()[1] { Term::List(l) => l, _ => panic!() };
        assert_eq!(conds[0].strip_index().functor(), Some(("=>", 2)));
        let ante = &conds[0].strip_index().args()[0];
        let ac = match &ante.args()[1] { Term::List(l) => l, _ => panic!() };
        assert_eq!(ac[1].strip_index().args()[1], Term::Atom("height-zone".into()));
        assert_eq!(ac[4].strip_index().functor(), Some(("-", 1)));
        assert_eq!(format!("{}", ac[0].strip_index()), "object(A,worker,countable,na,eq,1)");
    }
}
