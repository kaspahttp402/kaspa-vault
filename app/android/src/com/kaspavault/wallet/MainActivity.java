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
            "welcomed", "screenshots", "currency", "alerts_seen", "balance_money",
            "only_trusted", "watch_only", "clip_check", "panic_pin", "alert_second", "watch_phone", "send_grace", "colors",
            "live_network", "live_local", "guard_nodes", "signer_window", "lock_after", "last_balances", "toured", "alert_ack_at", "rules_move",
            "chart_ma20", "chart_ma50", "price_saved", "helper_names",
            "paper_shown")); // paper_shown: a reminder flag only (the page compares the code last shown)
    /**
     * The only screens Preferences > Allow screenshots may open to capture: no words, no keys, no
     * money action. Any other name, unknown or added later, keeps FLAG_SECURE (fails closed).
     */
    private static final Set<String> SHOT_OK = new HashSet<>(Arrays.asList(
            "welcome", "home", "price", "coins", "statements", "alerts", "people", "persondetail",
            "watched", "protection", "heirclock", "linkcode", "watchcode", "paperprint", "glossary", "howitworks",
            "settings", "prefs", "about"));
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
        vaultThread.execute(signer::ensureCanary); // the fingerprint check exists before any finger prompt (none needed)
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

        // Remote debugging of the page only in a debuggable build (KV_DEBUGGABLE=1: test phones and emulators, never a money phone).
        if ((getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0) WebView.setWebContentsDebuggingEnabled(true);
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
            // Scripts run only on the bundled pages, and the page itself fetches nothing:
            // the engine runs in the app (Bridge.vault), the price comes via Bridge.price.
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
        // The guard the user turned on comes back when the app opens (after an update, a restart or a kill).
        if ("1".equals(prefs.getString(InsetsGuard.GUARD_WANTED, "")) && !GuardService.RUNNING) insets.guardOn(true);
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

    /**
     * The page's own engine mode (data.js isLocal, setting live_local). Page-writable, so it only
     * words a failed prompt (Signer); it never decides FLAG_SECURE.
     */
    boolean live() { return "1".equals(prefs.getString("live_local", "")); }

    /**
     * FLAG_SECURE blocks screenshots and screen recording. Preferences > Allow screenshots lifts it
     * only on a SHOT_OK screen, in Live and Sample alike: live_local is page-writable, so it may
     * not open a words or money screen. Decided here, never by the page.
     */
    void applySecure() {
        boolean allow = "on".equals(prefs.getString("screenshots", "off"))
                && SHOT_OK.contains(screen);
        if (allow) getWindow().clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
        else getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
    }


    private static boolean isAppPage(String url) { return url != null && url.startsWith("file:///android_asset/"); }

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

    /** After onCreate: an old install's laptop-pairing secrets go (Signer.dropLaptopSecrets). */
    @Override protected void onPostCreate(Bundle state) {
        super.onPostCreate(state);
        signer.dropLaptopSecrets();
    }

    // The page locks the moment it is put away (KV_pause / KV_resume): a paused WebView may not fire visibilitychange.
    @Override protected void onPause() { super.onPause(); web.evaluateJavascript("window.KV_pause&&KV_pause()", null); web.onPause(); }
    @Override protected void onResume() { super.onResume(); web.onResume(); web.evaluateJavascript("window.KV_resume&&KV_resume()", null); }
    /** Put away (home, other app, screen off): a picked-up phone needs a new prompt. */
    @Override protected void onStop() { signer.shutSigner(); super.onStop(); }
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
