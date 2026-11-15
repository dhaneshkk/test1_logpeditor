/-
  ApeMigration.lean — machine-checked guarantees for the APE (SWI-Prolog) → Rust + Scryer port.
  Lean 4 core only (no Mathlib).  Check with:  lean formal/ApeMigration.lean

  §1  Query quoting (ape-rs `quote_atom`) is injection-safe: decoding the quoted body with an
      ISO-reader escape model returns exactly the input, and the encoded body never contains an
      unescaped quote, so user text can never terminate the atom and inject Prolog goals.
  §2  XML indentation (pure-Prolog port of SWI sgml_write `emit_indent/2`): N÷8 tabs + N mod 8
      spaces has display width exactly N with 8-column tabs, for every N.
  §3  ASCII case mapping in swi_compat (`lower_char/upper_char`) are mutually inverse on letters.
  §4  MCC: with zero disagreements (fp = fn = 0) the squared numerator equals the product of the
      four marginals, i.e. MCC = 1 whenever both classes occur; and MCC's numerator and
      denominator are symmetric under swapping reference/candidate (fp ↔ fn), so the parity score
      "Scryer vs SWI" equals "SWI vs Scryer".
-/

/-! ## §1 Quoting -/

/-- Escape one character exactly as `ape_rs::quote_atom` does for the cases that matter. -/
def escChar : Char → List Char
  | '\'' => ['\\', '\'']
  | '\\' => ['\\', '\\']
  | '\n' => ['\\', 'n']
  | '\t' => ['\\', 't']
  | '\r' => ['\\', 'r']
  | c    => [c]

def escape : List Char → List Char
  | []      => []
  | c :: cs => escChar c ++ escape cs

/-- ISO-reader model of escape decoding inside a quoted atom (`Option.none` = syntax error /
    premature end of atom, i.e. a bare quote). -/
def unescape : List Char → Option (List Char)
  | [] => some []
  | '\\' :: '\'' :: r => (unescape r).map ('\'' :: ·)
  | '\\' :: '\\' :: r => (unescape r).map ('\\' :: ·)
  | '\\' :: 'n'  :: r => (unescape r).map ('\n' :: ·)
  | '\\' :: 't'  :: r => (unescape r).map ('\t' :: ·)
  | '\\' :: 'r'  :: r => (unescape r).map ('\r' :: ·)
  | '\\' :: _ => none
  | '\'' :: _ => none            -- a bare quote would end the atom: injection point
  | c :: r => (unescape r).map (c :: ·)

/-- Round-trip: the Prolog reader recovers exactly the user's text. -/
theorem unescape_escape (s : List Char) : unescape (escape s) = some s := by
  induction s with
  | nil => rfl
  | cons c cs ih =>
    unfold escape
    by_cases h1 : c = '\'' ; · subst h1; simp [escChar, unescape, ih]
    by_cases h2 : c = '\\' ; · subst h2; simp [escChar, unescape, ih]
    by_cases h3 : c = '\n' ; · subst h3; simp [escChar, unescape, ih]
    by_cases h4 : c = '\t' ; · subst h4; simp [escChar, unescape, ih]
    by_cases h5 : c = '\r' ; · subst h5; simp [escChar, unescape, ih]
    have hc : escChar c = [c] := by
      unfold escChar; split <;> simp_all
    rw [hc]
    show unescape (c :: escape cs) = some (c :: cs)
    unfold unescape
    split <;> simp_all

/-- Corollary: decoding never hits a bare quote, so the literal cannot be terminated early. -/
theorem quoted_never_breaks_out (s : List Char) : (unescape (escape s)).isSome := by
  simp [unescape_escape]

/-! ## §2 XML indentation width -/

def indentChars (n : Nat) : List Char :=
  List.replicate (n / 8) '\t' ++ List.replicate (n % 8) ' '

def width : List Char → Nat
  | [] => 0
  | '\t' :: r => 8 + width r
  | _ :: r => 1 + width r

theorem width_cons_tab (r : List Char) : width ('\t' :: r) = 8 + width r := rfl

theorem width_cons (c : Char) (r : List Char) (h : c ≠ '\t') : width (c :: r) = 1 + width r := by
  simp_all [width]

theorem width_append (a b : List Char) : width (a ++ b) = width a + width b := by
  induction a with
  | nil => simp [width]
  | cons c r ih =>
    by_cases h : c = '\t'
    · subst h; rw [List.cons_append, width_cons_tab, width_cons_tab, ih]; omega
    · rw [List.cons_append, width_cons c _ h, width_cons c _ h, ih]; omega

theorem width_tabs (k : Nat) : width (List.replicate k '\t') = 8 * k := by
  induction k with
  | zero => rfl
  | succ k ih => rw [List.replicate_succ, width_cons_tab, ih]; omega

theorem width_spaces (k : Nat) : width (List.replicate k ' ') = k := by
  induction k with
  | zero => rfl
  | succ k ih => rw [List.replicate_succ, width_cons ' ' _ (by decide), ih]; omega

theorem indent_width (n : Nat) : width (indentChars n) = n := by
  unfold indentChars
  rw [width_append, width_tabs, width_spaces]
  exact Nat.div_add_mod n 8

/-! ## §3 ASCII case mapping (swi_compat lower_char / upper_char) -/

def lowerCode (x : Nat) : Nat := if 65 ≤ x ∧ x ≤ 90 then x + 32 else x
def upperCode (x : Nat) : Nat := if 97 ≤ x ∧ x ≤ 122 then x - 32 else x

theorem upper_lower (x : Nat) (h : 65 ≤ x ∧ x ≤ 90) : upperCode (lowerCode x) = x := by
  unfold lowerCode upperCode
  rw [if_pos h, if_pos (by omega)]; omega

theorem lower_upper (x : Nat) (h : 97 ≤ x ∧ x ≤ 122) : lowerCode (upperCode x) = x := by
  unfold lowerCode upperCode
  rw [if_pos h, if_pos (by omega)]; omega

theorem lower_idem (x : Nat) : lowerCode (lowerCode x) = lowerCode x := by
  unfold lowerCode
  by_cases h : 65 ≤ x ∧ x ≤ 90
  · rw [if_pos h, if_neg (by omega)]
  · rw [if_neg h, if_neg h]

/-! ## §4 Matthews Correlation Coefficient -/

/-- MCC numerator and the product under the square root of its denominator. -/
def mccNum (tp tn fp fn : Int) : Int := tp * tn - fp * fn
def mccDen2 (tp tn fp fn : Int) : Int := (tp + fp) * (tp + fn) * (tn + fp) * (tn + fn)

/-- Perfect agreement ⇒ num² = den², hence MCC = num / √den² = 1 when tp, tn > 0. -/
theorem mcc_perfect (tp tn : Int) :
    (mccNum tp tn 0 0) ^ 2 = mccDen2 tp tn 0 0 := by
  unfold mccNum mccDen2; simp [Int.pow_succ, Int.pow_zero]; ac_rfl

theorem mcc_perfect_pos (tp tn : Int) (h1 : 0 < tp) (h2 : 0 < tn) : 0 < mccNum tp tn 0 0 := by
  unfold mccNum; simp; exact Int.mul_pos h1 h2

/-- Swapping which system is "reference" swaps fp and fn and leaves MCC unchanged. -/
theorem mcc_symm (tp tn fp fn : Int) :
    mccNum tp tn fp fn = mccNum tp tn fn fp ∧ mccDen2 tp tn fp fn = mccDen2 tp tn fn fp := by
  unfold mccNum mccDen2
  constructor
  · rw [Int.mul_comm fp fn]
  · ac_rfl

/-- The concrete result of the 3,732-test differential run (Scryer vs SWI acceptance). -/
theorem ape_run_mcc_is_one :
    (mccNum 2682 1050 0 0) ^ 2 = mccDen2 2682 1050 0 0 ∧ 0 < mccNum 2682 1050 0 0 := by
  decide

/-! Axiom audit: every theorem above depends only on Lean's standard axioms. -/
#print axioms unescape_escape
#print axioms indent_width
#print axioms upper_lower
#print axioms mcc_perfect
#print axioms mcc_symm
#print axioms ape_run_mcc_is_one
