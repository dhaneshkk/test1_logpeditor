/*  serialize_term.pl — Scryer port.
    SWI operator declarations are module-local; in Scryer they are global. The SWI original
    therefore never saw e.g. drs_to_fol_to_prenex's `can`/`must`/... operators while writing.
    To reproduce SWI's output exactly, every operator that is not in Scryer's default table is
    disabled for the duration of the write and restored afterwards.
*/
:- module(serialize_term, [serialize_term/1, serialize_term/2, serialize_term_into_atom/2]).
:- use_module(library(lists)).
:- use_module(library(terms)).
:- use_module(library(iso_ext)).
:- use_module('../swi_compat', [swi_with_output_to_atom/2]).

serialize_term(Term) :- current_output(S), serialize_term(S, Term).
serialize_term(Stream, Term) :-
    \+ \+ ( numbervars(Term, 0, _),
            findall(op(P,T,N), (current_op(P, T, N), \+ default_op(P, T, N)), Ops),
            forall(member(op(_,T,N), Ops), op(0, T, N)),
            default_ops(Ds), forall((member(op(P,T,N), Ds), N \== (',')), op(P, T, N)),   % re-establish shadowed defaults (e.g. ':')
            op(0, fy, -), op(0, fy, ~), op(0, xfx, =>), op(0, xfx, v), op(0, xfx, &),  % as in the SWI original
            catch(write_term(Stream, Term, [numbervars(true), quoted(true)]), E, true),
            op(200, fy, -),
            forall(member(op(P,T,N), Ops), op(P, T, N)),
            ( nonvar(E) -> throw(E) ; true ) ).

serialize_term_into_atom(Term, Atom) :- swi_with_output_to_atom(serialize_term(Term), Atom).

default_op(P, T, N) :- default_ops(L), memberchk(op(P,T,N), L).
default_ops([
  op(1000,xfy,','),op(1050,xfy,->),op(1100,xfy,;),op(1200,fx,:-),op(1200,fx,?-),op(1200,xfx,-->),
  op(1200,xfx,:-),op(200,fy,+),op(200,fy,-),op(200,fy,\),op(200,xfx,**),op(200,xfy,^),op(400,yfx,*),
  op(400,yfx,/),op(400,yfx,//),op(400,yfx,<<),op(400,yfx,>>),op(400,yfx,div),op(400,yfx,mod),
  op(400,yfx,rdiv),op(400,yfx,rem),op(500,yfx,+),op(500,yfx,-),op(500,yfx,/\),op(500,yfx,\/),
  op(600,xfy,:),op(700,fx,non_counted_backtracking),op(700,xfx,<),op(700,xfx,=),op(700,xfx,=..),
  op(700,xfx,=:=),op(700,xfx,=<),op(700,xfx,==),op(700,xfx,=\=),op(700,xfx,>),op(700,xfx,>=),
  op(700,xfx,@<),op(700,xfx,@=<),op(700,xfx,@>),op(700,xfx,@>=),op(700,xfx,\=),op(700,xfx,\==),
  op(700,xfx,is),op(900,fy,\+)]).
