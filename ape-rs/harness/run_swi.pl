% Reference run: original APE on SWI-Prolog -> r(N, GoldCode, NMsgs, DRS) per regression test.
%   swipl harness/run_swi.pl <APE_DIR> harness/tests_canon.pl harness/swi_results.txt
:- initialization(main, main).
:- op(400, fx, -). :- op(400, fx, ~). :- op(500, xfx, =>). :- op(500, xfx, v).
main :-
  current_prolog_flag(argv, [Ape, Tests, Out|_]),
  directory_file_path(Ape, 'prolog/parser/ace_to_drs', A2D), use_module(A2D),
  directory_file_path(Ape, 'prolog/utils/are_equivalent', Eq), use_module(Eq),
  load_files(Tests, [module(user), encoding(utf8)]),
  open(Out, write, S, [encoding(utf8)]),
  forall(t(N, Cs, Gold0), run1(S, N, Cs, Gold0)), close(S).
run1(S, N, Cs, Gold0) :-
  remove_pn(Gold0, Gold),
  catch(call_with_time_limit(10, (acetext_to_drs(Cs, _, _, Drs, Msgs), length(Msgs, NM))), _, (Drs = err, NM = -1)),
  ( Drs == err -> Code = err ; result(Gold, Drs, Code) ),
  numbervars(Drs, 0, _), writeq(S, r(N, Code, NM, Drs)), write(S, '.'), nl(S).
remove_pn(drs(D0,C0), drs(D,C)) :- exclude(is_named, C0, C), exclude(ground, D0, D).
is_named(object(named(Name), Name, named, _, _, _)-_).
result(drs([], []), drs([], []), '0000') :- !.
result(drs([], []), Drs, '0->#') :- Drs \= drs([], []), !.
result(Drs, drs([], []), '#->0') :- Drs \= drs([], []), !.
result(D1, D2, '----') :- are_equivalent(D1, D2), !.
result(_, _, '####').
