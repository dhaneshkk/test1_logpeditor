/*  generate_json.pl - Codeco backend emitting the grammar as JSON for acewiki-rs.
    Mirrors codeco/generate_java.pl rule by rule (same operator table, same category mapping),
    so the Rust chart parser receives exactly the rules the Java ChartParser was built from.

    swipl -s generate_json.pl -g "generate_json('acewiki_grammar.pl','acewiki_grammar.json')" -t halt
*/
:- module(generate_json, [generate_json/2]).
:- use_module(library(http/json)).
:- op(1200, xfx, '=>').
:- op(1200, xfx, '~>').
:- op(600, xfx, ':').
:- op(500, yfx, '+').
:- op(300, fx, '$').
:- op(300, fx, '#').
:- op(0, fx, '+').
:- op(0, xfx, '>').
:- op(0, xfx, '<').

generate_json(In, Out) :-
    open(In, read, S),
    read_terms(S, Ts), close(S),
    convlist(term_json, Ts, Items),
    partition([X]>>(get_dict(kind, X, rule)), Items, Rules, Lex),
    open(Out, write, O, [encoding(utf8)]),
    json_write_dict(O, _{rules: Rules, lexrules: Lex}, [width(0)]),
    nl(O), close(O),
    length(Rules, NR), length(Lex, NL),
    format(user_error, "~w: ~d grammar rules, ~d lexical rules~n", [Out, NR, NL]).

read_terms(S, Ts) :-
    read_term(S, T, [module(generate_json)]),
    ( T == end_of_file -> Ts = [] ; numbervars(T, 0, _), Ts = [T|R], read_terms(S, R) ).

term_json(_:_, _) :- !, fail.                                    % title/section/paragraph
term_json($ Head => [Body], _{kind: lex, cat: C, word: W, ann: A}) :- !,
    head(  $ Head, A, [C0]), C = C0, Body =.. [W0], atom_string(W0, W).
term_json(Head => Body, R)  :- !, rule(Head, Body, false, R).
term_json(Head ~> Body, R)  :- !, rule(Head, Body, true,  R).
term_json(T, _) :- format(user_error, "WARNING: cannot process ~q~n", [T]), fail.

rule(Head, Body, SC, _{kind: rule, scope_closing: SC, ann: A, cats: [H|Bs]}) :-
    head(Head, A, [H]), cats(Body, Bs).

head({Ann}:Cat, A, Cs) :- !, anns(Ann, Ps), dict_create(A, _, Ps), cat(Cat, Cs).
head(Cat, _{}, Cs) :- cat(Cat, Cs).

anns((X, Xs), [P|Ps]) :- !, ann(X, P), anns(Xs, Ps).
anns(X, [P]) :- ann(X, P).
ann(N:V, N-J) :- struct(V, J).
struct('$VAR'(N), _{var: N}) :- !.
struct(A, S) :- atom(A), !, atom_string(A, S).
struct(T, _{f: F, args: As}) :- T =.. [F0|Args], atom_string(F0, F), maplist(struct, Args, As).

cats((C, Rest), Out) :- !, cat(C, A), cats(Rest, B), append(A, B, Out).
cats([], []) :- !.
cats(C, Out) :- cat(C, Out).

%% cat(+Codeco, -ListOfJsonCats)   (a terminal list expands to several terminals)
cat([], []) :- !.
cat([T|Rest], [_{t: term, name: N}|More]) :- !, T =.. [N0], atom_string(N0, N), cat(Rest, More).
cat($ C, [_{t: pre, name: N, f: F}]) :- !, C =.. [N0|Fs], atom_string(N0, N), feats(Fs, F).
cat(# Pos, [_{t: non, name: "#", f: F}]) :- !, feats([pos:Pos], F).
cat(C, [_{t: bref, pos: P, neg: Ng}]) :-
    C =.. ['<'|Ts], Ts \== [], \+ member(_:_, Ts), !, bref(Ts, P, Ng).
cat(C, [_{t: bref, pos: [F], neg: []}]) :- C =.. ['<'|Fs], !, feats(Fs, F).
cat(C, [_{t: non, name: N, f: F}]) :- C =.. [N0|Fs], atom_string(N0, N), feats(Fs, F).

bref([], [], []).
bref([T|Ts], [F|P], N) :- T =.. ['+'|Fs], !, feats(Fs, F), bref(Ts, P, N).
bref([T|Ts], P, [F|N]) :- T =.. ['-'|Fs], feats(Fs, F), bref(Ts, P, N).

feats(Fs, D) :- maplist(feat, Fs, Ps), dict_create(D, _, Ps).
feat(Name:'$VAR'(N), Name-_{var: N}) :- !.
feat(Name:V, Name-_{c: S}) :- atom_string(V, S).
