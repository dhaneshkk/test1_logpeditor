// Drop-in replacement for attempto-ape.jar's APEWebservice using only the JDK (no Apache HttpClient).
// Same request parameters as the original (text, ulextext, uri, guess, noclex, solo | c<type>=on).
package ch.uzh.ifi.attempto.ape;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;

public class APEWebservice extends ACEParser {
    private String wsUrl;
    public APEWebservice(String url) { setWebserviceUrl(url); }
    public void setWebserviceUrl(String url) {
        if (url == null) throw new NullPointerException("APE webservice URL cannot be null");
        this.wsUrl = url;
    }
    private StringBuilder params(String text, Lexicon lexicon) throws UnsupportedEncodingException {
        StringBuilder b = new StringBuilder();
        add(b, "text", text);
        if (lexicon != null) add(b, "ulextext", lexicon.toString());
        if (getURI() != null) add(b, "uri", getURI());
        add(b, "guess", isGuessingEnabled() ? "on" : "off");
        add(b, "noclex", isClexEnabled() ? "off" : "on");
        return b;
    }
    private static void add(StringBuilder b, String k, String v) throws UnsupportedEncodingException {
        if (b.length() > 0) b.append('&');
        b.append(URLEncoder.encode(k, "UTF-8")).append('=').append(URLEncoder.encode(v, "UTF-8"));
    }
    private String post(StringBuilder body) {
        try {
            HttpURLConnection c = (HttpURLConnection) new URL(wsUrl).openConnection(Proxy.NO_PROXY);
            c.setRequestMethod("POST");
            c.setDoOutput(true);
            c.setRequestProperty("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8");
            try (OutputStream os = c.getOutputStream()) { os.write(body.toString().getBytes(StandardCharsets.UTF_8)); }
            InputStream in0 = c.getResponseCode() >= 400 ? c.getErrorStream() : c.getInputStream();
            try (InputStream is = in0) {
                ByteArrayOutputStream bo = new ByteArrayOutputStream();
                byte[] buf = new byte[8192]; int n;
                while ((n = is.read(buf)) > 0) bo.write(buf, 0, n);
                return new String(bo.toByteArray(), StandardCharsets.UTF_8);
            }
        } catch (IOException e) {
            throw new RuntimeException("Accessing APE webservice failed: " + e, e);
        }
    }
    public String getSoloOutput(String text, Lexicon lexicon, OutputType outputType) throws ACEParserException {
        try {
            StringBuilder b = params(text, lexicon);
            add(b, "solo", outputType.toSoloFlag());
            return checkForErrors(post(b));
        } catch (UnsupportedEncodingException e) { throw new RuntimeException(e); }
    }
    public ACEParserResult getMultiOutput(String text, Lexicon lexicon, OutputType... outputTypes) {
        try {
            StringBuilder b = params(text, lexicon);
            for (OutputType t : outputTypes) add(b, t.toMultiFlag(), "on");
            String r = post(b);
            if (System.getenv("APE_SHIM_DEBUG") != null && !r.startsWith("<?xml")) System.err.println("APE-SHIM non-XML for " + text + " :: " + r);
            return new ACEParserResult(r);
        } catch (UnsupportedEncodingException e) { throw new RuntimeException(e); }
    }
}
