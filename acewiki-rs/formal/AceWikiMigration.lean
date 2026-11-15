/-
  AceWikiMigration.lean — machine-checked guarantees for the AceWiki (Java) → Rust port.
  Lean 4 core only (no Mathlib).  Check with:  lean formal/AceWikiMigration.lean

  §1  Storage fidelity: comment encoding (`~` ↦ `~t`, newline ↦ `~n`) followed by the Rust decoder
      `replace("~n","\n").replace("~t","~")` (two sequential passes, exactly as in storage.rs) is
      the identity, so importing/exporting wiki data never corrupts comments.
  §2  Sentence flags (engine.rs `finish`): reasonable ⇒ isOWL ⇒ isOWLSWRL for every APE result,
      under both global-restriction policies; a rule (`DLSafeRule`) is never reasonable.
  §3  Load-time demotion (engine.rs `load_all`): afterwards every integrated sentence is
      integrable, and demotion never integrates anything new.
  §4  Commit discipline (web `add_sentences` / `toggle_sentence`, Java `Ontology.commitSentence`):
      if the knowledge base is consistent and sentences are only integrated when the consistency
      check of KB ∪ {s} succeeds, the integrated knowledge base stays consistent after any sequence
      of edits (retractions included, assuming consistency is antitone).
  §5  Engine recycling (ape-rs `Ape::get_results`): with recycle_after = N > 0 the number of
      queries served by one Scryer machine is always < N, which bounds the per-machine
      assert/retract leak.
  §6  MCC: no disagreements ⇒ MCC = 1 (squared numerator = product of marginals); symmetric
      under swapping reference and candidate.
-/

/-! ## §1 Comment encoding round-trip -/

def encChar : Char → List Char
  | '~'  => ['~', 't']
  | '\n' => ['~', 'n']
  | c    => [c]

def encode : List Char → List Char
  | []      => []
  | c :: cs => encChar c ++ encode cs

/-- `replace("~n", "\n")`, left-to-right, non-overlapping (Rust `str::replace` semantics). -/
def passN : List Char → List Char
  | []          => []
  | [c]         => [c]
  | c :: d :: r => if c = '~' ∧ d = 'n' then '\n' :: passN r else c :: passN (d :: r)

/-- `replace("~t", "~")`. -/
def passT : List Char → List Char
  | []          => []
  | [c]         => [c]
  | c :: d :: r => if c = '~' ∧ d = 't' then '~' :: passT r else c :: passT (d :: r)

def decode (s : List Char) : List Char := passT (passN s)

theorem passN_cons (c : Char) (r : List Char) (h : c ≠ '~') : passN (c :: r) = c :: passN r := by
  cases r with
  | nil => rfl
  | cons d r => simp [passN, h]

theorem passT_cons (c : Char) (r : List Char) (h : c ≠ '~') : passT (c :: r) = c :: passT r := by
  cases r with
  | nil => rfl
  | cons d r => simp [passT, h]

/-- Intermediate form after the first pass: only `~` is still encoded. -/
def midChar : Char → List Char
  | '~' => ['~', 't']
  | c   => [c]

def mid : List Char → List Char
  | []      => []
  | c :: cs => midChar c ++ mid cs

theorem passN_encode (s : List Char) : passN (encode s) = mid s := by
  induction s with
  | nil => rfl
  | cons c cs ih =>
    by_cases h1 : c = '~'
    · subst h1; simp [encode, encChar, mid, midChar, passN]; rw [passN_cons _ _ (by decide), ih]
    by_cases h2 : c = '\n'
    · subst h2; simp [encode, encChar, mid, midChar, passN, ih]
    have he : encChar c = [c] := by unfold encChar; split <;> simp_all
    have hm : midChar c = [c] := by unfold midChar; split <;> simp_all
    simp only [encode, mid, he, hm, List.singleton_append]
    rw [passN_cons c _ h1, ih]

theorem passT_mid (s : List Char) : passT (mid s) = s := by
  induction s with
  | nil => rfl
  | cons c cs ih =>
    by_cases h1 : c = '~'
    · subst h1; simp [mid, midChar, passT, ih]
    have hm : midChar c = [c] := by unfold midChar; split <;> simp_all
    simp only [mid, hm, List.singleton_append]
    rw [passT_cons c _ h1, ih]

/-- Storage round trip for comments. -/
theorem decode_encode (s : List Char) : decode (encode s) = s := by
  unfold decode; rw [passN_encode, passT_mid]

/-- Encoded comments never contain a raw newline, so a comment always stays on one line of the
    article file (the line-oriented parser can never split it). -/
theorem encode_no_newline (s : List Char) : '\n' ∉ encode s := by
  induction s with
  | nil => simp [encode]
  | cons c cs ih =>
    simp only [encode, List.mem_append, not_or]
    refine ⟨?_, ih⟩
    unfold encChar; split <;> simp_all [eq_comm]

/-! ## §2 Sentence flags -/

/-- The facts about one APE result that `Engine::finish` inspects. -/
structure ApeFacts where
  owlMessage   : Bool   -- an APE message of type "owl"
  owlxmlEmpty  : Bool
  hasRule      : Bool   -- `<swrl:Imp>` or `<DLSafeRule>`
  hasChain     : Bool   -- `TransitiveObjectProperty` / `ObjectPropertyChain`
  noAxioms     : Bool   -- OWL/XML parsed to an empty ontology

def isOWLSWRL (f : ApeFacts) : Bool := !f.owlMessage && !f.owlxmlEmpty && !f.noAxioms
def isOWL (f : ApeFacts) : Bool := isOWLSWRL f && !f.hasRule
def reasonable (noChains : Bool) (f : ApeFacts) : Bool :=
  isOWL f && (if noChains then !f.hasChain else true)

theorem reasonable_imp_owl (nc : Bool) (f : ApeFacts) : reasonable nc f = true → isOWL f = true := by
  unfold reasonable; intro h; simp [Bool.and_eq_true] at h; exact h.1

theorem owl_imp_owlswrl (f : ApeFacts) : isOWL f = true → isOWLSWRL f = true := by
  unfold isOWL; intro h; simp [Bool.and_eq_true] at h; exact h.1

theorem rule_never_reasonable (nc : Bool) (f : ApeFacts) (h : f.hasRule = true) : reasonable nc f = false := by
  simp [reasonable, isOWL, h]

theorem chain_not_reasonable_under_no_chains (f : ApeFacts) (h : f.hasChain = true) : reasonable true f = false := by
  simp [reasonable, h]

/-! ## §3 Load-time demotion -/

structure Sent where
  integrated : Bool
  integrable : Bool

def demote (s : Sent) : Sent := { s with integrated := s.integrated && s.integrable }

theorem demote_sound (l : List Sent) : ∀ s ∈ l.map demote, s.integrated = true → s.integrable = true := by
  intro s hs hi
  simp only [List.mem_map] at hs
  obtain ⟨t, _, rfl⟩ := hs
  simp [demote, Bool.and_eq_true] at hi
  exact hi.2

theorem demote_monotone (s : Sent) : (demote s).integrated = true → s.integrated = true := by
  simp [demote, Bool.and_eq_true]; intro h _; exact h

/-! ## §4 Commit discipline keeps the knowledge base consistent -/

section Commit
variable {α : Type} (cons : List α → Prop)

inductive Edit (α : Type) where
  | add (s : α) | retract (s : α)

/-- One edit: an added sentence is integrated only if `KB ∪ {s}` passes the consistency check;
    otherwise it is stored unintegrated (not in the KB). Retraction removes occurrences. -/
def step [DecidableEq α] (dec : ∀ k, Decidable (cons k)) (kb : List α) : Edit α → List α
  | .add s     => if (dec (s :: kb)).decide then s :: kb else kb
  | .retract s => kb.filter (· ≠ s)

def run [DecidableEq α] (dec : ∀ k, Decidable (cons k)) (kb : List α) : List (Edit α) → List α
  | []      => kb
  | e :: es => run dec (step cons dec kb e) es

theorem step_consistent [DecidableEq α] (dec : ∀ k, Decidable (cons k))
    (anti : ∀ k s, cons k → cons (k.filter (· ≠ s)))
    (kb : List α) (h : cons kb) (e : Edit α) : cons (step cons dec kb e) := by
  cases e with
  | add s =>
    simp only [step]
    by_cases hc : cons (s :: kb)
    · rw [if_pos (@decide_eq_true _ (dec _) hc)]; exact hc
    · rw [if_neg (by rw [@decide_eq_false _ (dec _) hc]; simp)]; exact h
  | retract s => exact anti kb s h

theorem run_consistent [DecidableEq α] (dec : ∀ k, Decidable (cons k))
    (anti : ∀ k s, cons k → cons (k.filter (· ≠ s)))
    (es : List (Edit α)) : ∀ kb, cons kb → cons (run cons dec kb es) := by
  induction es with
  | nil => intro kb h; exact h
  | cons e es ih => intro kb h; exact ih _ (step_consistent cons dec anti kb h e)
end Commit

/-! ## §5 Engine recycling bound -/

/-- Counter after one query: increment, and reset when the limit is reached (machine rebuilt). -/
def tick (n q : Nat) : Nat := if n > 0 ∧ q + 1 ≥ n then 0 else q + 1

def ticks (n : Nat) : Nat → Nat → Nat
  | q, 0     => q
  | q, k + 1 => ticks n (tick n q) k

theorem tick_lt (n q : Nat) (hn : 0 < n) (hq : q < n) : tick n q < n := by
  unfold tick; split
  · exact hn
  · omega

theorem recycle_bound (n : Nat) (hn : 0 < n) : ∀ k q, q < n → ticks n q k < n := by
  intro k; induction k with
  | zero => intro q hq; exact hq
  | succ k ih => intro q hq; exact ih _ (tick_lt n q hn hq)

/-! ## §6 MCC -/

def mccNum (tp tn fp fn : Int) : Int := tp * tn - fp * fn
def mccDen2 (tp tn fp fn : Int) : Int := (tp + fp) * (tp + fn) * (tn + fp) * (tn + fn)

theorem mcc_perfect (tp tn : Int) : mccNum tp tn 0 0 * mccNum tp tn 0 0 = mccDen2 tp tn 0 0 := by
  simp [mccNum, mccDen2, Int.mul_comm, Int.mul_left_comm]

theorem mcc_symm (tp tn fp fn : Int) :
    mccNum tp tn fp fn = mccNum tp tn fn fp ∧ mccDen2 tp tn fp fn = mccDen2 tp tn fn fp := by
  constructor
  · simp [mccNum, Int.mul_comm]
  · simp only [mccDen2]; ac_rfl

/-! ## §7 Parser memoisation is exact

The optimised parser caches, per abstract-option identifier, which lexical rules fulfil it
(`LexCache.fulfil`). This is sound because fulfilment depends only on that identifier: the
identifier canonically encodes the category's bound values, variable co-references and exceptions.
Abstractly: if `f` factors through `key`, and every cache entry was computed by `f` on some input
with that key, then a cache lookup returns exactly `f x`, whether it hits or misses. -/

def lookup {α κ β : Type} (f : α → β) (key : α → κ) (cache : κ → Option β) (x : α) : β :=
  match cache (key x) with
  | some b => b
  | none   => f x

theorem memo_exact {α κ β : Type} (f : α → β) (key : α → κ) (cache : κ → Option β)
    (hf : ∀ x y, key x = key y → f x = f y)
    (hc : ∀ k b, cache k = some b → ∃ y, key y = k ∧ f y = b)
    (x : α) : lookup f key cache x = f x := by
  unfold lookup
  split
  · rename_i b h
    obtain ⟨y, hy, hb⟩ := hc _ _ h
    rw [← hb]; exact hf y x hy
  · rfl

/-- Filling the cache with `f` keeps the invariant `hc`, so `memo_exact` holds after every miss. -/
theorem memo_insert_ok {α κ β : Type} [DecidableEq κ] (f : α → β) (key : α → κ) (cache : κ → Option β)
    (hc : ∀ k b, cache k = some b → ∃ y, key y = k ∧ f y = b) (x : α) :
    ∀ k b, (fun k' => if k' = key x then some (f x) else cache k') k = some b → ∃ y, key y = k ∧ f y = b := by
  intro k b h
  by_cases hk : k = key x
  · subst hk; simp at h; exact ⟨x, rfl, h⟩
  · simp [hk] at h; exact hc k b h
