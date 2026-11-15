// Minimal OWLlink (HTTP/XML binding) server over OWLAPI 3 + HermiT, JDK-only HTTP (no Jetty).
// Implements the requests acewiki-rs' OWLlink client uses: CreateKB, ReleaseKB, Tell,
// IsKBSatisfiable, IsClassSatisfiable, GetFlattenedTypes, GetFlattenedInstances,
// GetSuperClasses, GetSubClasses (non-direct), GetDescription.
//   javac -cp "lib/*" OwlLinkServer.java && java -cp ".:lib/*" OwlLinkServer 8088
import com.sun.net.httpserver.HttpServer;
import java.io.*;
import java.net.InetSocketAddress;
import java.util.*;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.*;
import javax.xml.transform.dom.DOMSource;
import javax.xml.transform.stream.StreamResult;
import org.semanticweb.HermiT.Reasoner;
import org.semanticweb.owlapi.apibinding.OWLManager;
import org.semanticweb.owlapi.io.StringDocumentSource;
import org.semanticweb.owlapi.model.*;
import org.semanticweb.owlapi.reasoner.*;
import org.w3c.dom.*;

public class OwlLinkServer {
    static final String OWL = "http://www.w3.org/2002/07/owl#";
    static final Map<String, OWLOntology> kbs = new HashMap<>();
    static final Map<String, OWLReasoner> reasoners = new HashMap<>();
    static final OWLOntologyManager man = OWLManager.createOWLOntologyManager();
    static final OWLDataFactory df = man.getOWLDataFactory();
    static int tmp = 0;

    public static void main(String[] a) throws Exception {
        int port = a.length > 0 ? Integer.parseInt(a[0]) : 8080;
        HttpServer s = HttpServer.create(new InetSocketAddress("127.0.0.1", port), 0);
        s.createContext("/", ex -> {
            byte[] out;
            try { out = handle(ex.getRequestBody().readAllBytes()).getBytes("UTF-8"); }
            catch (Throwable t) { out = ("<?xml version=\"1.0\"?><ResponseMessage xmlns=\"http://www.owllink.org/owllink#\"><Error error=\"" + esc(String.valueOf(t)) + "\"/></ResponseMessage>").getBytes("UTF-8"); }
            ex.getResponseHeaders().add("Content-Type", "text/xml; charset=utf-8");
            ex.sendResponseHeaders(200, out.length);
            ex.getResponseBody().write(out); ex.close();
        });
        s.start();
        System.err.println("OWLlink (HermiT " + new Reasoner.ReasonerFactory().getReasonerName() + ") on http://127.0.0.1:" + port);
    }

    static synchronized String handle(byte[] body) throws Exception {
        DocumentBuilderFactory f = DocumentBuilderFactory.newInstance(); f.setNamespaceAware(true);
        Element root = f.newDocumentBuilder().parse(new ByteArrayInputStream(body)).getDocumentElement();
        StringBuilder r = new StringBuilder("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ResponseMessage xmlns=\"http://www.owllink.org/owllink#\" xmlns:owl=\"" + OWL + "\">");
        for (Element q : children(root)) {
            try { r.append(one(q)); }
            catch (Throwable t) { r.append("<Error error=\"").append(esc(String.valueOf(t))).append("\"/>"); }
        }
        return r.append("</ResponseMessage>").toString();
    }

    static String one(Element q) throws Exception {
        String kb = q.getAttribute("kb");
        switch (q.getLocalName()) {
            case "GetDescription": return "<Description name=\"acewiki-rs OWLlink/HermiT\"><ProtocolVersion major=\"1\" minor=\"0\"/><ReasonerVersion major=\"1\" minor=\"3\"/></Description>";
            case "CreateKB": {
                if (kb.isEmpty()) kb = "urn:kb:" + (tmp++);
                release(kb);
                kbs.put(kb, man.createOntology(IRI.create(kb)));
                return "<ResponseWithKB kb=\"" + esc(kb) + "\"/>";
            }
            case "ReleaseKB": release(kb); return "<OK/>";
            case "Tell": {
                OWLOntology o = kb(kb);
                OWLOntology t = parse(children(q), null);
                man.addAxioms(o, t.getAxioms());
                man.removeOntology(t);
                reasoners.remove(kb);
                return "<OK/>";
            }
            case "IsKBSatisfiable": return bool(reasoner(kb).isConsistent());
            case "IsClassSatisfiable": return bool(reasoner(kb).isSatisfiable(ce(children(q).get(0))));
            case "GetFlattenedTypes": {
                OWLNamedIndividual i = df.getOWLNamedIndividual(IRI.create(children(q).get(0).getAttribute("IRI")));
                return classes(reasoner(kb).getTypes(i, direct(q)).getFlattened());
            }
            case "GetFlattenedInstances": {
                StringBuilder s = new StringBuilder("<SetOfIndividuals>");
                for (OWLNamedIndividual i : reasoner(kb).getInstances(ce(children(q).get(0)), direct(q)).getFlattened())
                    s.append("<owl:NamedIndividual IRI=\"").append(esc(i.getIRI().toString())).append("\"/>");
                return s.append("</SetOfIndividuals>").toString();
            }
            case "GetSuperClasses": return synsets(reasoner(kb).getSuperClasses(ce(children(q).get(0)), direct(q)));
            case "GetSubClasses": return synsets(reasoner(kb).getSubClasses(ce(children(q).get(0)), direct(q)));
            default: return "<Error error=\"unsupported request " + esc(q.getLocalName()) + "\"/>";
        }
    }

    static boolean direct(Element q) { return "true".equals(q.getAttribute("direct")); }
    static void release(String kb) { OWLOntology o = kbs.remove(kb); if (o != null) man.removeOntology(o); OWLReasoner r = reasoners.remove(kb); if (r != null) r.dispose(); }
    static OWLOntology kb(String kb) { OWLOntology o = kbs.get(kb); if (o == null) throw new IllegalArgumentException("unknown KB " + kb); return o; }
    static OWLReasoner reasoner(String kb) {
        return reasoners.computeIfAbsent(kb, k -> new Reasoner.ReasonerFactory().createReasoner(kb(k)));
    }
    static String bool(boolean b) { return "<BooleanResponse result=\"" + b + "\"/>"; }
    static String classes(Set<OWLClass> cs) {
        StringBuilder s = new StringBuilder("<SetOfClasses>");
        for (OWLClass c : cs) s.append("<owl:Class IRI=\"").append(esc(c.getIRI().toString())).append("\"/>");
        return s.append("</SetOfClasses>").toString();
    }
    static String synsets(NodeSet<OWLClass> ns) {
        StringBuilder s = new StringBuilder("<ClassSynsets>");
        for (org.semanticweb.owlapi.reasoner.Node<OWLClass> n : ns) {
            s.append("<ClassSynset>");
            for (OWLClass c : n) s.append("<owl:Class IRI=\"").append(esc(c.getIRI().toString())).append("\"/>");
            s.append("</ClassSynset>");
        }
        return s.append("</ClassSynsets>").toString();
    }

    /** Parse OWL/XML elements by wrapping them in an ontology document. */
    static OWLOntology parse(List<Element> els, String extra) throws Exception {
        StringBuilder d = new StringBuilder("<?xml version=\"1.0\"?><Ontology xmlns=\"" + OWL + "\" ontologyIRI=\"urn:tmp:" + (tmp++) + "\">");
        for (Element e : els) d.append(xml(e));
        if (extra != null) d.append(extra);
        d.append("</Ontology>");
        return man.loadOntologyFromOntologyDocument(new StringDocumentSource(d.toString()));
    }
    /** A class expression element -> OWLClassExpression (via EquivalentClasses(urn:q, CE)). */
    static OWLClassExpression ce(Element e) throws Exception {
        if (e.getLocalName().equals("Class")) return df.getOWLClass(IRI.create(e.getAttribute("IRI")));
        String q = "urn:query#Q" + (tmp++);
        OWLOntology t = parse(Collections.emptyList(), "<EquivalentClasses><Class IRI=\"" + q + "\"/>" + xml(e) + "</EquivalentClasses>");
        OWLClassExpression res = null;
        for (OWLEquivalentClassesAxiom ax : t.getAxioms(AxiomType.EQUIVALENT_CLASSES))
            for (OWLClassExpression c : ax.getClassExpressions()) if (c.isAnonymous() || !c.asOWLClass().getIRI().toString().equals(q)) res = c;
        man.removeOntology(t);
        return res;
    }
    static List<Element> children(Element e) {
        List<Element> l = new ArrayList<>();
        for (org.w3c.dom.Node n = e.getFirstChild(); n != null; n = n.getNextSibling()) if (n instanceof Element) l.add((Element) n);
        return l;
    }
    static String xml(Element e) throws Exception {
        Transformer t = TransformerFactory.newInstance().newTransformer();
        t.setOutputProperty(OutputKeys.OMIT_XML_DECLARATION, "yes");
        StringWriter w = new StringWriter();
        t.transform(new DOMSource(e), new StreamResult(w));
        return w.toString();
    }
    static String esc(String s) { return s.replace("&", "&amp;").replace("<", "&lt;").replace("\"", "&quot;"); }
}
