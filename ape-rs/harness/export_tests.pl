% SWI: export APE's regression suite (tests/acetexts.pl) in canonical syntax readable by Scryer.
%   swipl harness/export_tests.pl <APE_DIR> harness/tests_canon.pl
:- initialization(main, main).
:- op(400, fx, -). :- op(400, fx, ~). :- op(500, xfx, =>). :- op(500, xfx, v).
main :- current_prolog_flag(argv, [Ape, Out|_]),
  directory_file_path(Ape, 'tests/acetexts.pl', F), load_files(F, [module(user)]),
  open(Out, write, S, [encoding(utf8)]),
  forall(text_drs_eval(0, N, Text, Drs, _, _, _, _),
         ( atom_codes(Text, Cs), write_canonical(S, t(N, Cs, Drs)), write(S, '.'), nl(S) )),
  close(S).
