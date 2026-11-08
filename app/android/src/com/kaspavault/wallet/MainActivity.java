package com.kaspavault.wallet;

import android.content.Intent;
import android.net.Uri;
import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.Insets;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import android.view.HapticFeedbackConstants;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.webkit.ConsoleMessage;
import android.webkit.JavascriptInterface;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.window.OnBackInvokedDispatcher;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509TrustManager;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import java.security.SecureRandom;

/** One edge-to-edge WebView that loads the bundled pages from assets/web. */
public class MainActivity extends Activity {
    /** The only preference keys the page may read or write. */
    static final Set<String> SETTINGS = new HashSet<>(Arrays.asList(
            "live_base", "live_on", "welcomed", "screenshots", "currency", "alert_push", "alert_email", "balance_money",
            "only_trusted", "watch_only", "clip_check", "panic_pin", "alert_second", "watch_phone", "send_grace", "colors",
            "live_network", "live_local", "guard_nodes", "auto_heir", "guard_on", "signer_window", "lock_after", "last_balances", "toured", "alert_ack_at", "rules_move",
            "chart_ma20", "chart_ma50", "chart_fib", "price_saved"));
    /** Screens that show words or move money: in Live mode they are always FLAG_SECURE, whatever Preferences says. */
    private static final Set<String> SENSITIVE = new HashSet<>(Arrays.asList(
            "words", "wordcheck", "restore", "movein", "send", "waiting", "sent", "pending", "stop", "stopdone", "safe", "takeout",
            "receive", "watcher", "guard", "keys", "makekeys", "sheet", "sheetcheck", "keysdrawer",
            "showsheets", "burnsheets", "takeoutwords", "pursepay", "topup", "fundacct", "keysmade"));
    /** Page console lines are cut to this many characters before they reach the device log. Dial. */
    private static final int LOG_MAX_CHARS = 200;

    WebView web;
    SharedPreferences prefs;
    final Signer signer = new Signer(this);
    final InsetsGuard insets = new InsetsGuard(this);
    final Bridge bridge = new Bridge(this);
    final Handler main = new Handler(Looper.getMainLooper());
    /** The one thread the Rust engine is called on: every VaultLib call blocks on a tokio runtime, never the UI thread. */
    final ExecutorService vaultThread = Executors.newSingleThreadExecutor();

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().setDecorFitsSystemWindows(false);
        prefs = getSharedPreferences("kaspavault", Context.MODE_PRIVATE);
        try { /* secrets wrapped by an older key (v1 fingerprint-only, v2 open for 30 s after any phone unlock) cannot be opened by the per-use key: forget them once, the user pairs again */
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore"); ks.load(null);
            if (ks.containsAlias(Signer.KEY_ALIAS_V1) || ks.containsAlias(Signer.KEY_ALIAS_V2)) { signer.forgetSecrets(); notice = Signer.REPAIR_NOTICE; }
        } catch (Exception ignored) { }
        applySecure();
        if (VaultLib.AVAILABLE) { // the engine's state lives under files/vault/<network>; a failed init is logged, never a crash
            String network = prefs.getString("live_network", "");
            if (network.isEmpty()) network = "mainnet"; // live by default; "testnet" only when the setting says so
            final String net = network, dir = getFilesDir() + "/vault/" + network;
            vaultThread.execute(() -> {
                try {
                    String initAnswer = VaultLib.init(dir, net);
                    android.util.Log.i("KaspaVault", "engine init (" + net + ") -> " + (initAnswer.contains("\"status\":200") ? "200" : "not 200") + ", " + initAnswer.length() + " chars");
                    // Self-test: one keyless status call; logcat gets its status code and size only (no balances, no addresses).
                    if (initAnswer.contains("\"status\":200")) { // the envelope lists body before status
                        String st = VaultLib.call("status", "{}");
                        android.util.Log.i("KaspaVault", "engine status -> " + (st.contains("\"status\":200") ? "200" : (st.contains("\"status\":503") ? "503" : "not 200")) + ", " + st.length() + " chars");
                    }
                }
                catch (Throwable t) { android.util.Log.w("KaspaVault", "engine init failed: " + t); }
            });
        }

        web = new WebView(this);
        web.setBackgroundColor(Color.parseColor("#06070c"));
        web.setOverScrollMode(View.OVER_SCROLL_NEVER);
        web.setVerticalScrollBarEnabled(false);
        web.setHorizontalScrollBarEnabled(false);
        web.setFilterTouchesWhenObscured(true); // taps through an overlay app are dropped
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setTextZoom(100);
        s.setAllowFileAccess(false);
        s.setAllowContentAccess(false);
        s.setAllowFileAccessFromFileURLs(false);
        s.setAllowUniversalAccessFromFileURLs(false);
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        s.setGeolocationEnabled(false);
        s.setSaveFormData(false);
        s.setSupportZoom(false);
        web.addJavascriptInterface(bridge, "KaspaVaultBridge");
        web.setWebViewClient(new WebViewClient() {
            /** The page's renderer died (out of memory, most likely): start the activity over instead of the app dying with it. */
            @Override public boolean onRenderProcessGone(WebView view, android.webkit.RenderProcessGoneDetail d) { main.post(() -> recreate()); return true; }
            // The wallet never leaves its own bundled pages.
            @Override public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest r) {
                return !isAppPage(r.getUrl().toString());
            }
            // Scripts run only on the bundled pages, and the page itself fetches nothing else: the laptop is reached through Bridge.http.
            @Override public void onPageStarted(WebView v, String url, Bitmap favicon) {
                v.getSettings().setJavaScriptEnabled(isAppPage(url));
            }
            @Override public WebResourceResponse shouldInterceptRequest(WebView v, WebResourceRequest r) {
                if (isAppPage(r.getUrl().toString())) return null;
                return new WebResourceResponse("text/plain", "UTF-8", 403, "Blocked", null, new ByteArrayInputStream(new byte[0]));
            }
            @Override public void onPageFinished(WebView v, String url) { insets.pushInsets(); openPending(); pushNotice(); }
        });
        // The scene's own log lines and page errors go to logcat so a phone run can be read on the laptop. Nothing else is
        // forwarded, and the page never logs money or words.
        web.setWebChromeClient(new WebChromeClient() {
            /** The camera for the code scanner: only our own bundled page, only video. Everything else is refused. */
            @Override public void onPermissionRequest(android.webkit.PermissionRequest r) {
                boolean ours = r.getOrigin() != null && "file".equals(r.getOrigin().getScheme());
                boolean video = java.util.Arrays.asList(r.getResources()).contains(android.webkit.PermissionRequest.RESOURCE_VIDEO_CAPTURE);
                if (ours && video) r.grant(new String[]{android.webkit.PermissionRequest.RESOURCE_VIDEO_CAPTURE}); else r.deny();
            }
            @Override public boolean onConsoleMessage(ConsoleMessage m) {
                String t = m.message();
                if (t != null && (t.startsWith("KVScene") || m.messageLevel() == ConsoleMessage.MessageLevel.ERROR)) {
                    String cut = t.length() > LOG_MAX_CHARS ? t.substring(0, LOG_MAX_CHARS) + "..." : t; // never the whole line: it may quote a token or an address
                    android.util.Log.i("KaspaVault", cut + " (" + m.sourceId() + ":" + m.lineNumber() + ")");
                }
                return true;
            }
        });
        setContentView(insets.frame());
        web.loadUrl("file:///android_asset/web/index.html");
        pendingOpen = shortcutScreen(getIntent());

        getOnBackInvokedDispatcher().registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT,
                () -> web.evaluateJavascript("window.KV_back?KV_back():false", r -> { if (!"true".equals(r)) finish(); }));
    }

    String screen = "words"; // the page's current screen; unknown counts as sensitive

    /**
     * Home-screen shortcuts (res/xml/shortcuts.xml) open one screen. Only these three names reach the page, as a literal,
     * and only when the phone's own home screen sent the intent: another app cannot steer the user onto Send or Stop.
     */
    private String pendingOpen = null;
    private String shortcutScreen(Intent i) {
        Uri u = i == null ? null : i.getData();
        if (u == null || !"kaspavault".equals(u.getScheme())) return null;
        if (!fromLauncher(i)) return null;
        String h = u.getHost();
        return "send".equals(h) || "receive".equals(h) || "stop".equals(h) ? h : null;
    }
    /**
     * True when the current home app started this intent. The system fills getReferrer() with the starting package
     * (a shortcut tap is started on the launcher's behalf); a caller who sets the referrer extras itself is not believed.
     */
    private boolean fromLauncher(Intent i) {
        try {
            if (i.hasExtra(Intent.EXTRA_REFERRER) || i.hasExtra(Intent.EXTRA_REFERRER_NAME)) return false;
            Uri ref = getReferrer();
            String from = ref != null && "android-app".equals(ref.getScheme()) ? ref.getHost() : null;
            if (from == null) return false;
            android.content.pm.ResolveInfo home = getPackageManager().resolveActivity(
                    new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME), android.content.pm.PackageManager.MATCH_DEFAULT_ONLY);
            return home != null && home.activityInfo != null && from.equals(home.activityInfo.packageName);
        } catch (Exception e) {
            return false;
        }
    }
    private void openPending() {
        if (pendingOpen == null) return;
        web.evaluateJavascript("window.KV_open&&KV_open('" + pendingOpen + "')", null);
        pendingOpen = null;
    }
    @Override protected void onNewIntent(Intent i) {
        super.onNewIntent(i); setIntent(i);
        pendingOpen = shortcutScreen(i); openPending();
    }

    boolean live() { return "1".equals(prefs.getString("live_on", "")); }

    /**
     * FLAG_SECURE blocks screenshots and screen recording. Preferences > Allow screenshots can lift it only in Sample
     * mode, or in Live mode on a screen with no words and no money action. Decided here, never by the page.
     */
    void applySecure() {
        boolean allow = "on".equals(prefs.getString("screenshots", "off")) && !(live() && SENSITIVE.contains(screen));
        if (allow) getWindow().clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
        else getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
    }


    private static boolean isAppPage(String url) { return url != null && url.startsWith("file:///android_asset/"); }

    /** https only, only to a home-network IPv4 address, and once a laptop address is saved only to that host. */
    boolean allowedUrl(String url) {
        try {
            URL u = new URL(url);
            if (!"https".equals(u.getProtocol())) return false;
            if (!privateHost(u.getHost())) return false;
            String paired = hostOf(prefs.getString("live_base", ""));
            return paired.isEmpty() || paired.equals(u.getHost());
        } catch (Exception e) {
            return false;
        }
    }

    /** A dotted-quad address in a private range (RFC 1918: 10/8, 172.16/12 or the class-C /16). No names, no public hosts, no 127/8. */
    static boolean privateHost(String host) {
        if (host == null) return false;
        String[] p = host.split("\\.");
        if (p.length != 4) return false;
        int[] q = new int[4];
        for (int i = 0; i < 4; i++) {
            if (!p[i].matches("\\d{1,3}")) return false;
            q[i] = Integer.parseInt(p[i]);
            if (q[i] > 255) return false;
        }
        return q[0] == 10 || (q[0] == 172 && q[1] >= 16 && q[1] <= 31) || (q[0] == 192 && q[1] == 168);
    }

    /** Host part of a saved laptop address, typed with or without a scheme; "" when there is none. */
    static String hostOf(String base) {
        try {
            String b = base == null ? "" : base.trim();
            if (b.isEmpty()) return "";
            if (!b.matches("(?i)^https://.*")) b = "https://" + b;
            String h = new URL(b).getHost();
            return h == null ? "" : h;
        } catch (Exception e) {
            return "";
        }
    }

    /** A one-time message for the page (window.KV_notice), sent once the page is up. */
    String notice = null;
    void pushNotice() {
        String n = notice;
        if (n == null) return;
        notice = null;
        android.util.Log.i("KaspaVault", n);
        call("KV_notice", JSONObject.quote(n));
    }

    // ---- TLS: the laptop's self-signed certificate, pinned by its SHA-256 ----

    /** Lower-case hex SHA-256 of the DER certificate: the pin, and what the three check words are made from. */
    private static String sha256Hex(X509Certificate c) throws CertificateException {
        try {
            byte[] d = MessageDigest.getInstance("SHA-256").digest(c.getEncoded());
            StringBuilder b = new StringBuilder(64);
            for (byte x : d) b.append(String.format("%02x", x));
            return b.toString();
        } catch (Exception e) {
            throw new CertificateException(e);
        }
    }

    /** host:port of a URL (default 443), the unit a pin belongs to. */
    static String hostPort(String url) {
        try {
            URL u = new URL(url);
            return u.getHost() + ":" + (u.getPort() < 0 ? 443 : u.getPort());
        } catch (Exception e) {
            return "";
        }
    }

    /**
     * The certificate hash seen by the last peek(), usable by http() for that host:port only, until the page stores the
     * pin (the user said the words match) or clears it (the user said no). Pairing needs it: /pair and the first status
     * are fetched before the pin is saved.
     */
    String pendingPin = "", pendingHost = "";
    /** The stored pin, read at the in-app prompt that opens the secrets and kept for this run. */
    String livePin = "";

    /** No CA store: the server certificate is trusted when, and only when, its SHA-256 equals the pin. */
    static SSLContext pinned(String pinHex, boolean[] mismatch) throws Exception {
        TrustManager tm = new X509TrustManager() {
            public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
                if (chain == null || chain.length == 0) throw new CertificateException("no certificate");
                byte[] seen = sha256Hex(chain[0]).getBytes(StandardCharsets.US_ASCII);
                byte[] want = pinHex.getBytes(StandardCharsets.US_ASCII);
                if (pinHex.length() != 64 || !MessageDigest.isEqual(seen, want)) { mismatch[0] = true; throw new CertificateException("pin"); }
            }
            public void checkClientTrusted(X509Certificate[] chain, String authType) throws CertificateException { throw new CertificateException(); }
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
        };
        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(null, new TrustManager[]{tm}, null);
        return ctx;
    }

    /** Trusts nothing: only records the certificate the laptop presents, for the first look at pairing. */
    static SSLContext peeking(String[] seen) throws Exception {
        TrustManager tm = new X509TrustManager() {
            public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
                if (chain == null || chain.length == 0) throw new CertificateException("no certificate");
                seen[0] = sha256Hex(chain[0]);
            }
            public void checkClientTrusted(X509Certificate[] chain, String authType) throws CertificateException { throw new CertificateException(); }
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
        };
        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(null, new TrustManager[]{tm}, null);
        return ctx;
    }



    // The page locks the moment it is put away (KV_pause / KV_resume): a paused WebView may not fire visibilitychange.
    @Override protected void onPause() { super.onPause(); web.evaluateJavascript("window.KV_pause&&KV_pause()", null); web.onPause(); }
    @Override protected void onResume() { super.onResume(); web.onResume(); web.evaluateJavascript("window.KV_resume&&KV_resume()", null); }
    @Override protected void onDestroy() { web.destroy(); super.onDestroy(); }
    @Override public void onRequestPermissionsResult(int code, String[] perms, int[] res) {
        super.onRequestPermissionsResult(code, perms, res);
        if (code == 7) call("KV_camera", res.length > 0 && res[0] == android.content.pm.PackageManager.PERMISSION_GRANTED ? "true" : "false");
        if (code == 8) call("KV_notifications", res.length > 0 && res[0] == android.content.pm.PackageManager.PERMISSION_GRANTED ? "true" : "false");
    }

    /** Calls window[name](args) on the page. The name is checked so a page value can never become code. */
    void call(String name, String... jsonArgs) {
        if (name == null || !name.matches("[A-Za-z_$][A-Za-z0-9_$]*")) return;
        String js = "window." + name + "&&" + name + "(" + String.join(",", jsonArgs) + ")";
        main.post(() -> web.evaluateJavascript(js, null));
    }


}
