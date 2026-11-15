% Reference run of all output modes (every STEP-th test) on SWI-APE.
%   swipl harness/outmodes_swi.pl <APE_DIR> harness/tests_canon.pl harness/om_swi.txt 10
:- initialization(main, main).
:- op(400, fx, -). :- op(400, fx, ~). :- op(500, xfx, =>). :- op(500, xfx, v).
main :- current_prolog_flag(argv, [Ape, Tests, Out, StepA|_]), atom_number(StepA, Step),
  directory_file_path(Ape, 'prolog/ape', ApeMod), use_module(ApeMod), load_files(Tests, [module(user), encoding(utf8)]),
  open(Out, write, S, [encoding(utf8)]),
  forall(( t(N, Cs, _), N mod Step =:= 1, ape:output_type(T), T \== drsrt ),
         ( atom_codes(A, Cs),
           catch(call_with_time_limit(10, get_ape_results([text=A, solo=T], CT, C)), E, (CT = err, term_to_atom(E, C))),
           writeq(S, r(N, T, CT, C)), write(S, '.'), nl(S) )),
  close(S).
