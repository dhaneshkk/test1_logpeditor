//! Rule store with versioning, an approval workflow and a tamper-evident audit log.
//!
//! * Every save creates a new immutable **version** of a rule. A version moves
//!   Draft → Submitted → Approved | Rejected; an approved version becomes the rule's **active**
//!   version (the one exported to the detection framework). Older drafts / submissions /
//!   approvals of the same rule become Superseded; an active rule can be Retired.
//! * **Four-eyes**: a version can only be approved or rejected by a user with the approver role
//!   who is not its author. Enforced here, not in the UI.
//! * Every action is appended to an **audit log** (JSON lines). Each entry carries the SHA-256 of
//!   the previous entry, so editing or deleting any past entry breaks the chain
//!   (`iogp audit verify`). See `formal/IogpAlerts.lean` §7–§8.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA: &str = "iogp-rule-store/v2";
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Draft, Submitted, Approved, Rejected, Superseded, Retired }

impl Status {
    pub fn label(self) -> &'static str {
        match self { Status::Draft => "draft", Status::Submitted => "awaiting approval", Status::Approved => "approved", Status::Rejected => "rejected", Status::Superseded => "superseded", Status::Retired => "retired" }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version {
    pub v: u32,
    pub text: String,
    pub text_sha256: String,
    pub author: String,
    pub created: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub submitted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub reviewer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub reviewed: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")] pub note: String,
    /// what the author meant, in their own words (collected for vocabulary work)
    #[serde(default, skip_serializing_if = "String::is_empty")] pub intent: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleRec {
    pub id: String,
    pub versions: Vec<Version>,
    /// the approved version in force
    #[serde(default)] pub active: Option<u32>,
}

impl RuleRec {
    pub fn latest(&self) -> &Version { self.versions.last().expect("a rule has at least one version") }
    pub fn version(&self, v: u32) -> Option<&Version> { self.versions.iter().find(|x| x.v == v) }
    pub fn active_version(&self) -> Option<&Version> { self.active.and_then(|v| self.version(v)) }
    fn version_mut(&mut self, v: u32) -> Option<&mut Version> { self.versions.iter_mut().find(|x| x.v == v) }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { Author, Approver, Admin }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User { pub name: String, pub roles: Vec<Role> }
impl User {
    pub fn can(&self, r: Role) -> bool { self.roles.contains(&r) || self.roles.contains(&Role::Admin) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub ts: String,
    pub user: String,
    pub action: String,
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")] pub detail: String,
    #[serde(default, skip_serializing_if = "String::is_empty")] pub text_sha256: String,
    pub prev: String,
    pub hash: String,
}

impl AuditEntry {
    /// Hash of the entry with `hash` blanked: SHA-256 over `prev` and the canonical JSON body.
    pub fn compute_hash(&self) -> String {
        let mut e = self.clone();
        e.hash = String::new();
        sha256_hex(format!("{}\n{}", self.prev, serde_json::to_string(&e).unwrap()).as_bytes())
    }
}

pub fn sha256_hex(b: &[u8]) -> String { Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect() }

/// RFC 3339 UTC timestamp from the system clock (no external time crate).
pub fn now() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (H. Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{dd:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[derive(Debug)]
pub struct Denied(pub String);
impl std::fmt::Display for Denied { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) } }
fn deny<T>(m: impl Into<String>) -> Result<T, Denied> { Err(Denied(m.into())) }

/// Rules + audit log, persisted next to each other (`rules.json`, `rules.audit.jsonl`).
pub struct Store {
    pub rules: Vec<RuleRec>,
    path: Option<PathBuf>,
    audit_path: Option<PathBuf>,
    pub audit_head: String,
    pub audit_seq: u64,
}

impl Store {
    pub fn in_memory() -> Store { Store { rules: vec![], path: None, audit_path: None, audit_head: GENESIS.into(), audit_seq: 0 } }

    pub fn audit_path_for(p: &Path) -> PathBuf {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("rules".into());
        p.with_file_name(format!("{stem}.audit.jsonl"))
    }

    /// Open (or create) a store. A v1 rule set (`{"rules":[{"id","text"}]}`) is imported as
    /// submitted versions by `legacy-import`, so nothing goes live without an approval.
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let audit_path = Self::audit_path_for(path);
        let mut st = Store { rules: vec![], path: Some(path.to_path_buf()), audit_path: Some(audit_path.clone()), audit_head: GENESIS.into(), audit_seq: 0 };
        if audit_path.exists() {
            let (n, head) = verify_audit(&audit_path).map_err(|e| anyhow::anyhow!("audit log {}: {e}", audit_path.display()))?;
            st.audit_seq = n; st.audit_head = head;
        }
        if path.exists() {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            if v["schema"] == STORE_SCHEMA {
                st.rules = serde_json::from_value(v["rules"].clone())?;
            } else {
                let old: Vec<serde_json::Value> = serde_json::from_value(v["rules"].clone()).unwrap_or_default();
                for r in old {
                    let (id, text) = (r["id"].as_str().unwrap_or("").to_string(), r["text"].as_str().unwrap_or("").to_string());
                    if id.is_empty() { continue; }
                    let u = User { name: "legacy-import".into(), roles: vec![Role::Author] };
                    let v = st.save_draft(&u, &id, &text, "imported from a v1 rule set").map_err(|e| anyhow::anyhow!("{e}"))?;
                    st.submit(&u, &id, v).map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                st.persist()?;
            }
        }
        Ok(st)
    }

    fn persist(&self) -> anyhow::Result<()> {
        if let Some(p) = &self.path {
            let tmp = p.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(&serde_json::json!({"schema": STORE_SCHEMA, "rules": self.rules}))?)?;
            std::fs::rename(tmp, p)?;
        }
        Ok(())
    }

    fn audit(&mut self, user: &str, action: &str, rule: &str, version: Option<u32>, detail: &str, text_sha256: &str) {
        let mut e = AuditEntry { seq: self.audit_seq + 1, ts: now(), user: user.into(), action: action.into(), rule: rule.into(), version,
            detail: detail.into(), text_sha256: text_sha256.into(), prev: self.audit_head.clone(), hash: String::new() };
        e.hash = e.compute_hash();
        if let Some(p) = &self.audit_path {
            let line = serde_json::to_string(&e).unwrap();
            let ok = std::fs::OpenOptions::new().create(true).append(true).open(p).and_then(|mut f| { writeln!(f, "{line}")?; f.sync_data() });
            if let Err(err) = ok { eprintln!("audit log write failed: {err}"); }
        }
        self.audit_seq = e.seq;
        self.audit_head = e.hash;
    }

    pub fn get(&self, id: &str) -> Option<&RuleRec> { self.rules.iter().find(|r| r.id == id) }
    fn get_mut(&mut self, id: &str) -> Option<&mut RuleRec> { self.rules.iter_mut().find(|r| r.id == id) }

    pub fn next_id(&self) -> String { (1..).map(|i| format!("R{i}")).find(|id| self.get(id).is_none()).unwrap() }

    /// New draft version (the caller has checked that the text compiles).
    pub fn save_draft(&mut self, u: &User, id: &str, text: &str, intent: &str) -> Result<u32, Denied> {
        if !u.can(Role::Author) { return deny(format!("{} may not write rules (needs the author role)", u.name)); }
        let ts = now();
        let ver = Version { v: 0, text: text.into(), text_sha256: sha256_hex(text.as_bytes()), author: u.name.clone(), created: ts, status: Status::Draft,
            submitted: None, reviewer: None, reviewed: None, note: String::new(), intent: intent.into() };
        let v = match self.get_mut(id) {
            Some(r) => {
                // a new draft supersedes earlier drafts and pending submissions (the approved version stays active)
                for x in r.versions.iter_mut() { if matches!(x.status, Status::Draft | Status::Submitted) { x.status = Status::Superseded; } }
                let v = r.latest().v + 1;
                r.versions.push(Version { v, ..ver });
                v
            }
            None => { self.rules.push(RuleRec { id: id.into(), versions: vec![Version { v: 1, ..ver }], active: None }); 1 }
        };
        let sha = sha256_hex(text.as_bytes());
        self.audit(&u.name, "draft", id, Some(v), &if intent.is_empty() { text.to_string() } else { format!("{text}  [intent: {intent}]") }, &sha);
        let _ = self.persist();
        Ok(v)
    }

    pub fn submit(&mut self, u: &User, id: &str, v: u32) -> Result<(), Denied> {
        let name = u.name.clone();
        let r = self.get_mut(id).ok_or(Denied(format!("no rule {id}")))?;
        let ver = r.version_mut(v).ok_or(Denied(format!("no version {v} of {id}")))?;
        if ver.status != Status::Draft { return deny(format!("{id} v{v} is {}, only drafts can be submitted", ver.status.label())); }
        if ver.author != name && !u.can(Role::Admin) { return deny(format!("only the author ({}) can submit {id} v{v}", ver.author)); }
        ver.status = Status::Submitted;
        ver.submitted = Some(now());
        let sha = ver.text_sha256.clone();
        self.audit(&name, "submit", id, Some(v), "", &sha);
        let _ = self.persist();
        Ok(())
    }

    /// Approve (four-eyes: approver role, and not the author). The version becomes active.
    pub fn approve(&mut self, u: &User, id: &str, v: u32, note: &str) -> Result<(), Denied> {
        if !u.can(Role::Approver) { return deny(format!("{} may not approve rules (needs the approver role)", u.name)); }
        let name = u.name.clone();
        let r = self.get_mut(id).ok_or(Denied(format!("no rule {id}")))?;
        let ver = r.version(v).ok_or(Denied(format!("no version {v} of {id}")))?;
        if ver.status != Status::Submitted { return deny(format!("{id} v{v} is {}, only submitted versions can be approved", ver.status.label())); }
        if ver.author == name { return deny(format!("four-eyes rule: {name} wrote {id} v{v} and cannot approve it")); }
        if let Some(prev) = r.active { if let Some(p) = r.version_mut(prev) { p.status = Status::Superseded; } }
        let ver = r.version_mut(v).unwrap();
        ver.status = Status::Approved;
        ver.reviewer = Some(name.clone());
        ver.reviewed = Some(now());
        ver.note = note.into();
        let sha = ver.text_sha256.clone();
        r.active = Some(v);
        self.audit(&name, "approve", id, Some(v), note, &sha);
        let _ = self.persist();
        Ok(())
    }

    pub fn reject(&mut self, u: &User, id: &str, v: u32, note: &str) -> Result<(), Denied> {
        if !u.can(Role::Approver) { return deny(format!("{} may not review rules (needs the approver role)", u.name)); }
        if note.trim().is_empty() { return deny("say why the rule is rejected (the author sees the note)"); }
        let name = u.name.clone();
        let r = self.get_mut(id).ok_or(Denied(format!("no rule {id}")))?;
        let ver = r.version_mut(v).ok_or(Denied(format!("no version {v} of {id}")))?;
        if ver.status != Status::Submitted { return deny(format!("{id} v{v} is {}, only submitted versions can be rejected", ver.status.label())); }
        if ver.author == name { return deny(format!("four-eyes rule: {name} wrote {id} v{v} and cannot review it")); }
        ver.status = Status::Rejected;
        ver.reviewer = Some(name.clone());
        ver.reviewed = Some(now());
        ver.note = note.into();
        let sha = ver.text_sha256.clone();
        self.audit(&name, "reject", id, Some(v), note, &sha);
        let _ = self.persist();
        Ok(())
    }

    /// Take the active version out of force (it stays in the history).
    pub fn retire(&mut self, u: &User, id: &str, note: &str) -> Result<(), Denied> {
        if !u.can(Role::Approver) { return deny(format!("{} may not retire rules (needs the approver role)", u.name)); }
        let name = u.name.clone();
        let r = self.get_mut(id).ok_or(Denied(format!("no rule {id}")))?;
        let Some(v) = r.active else { return deny(format!("{id} has no active version")) };
        let ver = r.version_mut(v).unwrap();
        ver.status = Status::Retired;
        let sha = ver.text_sha256.clone();
        r.active = None;
        self.audit(&name, "retire", id, Some(v), note, &sha);
        let _ = self.persist();
        Ok(())
    }

    /// Active (approved) rules, as (id, version) pairs.
    pub fn active(&self) -> Vec<(&RuleRec, &Version)> { self.rules.iter().filter_map(|r| r.active_version().map(|v| (r, v))).collect() }

    /// Revision id of the deployed rule set: SHA-256 over the sorted (id, version, text hash).
    pub fn revision(&self) -> String {
        let mut items: Vec<String> = self.active().iter().map(|(r, v)| format!("{}@{}:{}", r.id, v.v, v.text_sha256)).collect();
        items.sort();
        sha256_hex(items.join("\n").as_bytes())
    }

    pub fn audit_entries(&self) -> Vec<AuditEntry> {
        self.audit_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
    }
    pub fn audit_file(&self) -> Option<&Path> { self.audit_path.as_deref() }
}

/// Check every entry's hash and its link to the previous entry. Returns (entries, head hash).
pub fn verify_audit(path: &Path) -> Result<(u64, String), String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut prev = GENESIS.to_string();
    let mut n = 0u64;
    for (i, l) in s.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let e: AuditEntry = serde_json::from_str(l).map_err(|e| format!("line {}: not an audit entry ({e})", i + 1))?;
        if e.seq != n + 1 { return Err(format!("line {}: sequence {} after {n} (an entry was removed or reordered)", i + 1, e.seq)); }
        if e.prev != prev { return Err(format!("entry {}: does not link to the previous entry (an earlier entry was changed or removed)", e.seq)); }
        if e.compute_hash() != e.hash { return Err(format!("entry {}: content does not match its hash (it was edited)", e.seq)); }
        prev = e.hash.clone();
        n = e.seq;
    }
    Ok((n, prev))
}

// ------------------------------------------------------------------------------------------- users

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserRec { pub name: String, pub roles: Vec<Role>, pub salt: String, pub password_sha256: String }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Users { pub users: Vec<UserRec> }

pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") { use std::io::Read; let _ = f.read_exact(&mut b); }
    else { let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(); b = sha256_hex(&t.to_le_bytes()).into_bytes()[..n].to_vec(); }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Users {
    pub fn load(p: &Path) -> anyhow::Result<Users> { Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?) }
    pub fn save(&self, p: &Path) -> anyhow::Result<()> { std::fs::write(p, serde_json::to_string_pretty(self)?)?; Ok(()) }
    pub fn set(&mut self, name: &str, roles: Vec<Role>, password: &str) {
        let salt = random_hex(16);
        let rec = UserRec { name: name.into(), roles, password_sha256: sha256_hex(format!("{salt}:{password}").as_bytes()), salt };
        self.users.retain(|u| u.name != name);
        self.users.push(rec);
    }
    pub fn check(&self, name: &str, password: &str) -> Option<User> {
        let u = self.users.iter().find(|u| u.name == name)?;
        (sha256_hex(format!("{}:{password}", u.salt).as_bytes()) == u.password_sha256).then(|| User { name: u.name.clone(), roles: u.roles.clone() })
    }
    /// Demo identities when no users file is configured (no passwords; clearly flagged in the UI).
    pub fn demo() -> Vec<User> {
        vec![User { name: "author-demo".into(), roles: vec![Role::Author] },
             User { name: "approver-demo".into(), roles: vec![Role::Approver] },
             User { name: "admin-demo".into(), roles: vec![Role::Admin] }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn u(n: &str, r: Role) -> User { User { name: n.into(), roles: vec![r] } }

    #[test]
    fn workflow_four_eyes_and_audit_chain() {
        let dir = std::env::temp_dir().join(format!("iogp-store-{}", random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("rules.json");
        let (alice, bob) = (u("alice", Role::Author), u("bob", Role::Approver));
        let mut s = Store::open(&p).unwrap();
        let v = s.save_draft(&alice, "R1", "If a worker is in a height-zone then VisionGuard raises a critical working-at-height-alert for the worker.", "").unwrap();
        assert!(s.approve(&bob, "R1", v, "").is_err(), "drafts cannot be approved");
        assert!(s.submit(&bob, "R1", v).is_err(), "only the author submits");
        s.submit(&alice, "R1", v).unwrap();
        assert!(s.approve(&alice, "R1", v, "").is_err(), "authors cannot approve");
        let carol = User { name: "alice".into(), roles: vec![Role::Approver, Role::Author] };
        assert!(s.approve(&carol, "R1", v, "").is_err(), "four-eyes even with the approver role");
        s.approve(&bob, "R1", v, "ok").unwrap();
        assert_eq!(s.get("R1").unwrap().active, Some(1));
        // a new draft does not change what is deployed until it is approved
        let rev = s.revision();
        let v2 = s.save_draft(&alice, "R1", "If a worker is on a ladder then VisionGuard raises a critical working-at-height-alert for the worker.", "ladders").unwrap();
        assert_eq!(s.revision(), rev);
        s.submit(&alice, "R1", v2).unwrap();
        assert!(s.reject(&bob, "R1", v2, "").is_err(), "a rejection needs a note");
        s.reject(&bob, "R1", v2, "needs a duration").unwrap();
        assert_eq!(s.get("R1").unwrap().active, Some(1));
        s.retire(&bob, "R1", "zone removed").unwrap();
        assert!(s.active().is_empty());
        // audit chain verifies; reopening restores state
        let ap = Store::audit_path_for(&p);
        assert_eq!(verify_audit(&ap).unwrap().0, 7);
        let s2 = Store::open(&p).unwrap();
        assert_eq!(s2.audit_seq, 7);
        assert_eq!(s2.get("R1").unwrap().versions.len(), 2);
        // tampering: change an approval note
        let txt = std::fs::read_to_string(&ap).unwrap().replacen("\"ok\"", "\"OK\"", 1);
        std::fs::write(&ap, &txt).unwrap();
        assert!(verify_audit(&ap).unwrap_err().contains("edited"));
        // deleting an entry
        let orig: Vec<String> = txt.replacen("\"OK\"", "\"ok\"", 1).lines().map(String::from).collect();
        std::fs::write(&ap, orig.iter().enumerate().filter(|(i, _)| *i != 2).map(|(_, l)| format!("{l}\n")).collect::<String>()).unwrap();
        assert!(verify_audit(&ap).is_err());
        assert!(Store::open(&p).is_err(), "a broken audit log refuses to open");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn timestamps_and_users() {
        let t = now();
        assert!(regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$").unwrap().is_match(&t), "{t}");
        let mut us = Users::default();
        us.set("dana", vec![Role::Approver], "s3cret");
        assert!(us.check("dana", "s3cret").is_some_and(|u| u.can(Role::Approver) && !u.can(Role::Author)));
        assert!(us.check("dana", "wrong").is_none());
        assert_ne!(us.users[0].password_sha256, sha256_hex(b"s3cret"), "salted");
    }
}
