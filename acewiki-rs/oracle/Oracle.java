// Oracle.java - runs the ORIGINAL AceWiki engine (Java, HermiT, ChartParser) headlessly and dumps
// reference results as JSON lines, so acewiki-rs can be scored against it (MCC / exact parity).
//
//   java -cp classes:lib/* Oracle <dataDir> <wiki> <profile:OWL2RL|OWL2EL|none> <apeURL> <out.jsonl> [parser]
import ch.uzh.ifi.attempto.acewiki.core.*;
import ch.uzh.ifi.attempto.acewiki.aceowl.*;
import ch.uzh.ifi.attempto.acewiki.owl.*;
import ch.uzh.ifi.attempto.base.*;
import ch.uzh.ifi.attempto.chartparser.*;
import java.io.*;
import java.util.*;

public class Oracle {
    static PrintWriter out;
    static String q(String s) {
        if (s == null) return "null";
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"': b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n"); break;
                case '\r': b.append("\\r"); break;
                case '\t': b.append("\\t"); break;
                default: if (c < 0x20) b.append(String.format("\\u%04x", (int) c)); else b.append(c);
            }
        }
        return b.append("\"").toString();
    }
    static String arr(Collection<String> xs) {
        List<String> l = new ArrayList<String>(xs); Collections.sort(l);
        StringBuilder b = new StringBuilder("[");
        for (int i = 0; i < l.size(); i++) { if (i > 0) b.append(","); b.append(q(l.get(i))); }
        return b.append("]").toString();
    }
    static String word(OntologyElement oe) { return oe == null ? null : oe.getWord(); }
    static List<String> words(List<? extends OntologyElement> l) {
        List<String> r = new ArrayList<String>();
        if (l != null) for (OntologyElement e : l) if (e != null) r.add(e.getWord());
        return r;
    }
    static String sha(Collection<String> xs) throws Exception {
        List<String> l = new ArrayList<String>(xs); Collections.sort(l);
        java.security.MessageDigest md = java.security.MessageDigest.getInstance("SHA-256");
        for (String s : l) { md.update(s.getBytes("UTF-8")); md.update((byte) 10); }
        StringBuilder b = new StringBuilder();
        for (byte x : md.digest()) b.append(String.format("%02x", x));
        return b.toString();
    }

    public static void main(String[] a) throws Exception {
        String dataDir = a[0], wiki = a[1], profile = a[2], apeUrl = a[3];
        String mode = a.length > 5 ? a[5] : "reason";   // reason | parser | both
        out = new PrintWriter(new OutputStreamWriter(new FileOutputStream(a[4]), "UTF-8"));
        Map<String, String> apeParams = new HashMap<String, String>();
        apeParams.put("context:apetype", "webservice");
        apeParams.put("context:apewebservice", apeUrl);
        APE.setParameters(apeParams);
        Map<String, String> p = new HashMap<String, String>();
        p.put("ontology", wiki);
        p.put("baseuri", "http://attempto.ifi.uzh.ch/acewiki/");
        p.put("reasoner", "HermiT");
        p.put("owl_profile", profile);
        p.putAll(apeParams);
        // work on a copy: FileBasedStorage re-saves every element on load
        FileBasedStorage storage = new FileBasedStorage(dataDir);
        long t0 = System.currentTimeMillis();
        Ontology o = storage.getOntology(wiki, p);
        long tLoad = System.currentTimeMillis() - t0;
        AceWikiReasoner r = o.getReasoner();
        out.println("{\"kind\":\"meta\",\"wiki\":" + q(wiki) + ",\"profile\":" + q(profile) + ",\"uri\":" + q(o.getURI())
            + ",\"load_ms\":" + tLoad + ",\"elements\":" + o.getOntologyElements().size()
            + ",\"consistent\":" + r.isConsistent() + "}");
        List<OntologyElement> els = new ArrayList<OntologyElement>(o.getOntologyElements());
        Collections.sort(els, new Comparator<OntologyElement>() {
            public int compare(OntologyElement x, OntologyElement y) { return Long.compare(x.getId(), y.getId()); } });
        for (OntologyElement oe : els) {
            if (mode.equals("parser") || mode.equals("bench")) continue;
            StringBuilder b = new StringBuilder("{\"kind\":\"element\",\"id\":" + oe.getId() + ",\"type\":" + q(oe.getInternalType())
                + ",\"word\":" + q(oe.getWord()) + ",\"words\":" + arr(Arrays.asList(nz(oe.getWords()))));
            if (oe instanceof Individual) {
                b.append(",\"concepts\":" + arr(words(r.getConcepts((Individual) oe))));
            }
            if (oe instanceof Concept) {
                Concept c = (Concept) oe;
                b.append(",\"individuals\":" + arr(words(r.getIndividuals(c))));
                b.append(",\"super\":" + arr(words(r.getSuperConcepts(c))));
                b.append(",\"sub\":" + arr(words(r.getSubConcepts(c))));
                b.append(",\"satisfiable\":" + r.isSatisfiable(c));
            }
            b.append("}");
            out.println(b);
            int si = 0;
            for (Sentence s : oe.getArticle().getSentences()) {
                ACESentence as = (ACESentence) s;
                List<String> toks = new ArrayList<String>();
                for (TextElement te : as.getTextElements()) toks.add(te.getText());
                StringBuilder sb = new StringBuilder("{\"kind\":\"sentence\",\"element\":" + oe.getId() + ",\"idx\":" + (si++)
                    + ",\"serialized\":" + q(as.serialize()) + ",\"text\":" + q(as.getText(null))
                    + ",\"tokens\":" + jarr(toks) + ",\"orig_tokens\":" + jarr(orig(as))
                    + ",\"question\":" + (s instanceof Question)
                    + ",\"integrated\":" + s.isIntegrated() + ",\"owl\":" + as.isOWL() + ",\"owlswrl\":" + as.isOWLSWRL()
                    + ",\"reasonable\":" + as.isReasonable());
                if (s instanceof Question) {
                    List<AnswerElement> ans = r.getAnswer((Question) s);
                    List<String> aw = new ArrayList<String>();
                    if (ans != null) for (AnswerElement ae : ans) aw.add(ae instanceof OntologyElement ? ((OntologyElement) ae).getWord() : ae.toString());
                    sb.append(",\"answer\":" + (ans == null ? "null" : arr(aw)));
                }
                sb.append("}");
                out.println(sb);
            }
        }
        if (mode.equals("bench")) { parserBench(o, els, a.length > 6 ? Integer.parseInt(a[6]) : 5); System.exit(0); }
        if (!mode.equals("reason")) parserOptions(o, els);
        out.close();
        System.err.println("oracle done: " + wiki + " " + profile + " load " + tLoad + " ms");
        System.exit(0);
    }
    static List<String> orig(ACESentence as) { List<String> l = new ArrayList<String>(); for (TextElement te : as.getTextElements()) l.add(te.getOriginalText()); return l; }
    static String[] nz(String[] xs) { List<String> l = new ArrayList<String>(); for (String x : xs) if (x != null) l.add(x); return l.toArray(new String[0]); }
    static String jarr(List<String> l) {
        StringBuilder b = new StringBuilder("[");
        for (int i = 0; i < l.size(); i++) { if (i > 0) b.append(","); b.append(q(l.get(i))); }
        return b.append("]").toString();
    }

    /** Predictive-editor benchmark: same work per prefix as acewiki-rs examples/parser_bench.rs. */
    static void parserBench(Ontology o, List<OntologyElement> els, int rounds) throws Exception {
        ACEHandler h = (ACEHandler) o.getEngine().getLanguageHandler(o.getEngine().getLanguages()[0]);
        List<List<String>> sents = new ArrayList<List<String>>();
        for (OntologyElement oe : els) for (Sentence s : oe.getArticle().getSentences()) {
            List<String> toks = new ArrayList<String>();
            for (TextElement te : ((ACESentence) s).getTextElements()) toks.add(te.getOriginalText());
            sents.add(toks);
        }
        long best = Long.MAX_VALUE;
        for (int r = 0; r < rounds; r++) {
            long t0 = System.nanoTime(); long prefixes = 0, opts = 0;
            for (List<String> toks : sents) {
                ChartParser cp = (ChartParser) h.getPredictiveParser();
                for (int k = 0; k <= toks.size(); k++) {
                    if (k > 0) cp.addToken(toks.get(k - 1));
                    List<CPAbstractOption> ab = new ArrayList<CPAbstractOption>(cp.getAbstractOptions());
                    for (CPAbstractOption ao : ab) ao.toString();
                    opts += ab.size() + cp.getConcreteOptions().size();
                    cp.isComplete(); cp.getReference();
                    prefixes++;
                }
            }
            long dt = System.nanoTime() - t0; best = Math.min(best, dt);
            System.err.printf("java round %d: %d sentences, %d prefixes, %d options, %.3fs (%.1f us/prefix)%n", r, sents.size(), prefixes, opts, dt / 1e9, dt / 1e3 / prefixes);
        }
        System.err.printf("java best %.3fs%n", best / 1e9);
    }

    /** Next-token options of the real ChartParser (with AceWiki's dynamic lexicon) at every prefix. */
    static void parserOptions(Ontology o, List<OntologyElement> els) throws Exception {
        ACEHandler h = (ACEHandler) o.getEngine().getLanguageHandler(o.getEngine().getLanguages()[0]);
        for (OntologyElement oe : els) {
            int si = 0;
            for (Sentence s : oe.getArticle().getSentences()) {
                ACESentence as = (ACESentence) s;
                ChartParser cp = (ChartParser) h.getPredictiveParser();
                List<String> toks = new ArrayList<String>();
                for (TextElement te : as.getTextElements()) toks.add(te.getOriginalText());   // what PreditorWindow feeds
                for (int k = 0; k <= toks.size(); k++) {
                    Set<String> abs = new TreeSet<String>();
                    for (CPAbstractOption ao : cp.getAbstractOptions()) abs.add(ao.toString());
                    Set<String> con = new TreeSet<String>();
                    for (CPConcreteOption co : cp.getConcreteOptions()) con.add(co.getWord() + "|" + co.getCategoryName());
                    out.println("{\"kind\":\"options\",\"element\":" + oe.getId() + ",\"idx\":" + si + ",\"k\":" + k
                        + ",\"complete\":" + cp.isComplete() + ",\"n_concrete\":" + con.size() + ",\"concrete_sha\":" + q(sha(con))
                        + ",\"abstract\":" + jarr(new ArrayList<String>(abs)) + ",\"ref\":" + cp.getReference() + "}");
                    if (k < toks.size()) cp.addToken(toks.get(k));
                }
                si++;
            }
        }
    }
}
