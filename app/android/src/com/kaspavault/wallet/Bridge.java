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


/** Every @JavascriptInterface method the page calls, moved out of MainActivity; reaches the activity through act. */
final class Bridge {
    private final MainActivity act;
    Bridge(MainActivity a) { act = a; }

    static final int MAX_BODY = 256 * 1024;
    /**
     * Status values the page gets besides real HTTP codes: not reached; the laptop's certificate is not the pinned one;
     * the phone's store is still shut (no fingerprint in this run yet), so nothing was sent and the page should unlock first.
     */
    static final int NOT_REACHED = 0, PIN_MISMATCH = -2, LOCKED = -3;
    /** Status the page gets from Bridge.vault when this build has no wallet engine (no libvault_core.so): "not ready". */
    static final int ENGINE_MISSING = -4;
    /** Body sent with a 500 when the engine's envelope cannot be read. Plain words, the page shows it as is. */
    static final String ENGINE_UNREADABLE = "{\"ok\":false,\"error\":\"The wallet engine answered something unreadable.\"}";
    /** Body (200) when a money method needs a key this phone has not made yet. Bank words, the page shows it as is. */
    static final String NO_CARD_KEY = "{\"ok\":false,\"error\":\"This phone holds no card key yet. Make your keys first.\"}";
    static final String NO_STOP_KEY = "{\"ok\":false,\"error\":\"This phone holds no emergency stop key yet. Make your keys first.\"}";
    static final String NO_FUNDING_KEY = "{\"ok\":false,\"error\":\"This phone holds no funding key yet. Make your keys first.\"}";
    /** Body (200) when the engine made no keys (it refused, or answered something that is not 96 bytes). */
    static final String KEYS_NOT_MADE = "{\"ok\":false,\"error\":\"The keys could not be made on this phone.\"}";
    /** Body (200) when the engine made keys but this phone could not seal them: the account must be stopped and the keys made again. */
    static final String KEYS_NOT_KEPT = "{\"ok\":false,\"error\":\"The keys were made but this phone could not keep them. Stop this account and make the keys again.\"}";
    /**
     * Money methods and the key each one carries into the engine (VaultLib.callWithSecret): send -> the card key;
     * stop, decline -> the emergency stop key followed by the funding key (64 bytes); fund, purse.pay -> the funding key;
     * topup -> the card key followed by the funding key (64 bytes). takeout, keys.sheets and keys.burn are handled by name
     * in vault() (the safe's sheets). Every other method (status, quote, finish, keys.public, keys.reset, guard.*, log.tail,
     * ...) is keyless and goes through VaultLib.call.
     */
    static final Set<String> CARD_METHODS = new HashSet<>(Arrays.asList("send"));
    static final Set<String> STOP_METHODS = new HashSet<>(Arrays.asList("stop", "decline"));
    static final Set<String> FUNDING_METHODS = new HashSet<>(Arrays.asList("fund", "purse.pay"));
    static final Set<String> TOPUP_METHODS = new HashSet<>(Arrays.asList("topup"));
    /** Every engine method the page may call; anything else answers 404 without reaching the engine. */
    static final Set<String> ENGINE_METHODS = new HashSet<>(Arrays.asList(
            "status", "quote", "send", "finish", "stop", "decline", "funding", "fund", "topup", "takeout", "purse.pay",
            "keys.reset", "keys.public", "keys.sheets", "keys.burn", "rules.change", "heir.set",
            "guard.start", "guard.stop", "guard.status", "guard.wait", "guard.poll", "guard.export", "guard.import"));
    /** The moves that cannot be undone: a fingerprint within Signer.FRESH_PROMPT_MS, whatever the window setting says. */
    static final Set<String> FRESH_PROMPT_METHODS = new HashSet<>(Arrays.asList("takeout", "keys.sheets", "keys.burn", "keys.reset", "heir.set"));
    static final String NOT_A_METHOD = "{\"ok\":false,\"error\":\"That is not something this wallet does.\"}";
    static final String FRESH_FINGER = "{\"ok\":false,\"error\":\"This one needs your fingerprint again. Try once more.\"}";
    /** Body (200) for keys.sheets, or a takeout without typed sheets, once keys.burn wiped the sealed safe keys (the engine's own words). */
    static final String SHEETS_BURNED = "{\"ok\":false,\"error\":\"The sheets were burned from this phone. Only paper opens the vault now.\"}";
    /** Body (200) when the sealed safe keys are there but do not open (a torn store). */
    static final String NO_SAFE_KEYS = "{\"ok\":false,\"error\":\"The safe keys kept on this phone could not be read. Type the sheets from paper.\"}";
    static final String BURN_UNCONFIRMED = "{\"ok\":false,\"error\":\"Burning the sheets needs a confirmation.\"}";
    static final String TWO_SHEETS = "{\"ok\":false,\"error\":\"Two sheets of 24 words are needed.\"}";
    /** The copied address is wiped from the clipboard after this many seconds. Dial. */
    static final int CLIP_SECONDS = 60;
    /** A KAS price the chart may show must be above zero and under this many dollars; anything else is dropped. Dial. */
    static final double PRICE_MAX = 1000000;
    /** The most price points accepted from the public price host. Dial. */
    static final int PRICE_MAX_POINTS = 4000;

        /** Whether the wallet engine (libvault_core.so) loaded; the page then talks to the account on this phone, no laptop. */
        @JavascriptInterface public boolean engine() { return VaultLib.AVAILABLE; }
        @JavascriptInterface public String getSetting(String key) {
            return MainActivity.SETTINGS.contains(key) ? act.prefs.getString(key, "") : "";
        }

        /** Allow-listed keys only; a laptop address must be a home-network IPv4 address; screen flags follow. */
        @JavascriptInterface public void setSetting(String key, String value) {
            if (!MainActivity.SETTINGS.contains(key)) return;
            String v = value == null ? "" : value;
            if ("live_base".equals(key) && !v.isEmpty() && !MainActivity.privateHost(MainActivity.hostOf(v))) return;
            act.prefs.edit().putString(key, v).apply();
            if ("screenshots".equals(key) || "live_on".equals(key)) act.main.post(act::applySecure);
            if ("signer_window".equals(key)) act.signer.shutSigner(); // a new window starts at the next fingerprint
        }

        /** The page reports which screen it shows; the flag decision stays here. Unknown names count as sensitive. */
        @JavascriptInterface public void screen(String name) {
            act.screen = name != null && name.matches("[a-z]{1,24}") ? name : "words";
            act.main.post(act::applySecure);
        }

        /**
         * Stores a secret encrypted (AES-GCM under the data key an in-app prompt opened within UNLOCK_SECONDS). An empty
         * value removes it. True when stored; false when the store is shut (the page should ask for a fingerprint and try
         * again). A failed write never drops what was stored before.
         */
        @JavascriptInterface public synchronized boolean setSecret(String key, String value) {
            return act.signer.setSecret(key, value);
        }

        /**
         * "" when there is no such secret, no in-app prompt has opened the store in this run (or it shut again after
         * UNLOCK_SECONDS), or this secret was already read since the last prompt: one read per secret per prompt.
         */
        @JavascriptInterface public synchronized String getSecret(String key) {
            return act.signer.getSecret(key);
        }

        /** Puts text on the clipboard (the Receive address), marked sensitive so previews hide it, and wipes it after CLIP_SECONDS if it is still ours. */
        @JavascriptInterface public void copy(String text) {
            act.main.post(() -> {
                android.content.ClipboardManager cm = act.getSystemService(android.content.ClipboardManager.class);
                if (cm == null || text == null) return;
                android.content.ClipData clip = android.content.ClipData.newPlainText("address", text);
                android.os.PersistableBundle extras = new android.os.PersistableBundle();
                extras.putBoolean(android.content.ClipDescription.EXTRA_IS_SENSITIVE, true);
                clip.getDescription().setExtras(extras);
                cm.setPrimaryClip(clip);
                act.main.postDelayed(() -> {
                    try { // only our own copy is wiped, never something the user copied since
                        android.content.ClipData now = cm.getPrimaryClip();
                        if (now != null && now.getItemCount() > 0 && text.contentEquals(now.getItemAt(0).coerceToText(act))) cm.clearPrimaryClip();
                    } catch (Exception ignored) { }
                }, CLIP_SECONDS * 1000L);
            });
        }

        /** The camera for the code scanner on Send: asks once, answers window.KV_camera(true|false). */
        @JavascriptInterface public void askCamera() {
            act.main.post(() -> {
                if (act.checkSelfPermission(android.Manifest.permission.CAMERA) == android.content.pm.PackageManager.PERMISSION_GRANTED) act.call("KV_camera", "true");
                else act.requestPermissions(new String[]{android.Manifest.permission.CAMERA}, 7);
            });
        }

        /**
         * The guard inside the app (D5): on starts GuardService in the foreground, off stops it. The first time on it asks
         * for notifications (API 33+ runtime permission, answered to window.KV_notifications) and for the battery exemption
         * (so the guard keeps hearing the network through the night); both asks happen once, the phone remembers the answer.
         */
        @JavascriptInterface public void guardOn(boolean on) {
            act.insets.guardOn(on);
        }

        /** Whether the guard service is running right now (the engine accepted guard.start). */
        @JavascriptInterface public boolean guardRunning() { return act.insets.guardRunning(); }

        @JavascriptInterface public void haptic() {
            act.main.post(() -> act.web.performHapticFeedback(HapticFeedbackConstants.CONFIRM));
        }

        @JavascriptInterface public void confirmWithBiometric(String callbackName) {
            confirmWithBiometric(callbackName, "Confirm it is you");
        }

        /**
         * Answers the page with "ok" (a fingerprint or the phone's PIN), "cancel" (the user left), "away" (the phone took
         * the prompt down, usually because the app was put away: the page locks), "refused" (lockout, nothing
         * enrolled, sensor error: in Live mode nothing but "ok" counts) or "unavailable" (Sample mode only, no sensor:
         * the page may show a plain confirm there, since no money or words are at stake).
         */
        @JavascriptInterface public void confirmWithBiometric(String callbackName, String subtitle) {
            act.signer.confirmWithBiometric(callbackName, subtitle);
        }

        @JavascriptInterface public void confirmSigner(String callbackName) {
            confirmSigner(callbackName, "Confirm it is you");
        }

        /**
         * The signer store's own prompt: confirmWithBiometric with signerKey()/signerCipher() in place of the
         * pairing key, opening the signer data key for ONE vault() money call or ONE keysNew. Same answers: "ok",
         * "cancel", "away", "refused" (lockout, nothing enrolled, sensor error) or "unavailable" (no sensor and no PIN at
         * all). When the fingerprints changed, only the everyday keys on this phone (card, stop, funding) are cleared and
         * the page gets SIGNER_ENROL_NOTICE; the safe sheets stay (own key, Signer.SAFE_ALIAS) and the prompt goes on. Money needs the real key, so Sample mode gets no plain-confirm pass here.
         */
        @JavascriptInterface public void confirmSigner(String callbackName, String subtitle) {
            act.signer.confirmSigner(callbackName, subtitle);
        }

        /**
         * First look at a laptop for pairing: a TLS handshake only (no request, nothing sent), answering
         * window[callbackName](id, sha256hex) with the hash of the certificate it presented, or "" when it was not reached.
         * The hash is kept as the pending pin for that host:port so /pair can use it once the user has confirmed the words.
         * An empty url clears the pending pin (the user said the words do not match).
         */
        @JavascriptInterface public void peek(String callbackName, int id, String url) {
            if (url == null || url.isEmpty()) {
                synchronized (Bridge.this) { act.pendingPin = ""; act.pendingHost = ""; }
                act.call(callbackName, String.valueOf(id), JSONObject.quote(""));
                return;
            }
            new Thread(() -> {
                String[] seen = {""};
                try {
                    if (!act.allowedUrl(url)) throw new IllegalArgumentException("url");
                    URL u = new URL(url);
                    try (SSLSocket s = (SSLSocket) MainActivity.peeking(seen).getSocketFactory().createSocket()) {
                        s.connect(new InetSocketAddress(u.getHost(), u.getPort() < 0 ? 443 : u.getPort()), 8000);
                        s.setSoTimeout(8000);
                        s.startHandshake();
                    }
                } catch (Exception e) {
                    seen[0] = "";
                }
                synchronized (Bridge.this) { act.pendingPin = seen[0]; act.pendingHost = seen[0].isEmpty() ? "" : MainActivity.hostPort(url); }
                act.call(callbackName, String.valueOf(id), JSONObject.quote(seen[0]));
            }).start();
        }

        /**
         * Requests go through here so the bundled page is not held to browser cross-site rules. HTTPS only, and the laptop's
         * certificate must hash to the pin: the one being paired (peek) for that host, else the stored live_pin; no CA, no
         * hostname check (the pin is the identity). Answers window[callbackName](id, httpStatus, bodyText); status 0 means
         * the laptop was not reached, -2 that it presented a different certificate (nothing is sent then, not even the
         * token), -3 that the phone's store is still shut so no pin is known (unlock first; nothing was sent).
         *
         * The token travels only to the certificate the user has confirmed, the stored pin. Under a peeked, not yet
         * confirmed certificate the request carries no token at all: /pair needs none, and an impostor who answered the
         * peek learns nothing it could replay. The pin check itself runs in the TLS handshake (pinned()), before any byte
         * of the request leaves the phone; on a mismatch the handshake fails and nothing is sent.
         */
        @JavascriptInterface public void http(String callbackName, int id, String method, String url, String token, String body) {
            String pin;
            boolean confirmed;
            synchronized (this) {
                if (act.livePin.isEmpty()) act.livePin = act.signer.open("live_pin");
                boolean peeked = !act.pendingPin.isEmpty() && act.pendingHost.equals(MainActivity.hostPort(url));
                pin = peeked ? act.pendingPin : act.livePin;
                confirmed = !peeked && !act.livePin.isEmpty();
            }
            final String sendToken = confirmed && token != null ? token : "";
            new Thread(() -> {
                int status = NOT_REACHED;
                String text = "";
                boolean[] mismatch = {false};
                boolean locked = false;
                HttpsURLConnection c = null;
                try {
                    if (!act.allowedUrl(url)) throw new IllegalArgumentException("url");
                    if (pin.length() != 64) { locked = true; throw new IllegalStateException("no pin"); }
                    c = (HttpsURLConnection) new URL(url).openConnection();
                    c.setSSLSocketFactory(MainActivity.pinned(pin, mismatch).getSocketFactory());
                    c.setHostnameVerifier((h, sess) -> true); // the address is a LAN IP and the certificate is self-signed: the pin is the whole identity check
                    c.setInstanceFollowRedirects(false); // a 3xx is an error: the token never follows a Location header
                    c.setRequestMethod(method);
                    c.setConnectTimeout(8000);
                    c.setReadTimeout("GET".equals(method) ? 10000 : 150000);
                    c.setRequestProperty("X-Token", sendToken);
                    if (!"GET".equals(method)) {
                        c.setDoOutput(true);
                        c.setRequestProperty("Content-Type", "application/json");
                        try (OutputStream o = c.getOutputStream()) {
                            o.write((body == null ? "{}" : body).getBytes(StandardCharsets.UTF_8));
                        }
                    }
                    status = c.getResponseCode();
                    if (status >= 300 && status < 400) throw new IllegalStateException("redirect");
                    InputStream in = status >= 400 ? c.getErrorStream() : c.getInputStream();
                    if (in != null) {
                        ByteArrayOutputStream b = new ByteArrayOutputStream();
                        byte[] buf = new byte[8192];
                        for (int n; (n = in.read(buf)) > 0; ) {
                            if (b.size() + n > MAX_BODY) throw new IllegalStateException("body"); // bounded: no streaming forever
                            b.write(buf, 0, n);
                        }
                        text = b.toString("UTF-8");
                    }
                } catch (Exception e) {
                    status = mismatch[0] ? PIN_MISMATCH : locked ? LOCKED : NOT_REACHED;
                    text = "";
                } finally {
                    if (c != null) c.disconnect();
                }
                act.call(callbackName, String.valueOf(id), String.valueOf(status), JSONObject.quote(text));
            }).start();
        }
        /**
         * The wallet engine in the app itself (no laptop): runs one method on vaultThread and answers through the page's
         * __kvHttp path exactly as http does: callback(id, status, text). Without the library: -4 at once. The envelope is
         * {"status":<int>,"body":<json>}; anything unreadable becomes a 500 with ENGINE_UNREADABLE.
         * The key a method carries (`Slot` in core.rs): send -> card; stop, decline -> stop + funding (64 bytes);
         * fund, purse.pay -> funding; topup -> card + funding (64 bytes); takeout -> the two sheets typed on the page
         * (args.sheets, two strings of 24 words: joined with a newline as UTF-8 and lifted out of the JSON the engine sees)
         * or, without typed sheets, this phone's own sealed safe keys cold + cold2 (64 bytes), or an empty block once they
         * were burned (the engine answers in its own words); keys.sheets -> cold + cold2 + cold3 (96 bytes), answered here
         * as burned when the prefs are gone. keys.burn {confirm:true} wipes ONLY the three sealed cold prefs, then runs the
         * engine's keyless keys.burned so its marker goes too. keys.reset is keyless; when the engine answers ok, the sealed
         * keys here are dropped (forgetSignerPrefs + forgetSafe) so they never shadow the next set. Every method that reads the store or
         * burns needs a confirmSigner prompt first (else LOCKED, "Unlock first"); each slot is read once and the store shuts
         * here, before the engine runs, so one fingerprint is one signature. Sheet words are never logged.
         */
        @JavascriptInterface public void vault(String callbackName, int id, String method, String argsJson) {
            if (!VaultLib.AVAILABLE) { act.call(callbackName, String.valueOf(id), String.valueOf(ENGINE_MISSING), JSONObject.quote("")); return; }
            String args0 = argsJson == null ? "{}" : argsJson;
            final String m = method == null ? "" : method;
            if (!ENGINE_METHODS.contains(m)) { act.call(callbackName, String.valueOf(id), "404", JSONObject.quote(NOT_A_METHOD)); return; }
            // The irreversible moves need a fingerprint given just now. Refused in words and the signer shut, so the page's
            // next confirmSigner asks for the finger even inside the window; the user tries once more and it goes through.
            if (FRESH_PROMPT_METHODS.contains(m)) {
                synchronized (act.bridge) {
                    if (!act.signer.promptedRecently()) {
                        act.signer.shutSigner();
                        act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(FRESH_FINGER));
                        return;
                    }
                }
            }
            final boolean takeout = "takeout".equals(m), sheets = "keys.sheets".equals(m), burn = "keys.burn".equals(m);
            // Typed sheets ride in args.sheets: out of the JSON, into the key block.
            byte[] typed = null;
            if (takeout) {
                try {
                    JSONObject o = new JSONObject(args0);
                    JSONArray s = o.optJSONArray("sheets");
                    if (s != null) {
                        if (s.length() != 2) { act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(TWO_SHEETS)); return; }
                        typed = (s.getString(0).trim() + "\n" + s.getString(1).trim()).getBytes(java.nio.charset.StandardCharsets.UTF_8);
                        o.remove("sheets");
                        args0 = o.toString();
                    }
                } catch (org.json.JSONException e) {
                    act.call(callbackName, String.valueOf(id), "400", JSONObject.quote(""));
                    return;
                }
            }
            final String args = args0;
            final String engineMethod = burn ? "keys.burned" : m;
            final byte[] secret;
            if (CARD_METHODS.contains(m) || STOP_METHODS.contains(m) || FUNDING_METHODS.contains(m) || TOPUP_METHODS.contains(m) || takeout || sheets || burn) {
                String missing = null;
                byte[] a = null, b = null, c = null;
                synchronized (act.bridge) {
                    // The safe methods passed the fresh-prompt gate above; they may run on a plain prompt too (no signer key,
                    // e.g. right after a new fingerprint cleared it), since the safe store has its own key.
                    if (!act.signer.signerOpen() && !((takeout || sheets || burn) && act.signer.promptedRecently())) {
                        if (typed != null) Arrays.fill(typed, (byte) 0);
                        act.call(callbackName, String.valueOf(id), String.valueOf(LOCKED), JSONObject.quote(""));
                        return;
                    }
                    try {
                        if (CARD_METHODS.contains(m)) {
                            a = act.signer.openBytes("card");
                            if (a == null) missing = NO_CARD_KEY;
                        } else if (STOP_METHODS.contains(m)) {
                            a = act.signer.openBytes("stop");
                            b = act.signer.hasSlot("funding") ? act.signer.openBytes("funding") : null; // the engine accepts the stop key alone above the floor
                            if (a == null) missing = NO_STOP_KEY;
                        } else if (TOPUP_METHODS.contains(m)) {
                            a = act.signer.openBytes("card");
                            b = act.signer.openBytes("funding");
                            if (a == null) missing = NO_CARD_KEY;
                            else if (b == null) missing = NO_FUNDING_KEY;
                        } else if (FUNDING_METHODS.contains(m)) {
                            a = act.signer.openBytes("funding");
                            if (a == null) missing = NO_FUNDING_KEY;
                        } else if (takeout) {
                            if (typed != null) {
                                a = typed;
                            } else if (act.signer.hasSlot("cold") && act.signer.hasSlot("cold2")) {
                                try {
                                    byte[][] o = act.signer.openCold("cold", "cold2");
                                    a = o[0];
                                    b = o[1];
                                    if (a == null || b == null) missing = NO_SAFE_KEYS;
                                } catch (android.security.keystore.UserNotAuthenticatedException e) {
                                    act.signer.shutSigner(); // the safe key's seconds after the prompt ran out
                                    missing = FRESH_FINGER;
                                }
                            } else {
                                a = new byte[0]; // burned (or never kept): the engine says so in its words
                            }
                        } else if (sheets) {
                            if (!act.signer.hasSlot("cold") || !act.signer.hasSlot("cold2") || !act.signer.hasSlot("cold3")) {
                                missing = SHEETS_BURNED;
                            } else {
                                try {
                                    byte[][] o = act.signer.openCold("cold", "cold2", "cold3");
                                    a = o[0];
                                    b = o[1];
                                    c = o[2];
                                    if (a == null || b == null || c == null) missing = NO_SAFE_KEYS;
                                } catch (android.security.keystore.UserNotAuthenticatedException e) {
                                    missing = FRESH_FINGER; // the safe key's seconds after the prompt ran out
                                }
                            }
                        } else {
                            boolean confirmed;
                            try { confirmed = new JSONObject(args).optBoolean("confirm", false); } catch (org.json.JSONException e) { confirmed = false; }
                            if (!confirmed) missing = BURN_UNCONFIRMED;
                            else act.signer.burnCold(); // safe.cold* and any old signer.cold*
                        }
                    } finally {
                        if (sheets || burn) act.signer.shutSigner(); else act.signer.signerUsed();
                    }
                }
                if (missing != null) {
                    Signer.joinSecrets(a, b, c); // zero-fills what was read
                    act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(missing));
                    return;
                }
                secret = burn ? null : Signer.joinSecrets(a, b, c);
            } else {
                secret = null;
            }
            act.vaultThread.execute(() -> {
                int status;
                String text;
                try {
                    String envelope;
                    try {
                        envelope = secret == null ? VaultLib.call(engineMethod, args) : VaultLib.callWithSecret(engineMethod, args, secret);
                    } finally {
                        if (secret != null) Arrays.fill(secret, (byte) 0);
                    }
                    JSONObject env = new JSONObject(envelope);
                    status = env.getInt("status");
                    text = env.isNull("body") ? "" : String.valueOf(env.get("body")); // objects and arrays re-serialise as JSON text
                } catch (Throwable t) {
                    android.util.Log.w("KaspaVault", "engine answer unreadable (" + m + "): " + t);
                    status = 500;
                    text = ENGINE_UNREADABLE;
                }
                // One line per engine call for logcat: method and outcome, never a body (a refusal's plain words only).
                android.util.Log.i("KaspaVault", "engine " + m + " -> " + status + ((status == 200 && (bodyOk(text) || errorOf(text).isEmpty())) ? " ok" : " " + errorOf(text)));
                // The old key files are archived: the sealed keys of that set go too, before the page hears it.
                if ("keys.reset".equals(m) && status == 200 && bodyOk(text)) { act.signer.forgetSignerPrefs(); act.signer.forgetSafe(); }
                act.call(callbackName, String.valueOf(id), String.valueOf(status), JSONObject.quote(text));
            });
        }

        /**
         * Makes this phone's three keys: the engine's keysNew answers 96 raw bytes, [card 32][stop 32][funding 32],
         * which are sealed as signer.card / signer.stop / signer.funding in one prefs commit and zero-filled; the engine
         * keeps only the public side (keys.json). The page gets window[callbackName](id, "200", body) with body
         * {"ok":true} when the keys are kept, or {"ok":false,"error":<plain words>} when the engine made none or this
         * phone could not seal them; -3 when no confirmSigner prompt opened the store first (wrapping is an authorised
         * operation too); -4 without the engine. The key bytes never reach the page.
         * Mode "phone" (everything made on this phone, no drawer device): the engine's answer is 192 bytes, the three safe
         * keys [cold 32][cold2 32][cold3 32] after the first 96 (sealed here under the safe key as safe.cold / safe.cold2 /
         * safe.cold3 in the same commit, so "Show my sheets" and a paperless take-out work until keys.burn, even after a
         * new fingerprint; if the safe key's seconds after the prompt ran out they go to signer.cold* and migrateCold
         * moves them at the next prompt), then UTF-8 JSON
         * {"sheets":[[24 words],[24 words],[24 words]]}, the same keys as words. The words go to the page exactly once, as
         * the body {"ok":true,"sheets":[...]}, for paper; they are never logged, and the whole array is zero-filled afterwards.
         * (An engine from before the 192-byte block, 96 bytes then JSON, is still read: no safe keys are sealed then.)
         */
        @JavascriptInterface public void keysNew(String callbackName, int id, String argsJson) {
            if (!VaultLib.AVAILABLE) { act.call(callbackName, String.valueOf(id), String.valueOf(ENGINE_MISSING), JSONObject.quote("")); return; }
            final String args = argsJson == null ? "{}" : argsJson;
            if (!act.signer.signerOpen()) { act.call(callbackName, String.valueOf(id), String.valueOf(LOCKED), JSONObject.quote("")); return; }
            act.vaultThread.execute(() -> {
                byte[] k = null;
                String text;
                try {
                    try {
                        k = VaultLib.keysNew(args);
                    } catch (Throwable t) {
                        android.util.Log.w("KaspaVault", "keysNew failed: " + t);
                        k = null;
                    }
                    if (k == null || k.length < 96) {
                        text = KEYS_NOT_MADE;
                    } else {
                        String card, stop, funding, cold = null, cold2 = null, cold3 = null;
                        boolean coldSafe = false;
                        // 96 bytes: a drawer set (or an old engine's phone set with the words right after). 192 or more: the safe keys too.
                        final int tail = (k.length > 96 && k[96] == '{') ? 96 : (k.length >= 192 ? 192 : 96);
                        byte[] c = Arrays.copyOfRange(k, 0, 32), st = Arrays.copyOfRange(k, 32, 64), f = Arrays.copyOfRange(k, 64, 96);
                        byte[] k1 = tail == 192 ? Arrays.copyOfRange(k, 96, 128) : null;
                        byte[] k2 = tail == 192 ? Arrays.copyOfRange(k, 128, 160) : null;
                        byte[] k3 = tail == 192 ? Arrays.copyOfRange(k, 160, 192) : null;
                        synchronized (act.bridge) {
                            try {
                                card = act.signer.sealBytes("card", c);
                                stop = act.signer.sealBytes("stop", st);
                                funding = act.signer.sealBytes("funding", f);
                                if (k1 != null) {
                                    String[] sc = act.signer.sealSafe(k1, k2, k3); // the safe key: survives a new fingerprint
                                    coldSafe = sc != null;
                                    if (coldSafe) {
                                        cold = sc[0];
                                        cold2 = sc[1];
                                        cold3 = sc[2];
                                    } else { // safe key's seconds ran out: the old store for now, migrateCold moves them at the next prompt
                                        cold = act.signer.sealBytes("cold", k1);
                                        cold2 = act.signer.sealBytes("cold2", k2);
                                        cold3 = act.signer.sealBytes("cold3", k3);
                                    }
                                }
                            } finally {
                                Signer.joinSecrets(c, st, f, k1, k2, k3); // zero-fills every part
                                act.signer.shutSigner();
                            }
                        }
                        boolean safeKept = k1 == null || (cold != null && cold2 != null && cold3 != null);
                        if (card != null && stop != null && funding != null && safeKept) {
                            SharedPreferences.Editor ed = act.prefs.edit().putString("signer.card", card).putString("signer.stop", stop).putString("signer.funding", funding);
                            if (k1 != null) {
                                String p = coldSafe ? "safe." : "signer.", q = coldSafe ? "signer." : "safe.";
                                ed.putString(p + "cold", cold).putString(p + "cold2", cold2).putString(p + "cold3", cold3)
                                        .remove(q + "cold").remove(q + "cold2").remove(q + "cold3"); // the other store's old set never shadows this one
                            }
                            ed.apply();
                            if (k.length > tail) {
                                // the safe sheets follow the secrets: into the answer once, nowhere else
                                String tail1 = new String(k, tail, k.length - tail, java.nio.charset.StandardCharsets.UTF_8);
                                try {
                                    JSONObject answer = new JSONObject(tail1);
                                    answer.put("ok", true);
                                    text = answer.toString();
                                } catch (org.json.JSONException e) {
                                    text = KEYS_NOT_KEPT;
                                }
                            } else {
                                text = "{\"ok\":true}";
                            }
                        } else {
                            text = KEYS_NOT_KEPT;
                        }
                    }
                } finally {
                    if (k != null) Arrays.fill(k, (byte) 0);
                    act.signer.shutSigner();
                }
                act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(text));
            });
        }
        /**
         * The KAS price for the chart: a GET to one public host, no token, nothing of ours in the request, the body bounded
         * like the laptop's. Display only: the page gets the price points alone, each checked to be a real number between
         * zero and PRICE_MAX dollars; anything else (or more than PRICE_MAX_POINTS of them) is dropped as "not reached".
         */
        @JavascriptInterface public void price(String callbackName, int id, String url) {
            new Thread(() -> {
                int status = NOT_REACHED;
                String text = "";
                HttpsURLConnection c = null;
                try {
                    if (url == null || !url.startsWith("https://api.coingecko.com/api/v3/coins/kaspa/market_chart?")) throw new IllegalArgumentException("url");
                    c = (HttpsURLConnection) new URL(url).openConnection();
                    c.setInstanceFollowRedirects(false);
                    c.setRequestMethod("GET");
                    c.setConnectTimeout(8000);
                    c.setReadTimeout(10000);
                    c.setRequestProperty("Accept", "application/json");
                    status = c.getResponseCode();
                    InputStream in = status >= 400 ? c.getErrorStream() : c.getInputStream();
                    if (in != null) {
                        ByteArrayOutputStream b = new ByteArrayOutputStream();
                        byte[] buf = new byte[8192];
                        for (int n; (n = in.read(buf)) > 0; ) {
                            if (b.size() + n > MAX_BODY) throw new IllegalStateException("body");
                            b.write(buf, 0, n);
                        }
                        text = b.toString("UTF-8");
                    }
                    if (status == 200) { text = pricesOnly(text); if (text.isEmpty()) throw new IllegalStateException("prices"); }
                    else text = "";
                } catch (Exception e) {
                    android.util.Log.w("KaspaVault", "price fetch failed (" + status + "): " + e);
                    status = NOT_REACHED;
                    text = "";
                } finally {
                    if (c != null) c.disconnect();
                }
                // One line per fetch for logcat: status and how many characters of prices reached the page.
                android.util.Log.i("KaspaVault", "price fetch -> " + status + ", " + text.length() + " chars");
                act.call(callbackName, String.valueOf(id), String.valueOf(status), JSONObject.quote(text));
            }).start();
        }

    /** The plain-words "error" of a refusal body, at most 240 characters; never the body itself (sheets travel in bodies). */
    private static String errorOf(String text) {
        try {
            String e = new JSONObject(text).optString("error", "");
            return e.length() > 120 ? e.substring(0, 120) + "..." : e;
        } catch (Exception e) {
            return "(unreadable body, " + text.length() + " chars)";
        }
    }

    /** True when the body the engine answered says ok. */
    private static boolean bodyOk(String text) {
        try {
            return new JSONObject(text).optBoolean("ok", false);
        } catch (Exception e) {
            return false;
        }
    }

    /** Only the price points of a market_chart answer, each [time, price] with a real price in (0, PRICE_MAX); "" when the shape or any number is off. */
    static String pricesOnly(String body) {
        try {
            org.json.JSONArray in = new JSONObject(body).getJSONArray("prices"), out = new org.json.JSONArray();
            if (in.length() == 0 || in.length() > PRICE_MAX_POINTS) return "";
            for (int i = 0; i < in.length(); i++) {
                org.json.JSONArray p = in.getJSONArray(i);
                double t = p.getDouble(0), v = p.getDouble(1);
                if (Double.isNaN(t) || Double.isInfinite(t) || t <= 0 || Double.isNaN(v) || Double.isInfinite(v) || v <= 0 || v >= PRICE_MAX) return "";
                out.put(new org.json.JSONArray().put((long) t).put(v));
            }
            return new JSONObject().put("prices", out).toString();
        } catch (Exception e) {
            return "";
        }
    }
}
