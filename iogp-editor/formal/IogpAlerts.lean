/-
  IogpAlerts.lean — machine-checked guarantees for the iogp-editor alert runtime and compiler.
  Lean 4 core only (no Mathlib).  Check with:  lean formal/IogpAlerts.lean

  Time is measured in frame ticks (Nat); `g` is the grace period and `cd` the cooldown in ticks.

  §1  Evidence smoothing (runtime.rs `world`): a fact is *believed* at t iff it was observed at
      some s ≤ t with t ≤ s + g.  Observation ⇒ belief; g = 0 is exact; belief is monotone in g;
      no observation ⇒ no belief (positive-evidence policy).
  §2  Negated durations are exact: "not believed throughout [T-D, T]" ⇔ "not observed throughout
      [T-D-g, T]".  So "does not wear a harness for at least D seconds" fires exactly when the
      harness detector has been silent for D + g — never on a single missed frame, and never late.
  §3  Positive durations bridge gaps: if observations within [T-D-g, T] are never more than g
      ticks apart (and one is at or before T-D), the condition is believed throughout [T-D, T].
  §4  AlertMachine (runtime.rs, after yappu-fsm): an alert is raised only from IDLE while its
      condition holds; a continuously-holding condition raises at most once (one alert per
      episode); after a raise, no re-raise is possible before `cd` ticks have passed.
  §5  Tracker-ID robustness of the compiled FOL: the compiler puts the object of a duration
      condition *inside* the time quantifier (∀S. ∃H. wear(W,H,S)).  This is implied by the
      outer-scope reading (∃H. ∀S. wear(W,H,S)) but strictly weaker, so a harness whose tracker
      ID changes mid-window still counts as worn.
  §6  MCC: no disagreements ⇒ MCC = 1 (squared numerator = product of marginals); symmetric
      under swapping false positives and false negatives.
  §7  Audit log (store.rs): each entry's hash covers the previous hash and the entry body. If the
      hash function is injective (collision resistance, idealised), two verified logs with the
      same head hash are identical: the head commits to the entire history, so editing, removing
      or reordering any past entry is detected.
  §8  Four-eyes (store.rs): every workflow action preserves the invariant "every approved
      version was approved by someone other than its author", and only approved versions can be
      active (exported).
  §9  Exceptions and scoping (compile.rs, runtime.rs): "… unless E" compiles to an extra
      conjunct ¬E, so an exception can only suppress alerts, never create one; scoping a zone to
      a named instance or a camera only removes candidate bindings, so it too can only suppress.
  §10 ASP encoding of durations (asp.rs): the run start r(i) — "the condition has held since
      frame r(i)" — computed frame to frame (r(i+1) = r(i) if the condition held at frame i, else
      i+1) is exactly the start of the current streak: every frame from r(i) to i satisfies the
      condition, and the frame before r(i) does not. So "T - S >= D" in the encoding is the
      runtime timer's "held for at least D".
-/

/-! ## §1 Evidence smoothing -/

def believed (g : Nat) (obs : Nat → Prop) (t : Nat) : Prop :=
  ∃ s, s ≤ t ∧ t ≤ s + g ∧ obs s

theorem observed_believed {g t : Nat} {obs : Nat → Prop} (h : obs t) : believed g obs t :=
  ⟨t, Nat.le_refl _, Nat.le_add_right _ _, h⟩

theorem believed_zero (obs : Nat → Prop) (t : Nat) : believed 0 obs t ↔ obs t := by
  constructor
  · intro ⟨s, h1, h2, h3⟩
    have : s = t := by omega
    exact this ▸ h3
  · exact observed_believed

theorem believed_mono {g g' t : Nat} {obs : Nat → Prop} (hg : g ≤ g') :
    believed g obs t → believed g' obs t := by
  intro ⟨s, h1, h2, h3⟩
  exact ⟨s, h1, by omega, h3⟩

theorem positive_evidence {g t : Nat} {obs : Nat → Prop} (h : ∀ s, ¬ obs s) : ¬ believed g obs t := by
  intro ⟨s, _, _, h3⟩
  exact h s h3

/-! ## §2 Negated durations are exact -/

theorem absent_window (g D T : Nat) (obs : Nat → Prop) (hT : D + g ≤ T) :
    (∀ s, T - D ≤ s → s ≤ T → ¬ believed g obs s) ↔
    (∀ u, T - D - g ≤ u → u ≤ T → ¬ obs u) := by
  constructor
  · intro h u hu1 hu2 hobs
    by_cases hc : T - D ≤ u
    · exact h u hc hu2 (observed_believed hobs)
    · exact h (T - D) (Nat.le_refl _) (by omega) ⟨u, by omega, by omega, hobs⟩
  · intro h s hs1 hs2 ⟨u, hu1, hu2, hobs⟩
    exact h u (by omega) (by omega) hobs

/-- Consequence: a single missed frame (any gap of at most `D + g - 1` ticks without the object)
    can never satisfy a negated duration of D ticks. -/
theorem short_gap_never_fires (g D T : Nat) (obs : Nat → Prop) (hT : D + g ≤ T)
    (u : Nat) (hu1 : T - D - g ≤ u) (hu2 : u ≤ T) (hobs : obs u) :
    ¬ (∀ s, T - D ≤ s → s ≤ T → ¬ believed g obs s) := by
  intro h
  exact (absent_window g D T obs hT).mp h u hu1 hu2 hobs

/-! ## §3 Positive durations bridge detection gaps -/

theorem gaps_bridged (g D T : Nat) (obs : Nat → Prop)
    (hcover : ∀ s, T - D ≤ s → s ≤ T → ∃ u, u ≤ s ∧ s ≤ u + g ∧ obs u) :
    ∀ s, T - D ≤ s → s ≤ T → believed g obs s := by
  intro s h1 h2
  exact hcover s h1 h2

/-- If consecutive observations are at most `g` apart, every time between the first and the last
    observation is covered.  Stated for an explicit increasing list of observation times. -/
def gapsAtMost (g : Nat) : List Nat → Prop
  | [] => True
  | [_] => True
  | a :: b :: rest => a ≤ b ∧ b ≤ a + g ∧ gapsAtMost g (b :: rest)

theorem covered_between (g : Nat) (obs : Nat → Prop) :
    ∀ (l : List Nat) (a : Nat), gapsAtMost g (a :: l) → (∀ x, x ∈ a :: l → obs x) →
    ∀ s, a ≤ s → s ≤ (a :: l).getLast (List.cons_ne_nil _ _) → believed g obs s := by
  intro l
  induction l with
  | nil =>
    intro a _ hobs s h1 h2
    simp at h2
    have : s = a := by omega
    exact this ▸ observed_believed (hobs a (by simp))
  | cons b rest ih =>
    intro a hg hobs s h1 h2
    obtain ⟨hab, hba, hrest⟩ := hg
    by_cases hs : s < b
    · exact ⟨a, h1, by omega, hobs a (by simp)⟩
    · have hl : (a :: b :: rest).getLast (List.cons_ne_nil _ _) = (b :: rest).getLast (List.cons_ne_nil _ _) := by
        simp [List.getLast_cons]
      rw [hl] at h2
      exact ih b hrest (fun x hx => hobs x (List.mem_cons_of_mem _ hx)) s (by omega) h2

/-! ## §4 AlertMachine -/

inductive AS where
  | idle
  | active
  | cooling (since : Nat)
deriving DecidableEq

/-- One frame for one (rule, subject): returns the new state and whether the alert is raised. -/
def step (cd : Nat) : AS → Nat → Bool → AS × Bool
  | AS.idle,      _, true  => (AS.active, true)
  | AS.active,    _, true  => (AS.active, false)
  | AS.cooling _, _, true  => (AS.active, false)
  | AS.idle,      _, false => (AS.idle, false)
  | AS.active,    t, false => (AS.cooling t, false)
  | AS.cooling c, t, false => if c + cd ≤ t then (AS.idle, false) else (AS.cooling c, false)

/-- Run over (time, condition-holds) frames; returns final state and number of raises. -/
def run (cd : Nat) : AS → List (Nat × Bool) → AS × Nat
  | s, [] => (s, 0)
  | s, (t, d) :: xs =>
    let r := step cd s t d
    let rest := run cd r.1 xs
    (rest.1, rest.2 + if r.2 then 1 else 0)

theorem raise_only_from_idle (cd t : Nat) (s : AS) (d : Bool) :
    (step cd s t d).2 = true → s = AS.idle ∧ d = true := by
  cases s <;> cases d <;> simp [step]
  split <;> simp

theorem holds_goes_active (cd t : Nat) (s : AS) : (step cd s t true).1 = AS.active := by
  cases s <;> simp [step]

theorem no_raise_when_not_idle (cd t : Nat) (s : AS) (d : Bool) (h : s ≠ AS.idle) :
    (step cd s t d).2 = false := by
  cases s <;> cases d <;> simp [step] at *
  split <;> simp

theorem continuous_from_active (cd : Nat) :
    ∀ xs : List (Nat × Bool), (∀ x, x ∈ xs → x.2 = true) → (run cd AS.active xs).2 = 0 := by
  intro xs
  induction xs with
  | nil => intro _; rfl
  | cons x xs ih =>
    intro h
    obtain ⟨t, d⟩ := x
    have hd : d = true := h (t, d) (by simp)
    subst hd
    simp [run, step]
    exact ih (fun y hy => h y (List.mem_cons_of_mem _ hy))

/-- One alert per episode: while the condition holds continuously, at most one raise. -/
theorem one_alert_per_episode (cd : Nat) (s : AS) (xs : List (Nat × Bool))
    (h : ∀ x, x ∈ xs → x.2 = true) : (run cd s xs).2 ≤ 1 := by
  cases xs with
  | nil => simp [run]
  | cons x xs =>
    obtain ⟨t, d⟩ := x
    have hd : d = true := h (t, d) (by simp)
    subst hd
    have hrest := continuous_from_active cd xs (fun y hy => h y (List.mem_cons_of_mem _ hy))
    cases s <;> simp [run, step, hrest]

/-- Invariant after a raise at time t0: the machine is ACTIVE or COOLING since some c ≥ t0. -/
def NotCleared (t0 : Nat) : AS → Prop
  | AS.active => True
  | AS.cooling c => t0 ≤ c
  | AS.idle => False

theorem step_keeps (cd t0 t : Nat) (s : AS) (d : Bool) (hs : NotCleared t0 s)
    (ht1 : t0 ≤ t) (ht2 : t < t0 + cd) :
    NotCleared t0 (step cd s t d).1 ∧ (step cd s t d).2 = false := by
  cases s with
  | idle => exact absurd hs id
  | active => cases d <;> simp [step, NotCleared] <;> omega
  | cooling c =>
    simp [NotCleared] at hs
    cases d
    · simp only [step]
      split
      · omega
      · simp [NotCleared]; omega
    · simp [step, NotCleared]

/-- Cooldown: once raised at t0, the alert cannot be raised again by any frame before t0 + cd. -/
theorem no_reraise_within_cooldown (cd t0 : Nat) :
    ∀ (xs : List (Nat × Bool)) (s : AS), NotCleared t0 s →
    (∀ x, x ∈ xs → t0 ≤ x.1 ∧ x.1 < t0 + cd) → (run cd s xs).2 = 0 := by
  intro xs
  induction xs with
  | nil => intro _ _ _; rfl
  | cons x xs ih =>
    intro s hs ht
    obtain ⟨t, d⟩ := x
    have ⟨h1, h2⟩ := ht (t, d) (by simp)
    have ⟨hk, hr⟩ := step_keeps cd t0 t s d hs h1 h2
    simp only [run, hr]
    simp
    exact ih _ hk (fun y hy => ht y (List.mem_cons_of_mem _ hy))

/-- Right after a raise the state is ACTIVE, which satisfies the invariant. -/
theorem raised_then_not_cleared (cd t : Nat) : NotCleared t (step cd AS.idle t true).1 := by
  simp [step, NotCleared]

/-! ## §5 Tracker-ID robustness of the inner-scope FOL -/

theorem outer_implies_inner {α : Type} (wear : α → Nat → Prop) (D T : Nat) :
    (∃ h, ∀ s, T - D ≤ s → s ≤ T → wear h s) → (∀ s, T - D ≤ s → s ≤ T → ∃ h, wear h s) := by
  intro ⟨h, hh⟩ s h1 h2
  exact ⟨h, hh s h1 h2⟩

/-- The converse fails: a harness detected as track 0 at time 0 and as track 1 at time 1. -/
theorem inner_strictly_weaker :
    ¬ (∀ wear : Nat → Nat → Prop,
        (∀ s, 1 - 1 ≤ s → s ≤ 1 → ∃ h, wear h s) → ∃ h, ∀ s, 1 - 1 ≤ s → s ≤ 1 → wear h s) := by
  intro H
  obtain ⟨h, hh⟩ := H (fun h s => h = s) (fun s _ _ => ⟨s, rfl⟩)
  have a : h = 0 := hh 0 (by omega) (by omega)
  have b : h = 1 := hh 1 (by omega) (by omega)
  omega

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


/-! ## §7 Audit log: the head hash commits to the whole history -/

/-- A log is a list of entry bodies (newest first); `chain h genesis log` is its head hash. -/
def chain {B H : Type} (h : H → B → H) (genesis : H) : List B → H
  | [] => genesis
  | b :: rest => h (chain h genesis rest) b

/-- Injective hashing (idealised collision resistance): equal hashes ⇒ equal (prev, body). -/
def HashInjective {B H : Type} (h : H → B → H) : Prop := ∀ p₁ p₂ b₁ b₂, h p₁ b₁ = h p₂ b₂ → p₁ = p₂ ∧ b₁ = b₂

theorem head_commits {B H : Type} (h : H → B → H) (g : H) (hi : HashInjective h)
    (hg : ∀ p b, h p b ≠ g) :
    ∀ l₁ l₂ : List B, chain h g l₁ = chain h g l₂ → l₁ = l₂ := by
  intro l₁
  induction l₁ with
  | nil =>
    intro l₂ heq
    cases l₂ with
    | nil => rfl
    | cons b r => exact absurd heq.symm (hg _ _)
  | cons b₁ r₁ ih =>
    intro l₂ heq
    cases l₂ with
    | nil => exact absurd heq (hg _ _)
    | cons b₂ r₂ =>
      have ⟨hp, hb⟩ := hi _ _ _ _ heq
      rw [hb, ih r₂ hp]

/-- Tampering is detected: any change to the history changes the head. -/
theorem tamper_detected {B H : Type} (h : H → B → H) (g : H) (hi : HashInjective h)
    (hg : ∀ p b, h p b ≠ g) (l l' : List B) (hne : l ≠ l') : chain h g l ≠ chain h g l' :=
  fun heq => hne (head_commits h g hi hg l l' heq)

/-! ## §8 Four-eyes invariant -/

inductive VSt where
  | draft | submitted | approved | rejected | superseded | retired
deriving DecidableEq

structure Ver where
  author : Nat
  status : VSt
  reviewer : Option Nat

/-- Approved versions were approved by someone else. -/
def FourEyes (vs : List Ver) : Prop :=
  ∀ v, v ∈ vs → v.status = VSt.approved → ∃ r, v.reviewer = some r ∧ r ≠ v.author

/-- The store's approve step on one version (other steps never set `approved`). -/
def approveV (u : Nat) (v : Ver) : Option Ver :=
  if v.status = VSt.submitted ∧ u ≠ v.author then some { v with status := VSt.approved, reviewer := some u } else none

/-- Any non-approving action: changes status to anything but `approved`. -/
def otherV (s : VSt) (v : Ver) (_ : s ≠ VSt.approved) : Ver := { v with status := s }

theorem approve_keeps (u : Nat) (v v' : Ver) (h : approveV u v = some v') :
    v'.status = VSt.approved → ∃ r, v'.reviewer = some r ∧ r ≠ v'.author := by
  unfold approveV at h
  split at h
  · rename_i hc
    cases h
    intro _
    exact ⟨u, rfl, hc.2⟩
  · cases h

theorem other_keeps (s : VSt) (v : Ver) (hs : s ≠ VSt.approved) :
    (otherV s v hs).status = VSt.approved → False := by
  simp [otherV]; exact hs

/-- Replacing one version by the result of any action preserves the invariant. -/
theorem four_eyes_step (vs : List Ver) (hinv : FourEyes vs) (v' : Ver)
    (hv' : v'.status = VSt.approved → ∃ r, v'.reviewer = some r ∧ r ≠ v'.author) :
    FourEyes (v' :: vs) := by
  intro w hw hst
  cases hw with
  | head => exact hv' hst
  | tail _ hmem => exact hinv w hmem hst

/-- Only approved versions are exported: `active` is set by approve alone. -/
def exported (vs : List Ver) : List Ver := vs.filter (fun v => v.status = VSt.approved)

theorem exported_four_eyes (vs : List Ver) (h : FourEyes vs) :
    ∀ v, v ∈ exported vs → ∃ r, v.reviewer = some r ∧ r ≠ v.author := by
  intro v hv
  simp [exported, List.mem_filter] at hv
  exact h v hv.1 hv.2

/-! ## §9 Exceptions and scoping only suppress alerts -/

/-- "If A then alert unless E" ≡ "If A ∧ ¬E then alert". -/
def fires (A E : Prop) : Prop := A ∧ ¬ E

theorem unless_suppresses (A E : Prop) : fires A E → A := fun h => h.1

theorem unless_exact (A E : Prop) : fires A E ↔ (A ∧ ¬ E) := Iff.rfl

/-- Several exceptions ("unless E₁ or E₂") suppress at least as much as either one. -/
theorem more_exceptions_fewer_alerts (A E₁ E₂ : Prop) : fires A (E₁ ∨ E₂) → fires A E₁ :=
  fun ⟨ha, hn⟩ => ⟨ha, fun h1 => hn (Or.inl h1)⟩

/-- Scoping restricts the candidate zones; an alert derived with the narrower candidate set is
    also derived with the wider one (∃ over a subset implies ∃ over the superset). -/
theorem scoping_suppresses {Z : Type} (cand narrow : Z → Prop) (hsub : ∀ z, narrow z → cand z)
    (holds : Z → Prop) : (∃ z, narrow z ∧ holds z) → (∃ z, cand z ∧ holds z) := by
  intro ⟨z, hs, hh⟩
  exact ⟨z, hsub z hs, hh⟩


/-! ## §10 ASP run-start encoding = start of the current streak -/

/-- Run start at frame i, as the ASP rules compute it frame to frame. -/
def runStart (h : Nat → Bool) : Nat → Nat
  | 0 => 0
  | i + 1 => if h i then runStart h i else i + 1

theorem runStart_le (h : Nat → Bool) : ∀ i, runStart h i ≤ i := by
  intro i
  induction i with
  | zero => simp [runStart]
  | succ n ih => simp only [runStart]; split <;> omega

/-- Every frame from the run start to i satisfies the condition (when it holds at i). -/
theorem runStart_streak (h : Nat → Bool) :
    ∀ i, h i = true → ∀ j, runStart h i ≤ j → j ≤ i → h j = true := by
  intro i
  induction i with
  | zero => intro hi j h1 h2; have : j = 0 := by omega
            subst this; exact hi
  | succ n ih =>
    intro hi j h1 h2
    simp only [runStart] at h1
    by_cases hn : h n = true
    · rw [if_pos hn] at h1
      by_cases hj : j = n + 1
      · subst hj; exact hi
      · exact ih hn j h1 (by omega)
    · rw [if_neg hn] at h1
      have : j = n + 1 := by omega
      subst this; exact hi

/-- The frame just before the run start does not satisfy the condition (the streak is maximal). -/
theorem runStart_maximal (h : Nat → Bool) :
    ∀ i, 0 < runStart h i → h (runStart h i - 1) = false := by
  intro i
  induction i with
  | zero => intro hp; simp [runStart] at hp
  | succ n ih =>
    intro hp
    simp only [runStart] at hp ⊢
    by_cases hn : h n = true
    · rw [if_pos hn] at hp ⊢; exact ih hp
    · rw [if_neg hn]; simp only [Nat.add_sub_cancel]; simpa using hn
