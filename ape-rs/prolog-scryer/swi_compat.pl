/*  swi_compat.pl — minimal SWI-Prolog compatibility layer for running APE on Scryer Prolog.
    Each predicate here exists because APE calls it and Scryer lacks it (or differs).
    Keep this list short; every entry is a candidate either for an upstream Scryer
    contribution or for removal by refactoring the APE call site.
*/
:- module(swi_compat, [
    concat_atom/2, concat_atom/3, atomic_list_concat/2, atomic_list_concat/3,
    swi_format_atom/3, swi_with_output_to_atom/2,
    nb_setval/2, nb_getval/2, b_setval/2, b_getval/2,
    debug/3, debug/1, nodebug/1,
    last/2, subtract/3, delete/3, exclude/3, include/3, partition/4,
    flatten/2, sumlist/2, max_list/2, min_list/2,
    term_to_atom/2, atom_number/2, atom_to_term/3,
    writef/2, writef/1, writeln/1, print_message/2,
    ignore/1, is_list/1, string/1, string_codes/2, string_to_atom/2,
    downcase_atom/2, upcase_atom/2,
    call_with_time_limit/2, get_time/1,
    flag/3, gensym/2, reset_gensym/1,
    garbage_collect/0, style_check/1,
    subsumes/2, concat_atom_codes/2,
    numbervars/3, read_term_from_atom/3,
    succ_or_zero/1, format_predicate/2, swi_must_be/2,
    swi_atom_concat/3, swi_atom_length/2, swi_atom_codes/2, swi_atom_chars/2,
    open_string/2, swi_random/2, swi_arg/3, swi_write_term/2, swi_format/2, swi_format/3, swi_univ/2
]).

:- use_module(library(lists)).

:- meta_predicate(exclude(1, ?, ?)).
:- meta_predicate(include(1, ?, ?)).
:- meta_predicate(partition(1, ?, ?, ?)).
:- meta_predicate(ignore(0)).
:- meta_predicate(swi_with_output_to_atom(0, ?)).
:- meta_predicate(call_with_time_limit(?, 0)).
:- use_module(library(iso_ext)).
:- use_module(library(charsio)).
:- use_module(library(between)).
:- use_module(library(error)).
:- use_module(library(format), []).
:- use_module(library(terms), []).
:- use_module(library(os), []).
:- use_module(library(files), [delete_file/1]).
:- use_module(library(random), [random_integer/3]).

%% ---------------------------------------------------------------- atoms / text

concat_atom(L, A) :- atomic_list_concat(L, A).
concat_atom(L, Sep, A) :- atomic_list_concat(L, Sep, A).

atomic_list_concat(L, A) :- maplist(to_chars, L, Css), append(Css, Cs), atom_chars(A, Cs).
atomic_list_concat(L, Sep, A) :-
    (   nonvar(L), ground(L) ->
        to_chars(Sep, SCs), maplist(to_chars, L, Css), join(Css, SCs, Cs), atom_chars(A, Cs)
    ;   % split mode
        must_be(atom, A), atom_chars(A, ACs), to_chars(Sep, SCs),
        split(ACs, SCs, Parts), maplist(chars_atom, Parts, L)
    ).

join([], _, []).
join([X], _, X) :- !.
join([X|Xs], S, R) :- append(X, S, XS), join(Xs, S, R0), append(XS, R0, R).

split(Cs, Sep, [P|Ps]) :-
    (   append(P, Rest0, Cs), append(Sep, Rest, Rest0) -> split(Rest, Sep, Ps)
    ;   P = Cs, Ps = []
    ).

to_chars(X, Cs) :- atom(X), !, atom_chars(X, Cs).
to_chars(X, Cs) :- number(X), !, number_chars(X, Cs).
to_chars(X, Cs) :- is_list(X), !, ( X = [C|_], integer(C) -> maplist(code_char, X, Cs) ; Cs = X ).
to_chars(X, Cs) :- write_term_to_chars(X, [], Cs).

string(X) :- is_list(X), X = [C|_], atom(C), atom_length(C, 1).   % chars-as-strings approximation
string_codes(S, Cs) :- ( var(S) -> maplist(code_char, Cs, S) ; to_chars(S, Ch), maplist(char_code, Ch, Cs) ).
string_to_atom(S, A) :- ( var(A) -> to_chars(S, Cs), atom_chars(A, Cs) ; atom_chars(A, S) ).

term_to_atom(T, A) :-
    (   var(A) -> write_term_to_chars(T, [quoted(true)], Cs), atom_chars(A, Cs)
    ;   atom_chars(A, Cs), append(Cs, ['.'], Cs1), read_from_chars(Cs1, T)
    ).
atom_to_term(A, T, Bindings) :-
    atom_chars(A, Cs), append(Cs, ['.'], Cs1), read_term_from_chars(Cs1, T, [variable_names(Bindings)]).
read_term_from_atom(A, T, _) :- term_to_atom(T, A).

atom_number(A, N) :-
    (   var(A) -> number(N), number_chars(N, Cs), atom_chars(A, Cs)
    ;   atom_chars(A, Cs), catch(number_chars(N0, Cs), _, fail), N = N0
    ).

downcase_atom(A, L) :- atom_chars(A, Cs), maplist(lower_char, Cs, Ls), atom_chars(L, Ls).
upcase_atom(A, U) :- atom_chars(A, Cs), maplist(upper_char, Cs, Us), atom_chars(U, Us).
%% Case mapping by code point (ASCII + Latin-1); Scryer's char_type/2 case mapping proved unreliable here.
lower_char(C, L) :- char_code(C, X), ( (X >= 0'A, X =< 0'Z ; X >= 192, X =< 222, X =\= 215) -> Y is X + 32, char_code(L, Y) ; L = C ).
upper_char(C, U) :- char_code(C, X), ( (X >= 0'a, X =< 0'z ; X >= 224, X =< 254, X =\= 247) -> Y is X - 32, char_code(U, Y) ; U = C ).

concat_atom_codes(L, A) :- atomic_list_concat(L, A).

%% ---------------------------------------------------------------- format (accept atoms, SWI-only directives)

fmt_chars(F, Cs) :- atom(F), !, atom_chars(F, Cs).
fmt_chars(F, Cs) :- is_list(F), F = [C|_], integer(C), !, maplist(code_char, F, Cs).
fmt_chars(F, F).

norm_args(A, L) :- ( is_list(A) -> L = A ; L = [A] ).

%% ~W (term + write options) is SWI-only: rewrite to ~w over a pre-rendered atom.
pre(Cs0, Args0, Cs, Args) :- pre_(Cs0, Args0, Cs, Args).
pre_([], A, [], A).
pre_(['~','W'|T], [Term, Opts|As], ['~','a'|T2], [Atom|As2]) :- !,
    write_term_to_chars(Term, Opts, Cs), atom_chars(Atom, Cs), pre_(T, As, T2, As2).
pre_(['~','~'|T], As, ['~','~'|T2], As2) :- !, pre_(T, As, T2, As2).
pre_(['~','c'|T], [Code|As], ['~','a'|T2], [Ch|As2]) :- !, char_code(Ch, Code), pre_(T, As, T2, As2).
pre_(['~'|T0], [N|As], ['~','a'|T2], [Atom|As2]) :-      % ~<radix>r  (SWI), e.g. ~16r
    digits(T0, Ds, ['r'|T]), Ds \== [], !, number_chars(R, Ds),
    radix_chars(N, R, Cs), atom_chars(Atom, Cs), pre_(T, As, T2, As2).
pre_(['~',D|T], [Term|As], ['~','a'|T2], [Atom|As2]) :- custom_fp(D, M:G), !,
    functor(G, N, _), G2 =.. [N, default, Term],
    swi_with_output_to_atom(M:G2, Atom), pre_(T, As, T2, As2).
pre_(['~',D|T], [A|As], ['~',D|T2], [A|As2]) :- consumes(D), !, pre_(T, As, T2, As2).
pre_(['~',D,X|T], [A|As], ['~',D,X|T2], [A|As2]) :- char_type(D, decimal_digit), consumes(X), !, pre_(T, As, T2, As2).
pre_([C|T], As, [C|T2], As2) :- pre_(T, As, T2, As2).
consumes(D) :- member(D, [w,q,a,d,s,f,e,g,c,r,p,i,'R','D']).

swi_format(F, A) :- fmt_chars(F, Cs0), norm_args(A, As0), pre(Cs0, As0, Cs, As), format:format(Cs, As).
swi_format(S, F, A) :- fmt_chars(F, Cs0), norm_args(A, As0), pre(Cs0, As0, Cs, As), format:format(S, Cs, As).
%% SWI: T =.. [X] with compound X simply fails; ISO/Scryer raise type_error.
swi_univ(T, L) :- catch(T =.. L, error(type_error(_, _), _), fail).

swi_format_atom(F, A, Atom) :-
    fmt_chars(F, Cs0), norm_args(A, As0), pre(Cs0, As0, Cs, As),
    phrase(format:format_(Cs, As), Out), atom_chars(Atom, Out).

%% General with_output_to/2: Scryer (as of 0.10) has no user-visible memory output stream,
%% so we route through a temp file. Hot paths are rewritten to swi_format_atom/3 instead.
swi_with_output_to_atom(G, Atom) :-
    tmp_path(ape_out_, P),
    current_output(Old),
    setup_call_cleanup(open(P, write, S),
        ( set_output(S), ( catch(G, E, (set_output(Old), throw(E))) -> R = true ; R = false ), set_output(Old) ),
        close(S)),
    R == true,
    setup_call_cleanup(open(P, read, In), read_all_chars(In, Cs), close(In)),
    rm_tmp(P),
    atom_chars(Atom, Cs).

read_all_chars(In, Cs) :- get_char(In, C), ( C == end_of_file -> Cs = [] ; Cs = [C|T], read_all_chars(In, T) ).

writef(A) :- write(A).
writef(F, Args) :-   % SWI writef: %w substitutes args, \n newline
    atom_chars(F, Cs), writef_(Cs, Args).
writef_([], _).
writef_(['%','w'|T], [A|As]) :- !, write(A), writef_(T, As).
writef_(['%','d'|T], [A|As]) :- !, write(A), writef_(T, As).
writef_(['%','r'|T], [A, N|As]) :- !, rep_write(N, A), writef_(T, As).
writef_(['%','s'|T], [A|As]) :- !, ( A = [C|_], integer(C) -> atom_codes(X, A) ; X = A ), write(X), writef_(T, As).
writef_(['\\','n'|T], As) :- !, nl, writef_(T, As).
writef_(['\\','l'|T], As) :- !, nl, writef_(T, As).
writef_([C|T], As) :- put_char(C), writef_(T, As).

writeln(X) :- write(X), nl.
print_message(_, _).

%% ---------------------------------------------------------------- global variables

nb_setval(K, V) :- bb_put(K, V).
%% SWI raises existence_error for unset keys (APE's counters rely on catching it); bb_get/2 fails.
nb_getval(K, V) :- ( catch(bb_get(K, V0), _, fail) -> V = V0 ; throw(error(existence_error(variable, K), nb_getval/2)) ).
b_setval(K, V) :- bb_b_put(K, V).
b_getval(K, V) :- nb_getval(K, V).

flag(K, Old, New) :-
    atom_concat('$flag_', K, BK),
    ( catch(bb_get(BK, Old0), _, fail) -> Old = Old0 ; Old = 0 ),
    V is New, bb_put(BK, V).

gensym(Base, A) :-
    atom_concat('$gensym_', Base, K),
    ( catch(bb_get(K, N0), _, fail) -> true ; N0 = 0 ),
    N is N0 + 1, bb_put(K, N), number_chars(N, NC), atom_chars(NA, NC), atom_concat(Base, NA, A).
reset_gensym(Base) :- atom_concat('$gensym_', Base, K), bb_put(K, 0).

%% ---------------------------------------------------------------- lists (SWI library(lists)/apply)

last([X], X) :- !.
last([_|T], X) :- last(T, X).
subtract([], _, []).
subtract([H|T], L, R) :- ( memberchk(H, L) -> R = R1 ; R = [H|R1] ), subtract(T, L, R1).
delete([], _, []).
delete([H|T], X, R) :- ( H \= X -> R = [H|R1] ; R = R1 ), delete(T, X, R1).
exclude(_, [], []).
exclude(P, [H|T], R) :- ( call(P, H) -> R = R1 ; R = [H|R1] ), exclude(P, T, R1).
include(_, [], []).
include(P, [H|T], R) :- ( call(P, H) -> R = [H|R1] ; R = R1 ), include(P, T, R1).
partition(_, [], [], []).
partition(P, [H|T], I, E) :- ( call(P, H) -> I = [H|I1], E = E1 ; I = I1, E = [H|E1] ), partition(P, T, I1, E1).
memberchk_eq(X, [Y|Ys]) :- ( X == Y -> true ; memberchk_eq(X, Ys) ).
flatten(L, F) :- flatten_(L, [], F0), !, F = F0.
flatten_(V, T, [V|T]) :- var(V), !.
flatten_([], T, T) :- !.
flatten_([H|R], T, F) :- !, flatten_(H, F1, F), flatten_(R, T, F1).
flatten_(A, T, [A|T]).
sumlist(L, S) :- sum_list(L, S).
max_list([H|T], M) :- foldl(max_, T, H, M).
min_list([H|T], M) :- foldl(min_, T, H, M).

ignore(G) :- ( call(G) -> true ; true ).
is_list(X) :- var(X), !, fail.
is_list([]).
is_list([_|T]) :- is_list(T).

subsumes(G, S) :- subsumes_term(G, S), G = S.

numbervars(T, S, E) :- terms:numbervars(T, S, E).

%% ---------------------------------------------------------------- misc runtime

debug(_, _, _).
debug(_).
nodebug(_).
garbage_collect.
style_check(_).
succ_or_zero(_).

get_time(T) :- T = 0.0.        % APE only uses this for durations in its XML header; host (Rust) times it.

%% Scryer has call_with_inference_limit/3 but no wall-clock limit; approximate with an
%% inference budget. The Rust host enforces the real wall-clock timeout.
call_with_time_limit(Secs, G) :-
    Limit is truncate(Secs * 20000000),
    call_with_inference_limit(G, Limit, R),
    ( R == inference_limit_exceeded -> throw(time_limit_exceeded) ; true ).

chars_atom(P, X) :- atom_chars(X, P).
code_char(Co, Ch) :- char_code(Ch, Co).
max_(X, A0, A) :- A is max(A0, X).
min_(X, A0, A) :- A is min(A0, X).

%% SWI format_predicate/2: user-defined ~X directives.
format_predicate(C, MG) :- atom_concat('$fp_', C, K), bb_put(K, MG).
custom_fp(D, MG) :- atom(D), atom_concat('$fp_', D, K), catch(bb_get(K, MG), _, fail).

%% SWI must_be/2 types Scryer's library(error) lacks.
swi_must_be(codes, X) :- !, must_be(list, X), ( maplist(integer, X) -> true ; type_error(codes, X) ).
swi_must_be(chars, X) :- !, must_be(list, X), ( maplist(atom, X) -> true ; type_error(chars, X) ).
swi_must_be(oneof(L), X) :- !, must_be(atom, X), ( memberchk(X, L) -> true ; domain_error(oneof(L), X) ).
swi_must_be(T, X) :- must_be(T, X).

%% SWI accepts any atomic (numbers, strings) where ISO/Scryer demands atoms.
atomize(X, A) :- ( atom(X) -> A = X ; number(X) -> number_chars(X, Cs), atom_chars(A, Cs) ; is_list(X) -> to_chars(X, Cs), atom_chars(A, Cs) ; A = X ).
swi_atom_concat(A, B, C) :-
    ( (A == [] ; B == []) -> type_error(atom, []) ; true ),
    ( nonvar(A) -> atomize(A, A1) ; A1 = A ), ( nonvar(B) -> atomize(B, B1) ; B1 = B ),
    ( nonvar(C) -> atomize(C, C1) ; C1 = C ), atom_concat(A1, B1, C1).
%% SWI-7: [] is not an atom (atom_length([],0), others raise type_error(atom,[])).
swi_atom_length(A, L) :- ( A == [] -> L = 0 ; atomize(A, A1), atom_length(A1, L) ).
swi_atom_codes(A, Cs) :- ( A == [] -> type_error(atom, []) ; nonvar(A) -> atomize(A, A1), atom_codes(A1, Cs) ; atom_codes(A, Cs) ).
swi_atom_chars(A, Cs) :- ( A == [] -> type_error(atom, []) ; nonvar(A) -> atomize(A, A1), atom_chars(A1, Cs) ; atom_chars(A, Cs) ).

digits([D|T], [D|Ds], R) :- char_type(D, decimal_digit), !, digits(T, Ds, R).
digits(R, [], R).
radix_chars(N, R, Cs) :- integer(N), N < 0, !, M is -N, radix_chars(M, R, Cs0), Cs = ['-'|Cs0].
radix_chars(N, R, Cs) :- rc(N, R, [], Cs).
rc(N, R, Acc, Cs) :- D is N mod R, Q is N // R, nth0(D, ['0','1','2','3','4','5','6','7','8','9',a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r,s,t,u,v,w,x,y,z], C),
    ( Q =:= 0 -> Cs = [C|Acc] ; rc(Q, R, [C|Acc], Cs) ).

%% SWI open_string/2: read stream over text. Scryer has no user-level string input
%% stream constructor, so spill to a temp file (only used for user lexicons).
open_string(Text, Stream) :-
    %% Text is written verbatim: trailing layout after the final '.' (AceWiki's ulex ends in ". ")
    %% reads as end_of_file since the scryer-prolog fix on dhaneshkk/scryer-prolog
    %% fix/ape-rs-findings (upstream raised syntax_error(incomplete_reduction)).
    to_chars(Text, Cs),
    tmp_path(ape_in_, P),
    setup_call_cleanup(open(P, write, W), maplist(put_char(W), Cs), close(W)),
    open(P, read, Stream),
    rm_tmp(P).                                  % unlinked while open: still readable on POSIX

rep_write(N, _) :- N =< 0, !.
rep_write(N, A) :- write(A), N1 is N - 1, rep_write(N1, A).

swi_random(N, X) :- random_integer(0, N, X).

%% SWI arg/3 enumerates argument positions when N is unbound; ISO raises instantiation_error.
swi_arg(N, T, A) :- ( var(N) -> functor(T, _, Ar), between(1, Ar, N), arg(N, T, A) ; arg(N, T, A) ).

%% SWI writes '$'-prefixed alphanumeric atoms unquoted (e.g. TPTP's $true); Scryer quotes them.
swi_write_term(T, Opts) :-
    ( atom(T), memberchk(quoted(true), Opts), atom_chars(T, ['$'|Cs]), Cs = [C|_],
      char_type(C, alpha), maplist(alnum_, Cs) -> write(T) ; write_term(T, Opts) ).
alnum_(C) :- ( char_type(C, alnum) -> true ; C == '_' ).

%% Temp-file names must be unique per *engine*: several Scryer machines run in one process
%% (ape-parity shards, server pools) and each has its own gensym counter. The Rust host sets
%% swi_tmp_prefix to a per-engine unique path prefix right after loading (Ape::new).
tmp_path(Base, P) :-
    ( catch(bb_get(swi_tmp_prefix, Pre), _, fail) -> true ; Pre = '/tmp/ape_' ),
    gensym(Base, N), atom_concat(Pre, N, P).
rm_tmp(P) :- atom_chars(P, Cs), catch(delete_file(Cs), _, true).

