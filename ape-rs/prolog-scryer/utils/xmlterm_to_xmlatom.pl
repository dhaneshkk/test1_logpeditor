/*  xmlterm_to_xmlatom.pl — pure-Prolog replacement for APE's SWI-only version.
    The original wrote via SWI library(sgml_write) into a memory file. This port reproduces
    sgml_write's XML layout rules exactly (attributes wrapped when their total length > 60,
    2-space nesting, indentation emitted as tabs for each 8 columns, XML entity escaping,
    '/>' for empty elements, UTF-8 header), so outputs are byte-identical to SWI-APE.
*/
:- module(xmlterm_to_xmlatom, [xmlterm_to_xmlatom/2, xmlterm_to_xmlatom/3]).
:- use_module(library(lists)).
:- use_module(library(dcgs)).
:- use_module('../swi_compat', [swi_atom_length/2, swi_atom_codes/2, is_list/1]).

xmlterm_to_xmlatom(X, A) :- xmlterm_to_xmlatom(X, [], A).
xmlterm_to_xmlatom(X, Options, A) :-
    ( memberchk(header(false), Options) -> H = [] ; atom_chars('<?xml version="1.0" encoding="UTF-8"?>\n\n', H) ),
    phrase(emit(X, 0), Body),
    append(H, Body, Cs), atom_chars(A, Cs).

emit(V, _) --> { var(V), !, throw(error(instantiation_error, xml_write/3)) }.
emit([], _) --> !.
emit([H|T], I) --> !, emit(H, I), emit(T, I).
emit(element(N, Atts, C), I) --> !, element(N, Atts, C, I).
emit(pi(PI), _) --> !, "<?", text(PI, ""), "?>".
emit(CDATA, _) --> { atomic(CDATA) }, !, content_text(CDATA).
emit(X, _) --> { throw(error(type_error(xml_dom, X), _)) }.

element(N, Atts, C, I) -->
    { att_length(Atts, 0, L), ( L > 60 -> Sep = nl, AI is I + 4 ; Sep = sp, AI = I ) },
    "<", name(N), attributes(Atts, Sep, AI), content(C, N, I).

attributes([], _, _) --> [].
attributes([N=V|T], Sep, I) --> ( { Sep == nl } -> indent(I) ; " " ), name(N), "=", attval(V), attributes(T, Sep, I).

attval(Vs) --> { is_list(Vs) }, !, "\"", attlist(Vs), "\"".
attval(V) --> { atom(V) }, !, "\"", text(V, att), "\"".
attval(V) --> { number(V) }, !, "\"", { number_chars(V, Cs) }, seq(Cs), "\"".

attlist([]) --> [].
attlist([H]) --> !, text(H, att).
attlist([H|T]) --> text(H, att), " ", attlist(T).

content([], _, _) --> !, "/>".
content([CDATA], N, _) --> { atomic(CDATA) }, !, ">", content_text(CDATA), close(N).
content(C, N, I) --> { element_content(C, Es) }, !, ">", { I2 is I + 2 }, children(Es, I2), indent(I), close(N).
content(C, N, I) --> ">", mixed(C, I), close(N).

children([], _) --> [].
children([E|Es], I) --> indent(I), emit(E, I), children(Es, I).

mixed([], _) --> [].
mixed([H|T], I) --> ( { atomic(H) } -> content_text(H)
                    ; { H = element(_,_,_) } -> emit(H, I)
                    ; { throw(error(type_error(sgml_content, H), _)) } ), mixed(T, I).

element_content([], []).
element_content([element(N,A,C)|T0], [element(N,A,C)|T]) :- !, element_content(T0, T).
element_content([B|T0], T) :- atom(B), atom_chars(B, Cs), maplist(blank, Cs), element_content(T0, T).
blank(C) :- memberchk(C, [' ', '\t', '\n', '\r']).

close(N) --> "</", name(N), ">".
name(N) --> { atom_chars(N, Cs) }, seq(Cs).

content_text(A) --> { atom(A) }, !, text(A, content).
content_text(X) --> { number(X), number_chars(X, Cs) }, seq(Cs).

text(A, Mode) --> { swi_atom_codes(A, Codes) }, esc(Codes, Mode).
esc([], _) --> [].
esc([C|Cs], M) --> ( { escape(M, C) } -> entity(C) ; { char_code(Ch, C) }, [Ch] ), esc(Cs, M).

escape(att, C) :- memberchk(C, [0'", 0'<, 0'&, 13, 10, 9]).
escape(content, C) :- memberchk(C, [0'<, 0'&, 0'>, 13]).
escape("", _) :- fail.

entity(0'<) --> !, "&lt;".
entity(0'>) --> !, "&gt;".
entity(0'&) --> !, "&amp;".
entity(0'") --> !, "&quot;".
entity(C) --> "&#x", { hex(C, Hs) }, seq(Hs), ";".
hex(N, Cs) :- hx(N, [], Cs).
hx(N, Acc, Cs) :- D is N mod 16, Q is N // 16, nth0(D, ['0','1','2','3','4','5','6','7','8','9','A','B','C','D','E','F'], H),
    ( Q =:= 0 -> Cs = [H|Acc] ; hx(Q, [H|Acc], Cs) ).

%% indentation: newline, then Indent//8 tabs and Indent mod 8 spaces (SWI emit_indent/2)
indent(I) --> "\n", { T is I // 8, S is I mod 8 }, rep(T, '\t'), rep(S, ' ').
rep(0, _) --> !.
rep(N, C) --> [C], { N1 is N - 1 }, rep(N1, C).

att_length([], L, L).
att_length([N=V|T], L0, L) :- swi_atom_length(N, NL), vlen(V, VL), L1 is L0 + 1 + NL + VL + 3, att_length(T, L1, L).
vlen(V, L) :- is_list(V), !, vl(V, 0, L).
vlen(V, L) :- swi_atom_length(V, L).
vl([], L, L).
vl([H|T], L0, L) :- swi_atom_length(H, HL), ( L0 == 0 -> L1 is L0 + HL ; L1 is L0 + HL + 1 ), vl(T, L1, L).
