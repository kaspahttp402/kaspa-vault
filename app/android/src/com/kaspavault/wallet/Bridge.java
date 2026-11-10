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
import android.view.inputmethod.InputMethodManager;
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
    static final String ENGINE_UNREADABLE = "{\"ok\":false,\"error\":\"The app could not read its own answer.\"}";
    /** Body (200) when a money method needs a key this phone has not made yet. Bank words, the page shows it as is. */
    static final String NO_CARD_KEY = "{\"ok\":false,\"error\":\"This phone has no everyday card yet. Complete setup first.\"}";
    static final String NO_STOP_KEY = "{\"ok\":false,\"error\":\"This phone has no emergency stop button yet. Complete setup first.\"}";
    static final String NO_FUNDING_KEY = "{\"ok\":false,\"error\":\"This phone has no purse yet. Complete setup first.\"}";
    /** Body (200) when the engine made no keys (it refused, or answered something that is not 96 bytes). */
    static final String KEYS_NOT_MADE = "{\"ok\":false,\"error\":\"Setup could not be completed on this phone.\"}";
    static final String KEYS_EXIST = "{\"ok\":false,\"already_has_keys\":true,\"error\":\"This phone already has an account set up.\"}";
    /** Body (200) when the engine made keys but this phone could not seal them: the account must be stopped and the keys made again. */
    static final String KEYS_NOT_KEPT = "{\"ok\":false,\"error\":\"Setup finished but this phone could not store it. Stop this account and set up again.\"}";
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
    /** The fingerprint windows the Settings screen offers, in seconds ("" = the default). Nothing else is stored. */
    static final Set<String> SIGNER_WINDOW_CHOICES = new HashSet<>(Arrays.asList("", "0", "300", "900", "3600"));
    /** live_network names the engine's folder: only these ("" = mainnet) are ever stored. */
    static final Set<String> NETWORK_CHOICES =
            new HashSet<>(Arrays.asList("", "mainnet", "testnet"));
    /** Every engine method the page may call; anything else answers 404 without reaching the engine. */
    static final Set<String> ENGINE_METHODS = new HashSet<>(Arrays.asList(
            "status", "send", "finish", "stop", "decline", "funding", "fund", "topup", "takeout", "purse.pay",
            "keys.reset", "keys.public", "keys.sheets", "keys.burn", "rules.change", "heir.set",
            "guard.export", "coins.info",
            "link.code", "link.read", "link.heir_words",
            "watch.import", "watch.list", "watch.status", "watch.stop", "watch.decline", "watch.claim", "watch.forget",
            "watch.code",
            "heir.purse", "heir.pay",
            // Paper stop (docs/PAPER_STOP.md), all keyless here. check and stop need no prompt
            // and no sealed keys (a new phone after a loss); stop's args hold the 24 words.
            "paper.make", "paper.confirm", "paper.sheet", "paper.check", "paper.stop",
            // takeout: two vault seeds in the args are the authority (keyless, no prompt).
            "paper.takeout",
            // Public supply figures (keyless, read-only, cached by the engine).
            "market.supply"));
    /** The helper's moves on a watched account: the link's guardian key (safe.guardian), 32 bytes. */
    static final Set<String> GUARDIAN_METHODS = new HashSet<>(Arrays.asList("watch.stop", "watch.decline"));
    /** The link's heir key (safe.heir), 32 bytes: the claim, and the heir sheet again. */
    static final Set<String> HEIR_METHODS = new HashSet<>(Arrays.asList("watch.claim", "link.heir_words", "heir.pay"));
    /** The moves that cannot be undone: a fingerprint within Signer.FRESH_PROMPT_MS, whatever the window setting says. */
    static final Set<String> FRESH_PROMPT_METHODS = new HashSet<>(Arrays.asList("takeout", "keys.sheets", "keys.burn", "keys.reset", "heir.set",
            "watch.stop", "watch.decline", "watch.claim", "link.heir_words", "heir.pay",
            "paper.make", // a new stop key: its answer holds the 24 words
            "paper.confirm")); // the typed-back words put that key in a stop slot
    /** rules.change fields that change who can stop or inherit the account (as heir.set does). */
    static final Set<String> PEOPLE_FIELDS = new HashSet<>(Arrays.asList(
            "guardian_x", "guardian_which", "heir_x", "heir_address"));
    static final String NO_GUARDIAN_KEY = "{\"ok\":false,\"error\":\"This phone is not set up as a guardian yet. Make your link code first.\"}";
    static final String NO_HEIR_KEY = "{\"ok\":false,\"error\":\"This phone is not set up as an heir yet. Make your link code first.\"}";
    static final String LINK_NOT_MADE = "{\"ok\":false,\"error\":\"The link code could not be made on this phone.\"}";
    static final String LINK_EXISTS = "{\"ok\":false,\"error\":\"This phone already made its link code.\"}";
    static final String LINK_NOT_KEPT = "{\"ok\":false,\"error\":\"The link code was made but this phone could not store it. Try once more.\"}";
    static final String NOT_A_METHOD = "{\"ok\":false,\"error\":\"That is not something this app does.\"}";
    static final String FRESH_FINGER = "{\"ok\":false,\"error\":\"Fingerprint confirmation required. Please try again.\"}";
    /** A vault-seed action (take-out from the phone's copy, Show my sheets) whose prompt was answered with the PIN: only a finger opens them (PIN gap, safe.v2). */
    static final String FINGER_ONLY = "{\"ok\":false,\"error\":\"Use your fingerprint to open the vault seeds on this phone. Two paper vault seeds also work.\"}";
    /** A guardian or heir step whose prompt was answered with the PIN (those are kept beside the vault seeds, fingerprint only). */
    static final String LINK_FINGER_ONLY = "{\"ok\":false,\"error\":\"Use your fingerprint for this step. The PIN does not open it on this phone.\"}";
    /** Body (200) for keys.sheets, or a takeout without typed sheets, once keys.burn wiped the sealed safe keys (the engine's own words). */
    static final String SHEETS_BURNED = "{\"ok\":false,\"error\":\"The vault seeds were deleted from this phone. Only your paper copies open the vault now.\"}";
    /** Body (200) when the sealed safe keys are there but do not open (a torn store). */
    static final String NO_SAFE_KEYS = "{\"ok\":false,\"error\":\"The vault seeds kept on this phone could not be read. Type the seeds from paper.\"}";
    static final String BURN_UNCONFIRMED = "{\"ok\":false,\"error\":\"Deleting the vault seeds needs a confirmation.\"}";
    static final String BURN_NOT_DONE = "{\"ok\":false,\"burn_not_done\":true,\"error\":"
            + "\"The vault seeds could not be deleted from this phone. Nothing changed.\"}";
    static final String TWO_SHEETS = "{\"ok\":false,\"error\":\"Two vault seeds of 24 words are needed.\"}";
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

        /**
         * Allow-listed keys only; screen flags follow. Retargeting the guard (guard_nodes,
         * live_network) and widening signer_window need a fresh prompt (changeAllowed).
         */
        @JavascriptInterface public void setSetting(String key, String value) {
            if (!MainActivity.SETTINGS.contains(key)) return;
            String v = value == null ? "" : value;
            if ("signer_window".equals(key) && !SIGNER_WINDOW_CHOICES.contains(v)) return; // only what Settings offers
            if ("live_network".equals(key) && !NETWORK_CHOICES.contains(v)) return;
            if (!changeAllowed(key, v)) return;
            act.prefs.edit().putString(key, v).apply();
            if ("screenshots".equals(key)) act.main.post(act::applySecure);
            if ("signer_window".equals(key)) act.signer.shutSigner(); // a new window starts at the next fingerprint
        }

        /**
         * A script in the page may not point the guard at other nodes or another network, nor
         * stretch the fingerprint window. A CHANGE to guard_nodes or live_network, or a
         * signer_window wider than the one in force, is stored only within FRESH_PROMPT_MS of a
         * real prompt. The same guard value is a no-op; a narrower window stays free.
         */
        private boolean changeAllowed(String key, String v) {
            boolean guard = "guard_nodes".equals(key) || "live_network".equals(key);
            if (guard && v.equals(act.prefs.getString(key, ""))) return false; // same: no-op
            boolean wider = "signer_window".equals(key)
                    && windowSeconds(v) > act.signer.signerWindowSeconds();
            // Allowing screenshots needs a fresh prompt too, so a page script cannot lift capture blocking.
            boolean shots = "screenshots".equals(key) && "on".equals(v)
                    && !"on".equals(act.prefs.getString(key, "off"));
            if (!guard && !wider && !shots) return true;
            if (act.signer.promptedRecently()) return true;
            android.util.Log.i("KaspaVault", "setSetting(" + key + ") dropped: no fresh prompt");
            return false;
        }

        /** The seconds a signer_window choice means, read as signerWindowSeconds() reads it. */
        private static int windowSeconds(String v) {
            if (v.isEmpty()) return Signer.SIGNER_WINDOW_DEFAULT;
            try {
                return Math.max(0, Math.min(Signer.SIGNER_WINDOW_MAX, Integer.parseInt(v.trim())));
            } catch (NumberFormatException e) {
                return Signer.SIGNER_WINDOW_DEFAULT;
            }
        }

        /** The page reports which screen it shows; the flag decision stays here. Unknown names count as sensitive. */
        @JavascriptInterface public void screen(String name) {
            act.screen = name != null && name.matches("[a-z]{1,24}") ? name : "words";
            act.main.post(act::applySecure);
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

        /** Longest text share() hands on; longer text is cut, never refused. */
        static final int SHARE_MAX = 8000;

        /** The share sheet with plain text (statement, address, watch code); the person picks. */
        @JavascriptInterface public void share(String text) {
            if (text == null || text.isEmpty()) return;
            String t = text.length() > SHARE_MAX ? text.substring(0, SHARE_MAX) : text;
            act.main.post(() -> {
                try {
                    android.content.Intent send = new android.content.Intent(
                            android.content.Intent.ACTION_SEND);
                    send.setType("text/plain");
                    send.putExtra(android.content.Intent.EXTRA_TEXT, t);
                    android.content.Intent pick = android.content.Intent.createChooser(send, null);
                    pick.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
                    act.startActivity(pick);
                } catch (Exception ignored) { } // no app can take text: nothing opens
            });
        }

        /** The only page openUrl will hand to the system browser (the privacy page). */
        static final String PRIVACY_HOST = "sites.google.com";
        static final String PRIVACY_PATH = "/view/kaspavault-privacy";

        /**
         * Opens the privacy page in the system browser, never in the WebView. Only
         * https://sites.google.com/view/kaspavault-privacy... is allowed: no credentials, no port,
         * no query or fragment, plain ASCII. Answers true once the browser took it, false otherwise
         * (refused, or no browser on the phone).
         */
        @JavascriptInterface public boolean openUrl(String url) {
            if (url == null || url.length() > 200) return false;
            if (!url.matches("[A-Za-z0-9/:._~%-]+")) return false;
            if (!url.startsWith("https://" + PRIVACY_HOST + PRIVACY_PATH)) return false;
            final Uri u = Uri.parse(url);
            if (!"https".equals(u.getScheme()) || !PRIVACY_HOST.equals(u.getHost())) return false;
            if (u.getUserInfo() != null || u.getPort() != -1) return false;
            if (u.getQuery() != null || u.getFragment() != null) return false;
            String path = u.getEncodedPath();
            if (path == null || !path.startsWith(PRIVACY_PATH)) return false;
            if (path.contains("..") || path.contains("//")) return false;
            final boolean[] opened = {false};
            final java.util.concurrent.CountDownLatch done =
                    new java.util.concurrent.CountDownLatch(1);
            act.main.post(() -> {
                try {
                    Intent i = new Intent(Intent.ACTION_VIEW, u);
                    i.addCategory(Intent.CATEGORY_BROWSABLE);
                    i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                    act.startActivity(i);
                    opened[0] = true;
                } catch (android.content.ActivityNotFoundException e) {
                    opened[0] = false;
                } finally {
                    done.countDown();
                }
            });
            try {
                if (!done.await(3, java.util.concurrent.TimeUnit.SECONDS)) return false;
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return false;
            }
            return opened[0];
        }

        /** The camera for the code scanner on Send: asks once, answers window.KV_camera(true|false). */
        @JavascriptInterface public void askCamera() {
            act.main.post(() -> {
                if (act.checkSelfPermission(android.Manifest.permission.CAMERA) == android.content.pm.PackageManager.PERMISSION_GRANTED) act.call("KV_camera", "true");
                else act.requestPermissions(new String[]{android.Manifest.permission.CAMERA}, 7);
            });
        }

        /**
         * The guard inside the app (D5): on starts GuardService in the foreground. The first time
         * on it asks for notifications (API 33+ runtime permission, answered to
         * window.KV_notifications) and for the battery exemption (so the guard keeps hearing the
         * network through the night); both asks happen once, the phone remembers the answer.
         * Off is a plain switch too: switching alerts off needs no PIN.
         */
        @JavascriptInterface public void guardOn(boolean on) {
            android.util.Log.i("KaspaVault", "guard switch: " + (on ? "on" : "off"));
            act.insets.guardOn(on);
        }

        /** Whether the guard service is running right now (the engine accepted guard.start). */
        @JavascriptInterface public boolean guardRunning() { return act.insets.guardRunning(); }

        @JavascriptInterface public void haptic() {
            act.main.post(() -> act.web.performHapticFeedback(HapticFeedbackConstants.CONFIRM));
        }

        /**
         * Raises the keyboard for the field the page just focused. A WebView does not always show
         * the keyboard for a focus() made by script (the lock screen's PIN box).
         */
        @JavascriptInterface public void showKeyboard() {
            act.main.post(() -> {
                act.web.requestFocus();
                InputMethodManager imm =
                        (InputMethodManager) act.getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) imm.showSoftInput(act.web, InputMethodManager.SHOW_IMPLICIT);
            });
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
            // the page's subtitle may carry a name from another phone: no bidi or zero-width marks
            String shown = GuardService.printable(subtitle, GuardService.TEXT_MAX);
            act.signer.confirmWithBiometric(callbackName, shown);
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
            String shown = GuardService.printable(subtitle, GuardService.TEXT_MAX);
            act.signer.confirmSigner(callbackName, shown);
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
         * here, before the engine runs, so one fingerprint is one signature (unless Settings widens the fingerprint window). Sheet words are never logged.
         */
        @JavascriptInterface public void vault(String callbackName, int id, String method, String argsJson) {
            if (!VaultLib.AVAILABLE) { act.call(callbackName, String.valueOf(id), String.valueOf(ENGINE_MISSING), JSONObject.quote("")); return; }
            String args0 = argsJson == null ? "{}" : argsJson;
            final String m = method == null ? "" : method;
            if (!ENGINE_METHODS.contains(m)) { act.call(callbackName, String.valueOf(id), "404", JSONObject.quote(NOT_A_METHOD)); return; }
            // The irreversible moves need a fingerprint given just now. Refused in words and the signer shut, so the page's
            // next confirmSigner asks for the finger even inside the window; the user tries once more and it goes through.
            final boolean fresh = FRESH_PROMPT_METHODS.contains(m) || peopleChange(m, args0);
            if (fresh) {
                synchronized (act.bridge) {
                    if (!act.signer.consumeFresh()) { // spends the yes: one prompt, one move
                        act.signer.shutSigner();
                        act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(FRESH_FINGER));
                        return;
                    }
                }
            }
            final boolean takeout = "takeout".equals(m), sheets = "keys.sheets".equals(m), burn = "keys.burn".equals(m);
            final boolean link = GUARDIAN_METHODS.contains(m) || HEIR_METHODS.contains(m); // the safe key's link slots
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
            if (CARD_METHODS.contains(m) || STOP_METHODS.contains(m) || FUNDING_METHODS.contains(m) || TOPUP_METHODS.contains(m) || takeout || sheets || burn || link) {
                // Fingerprints changed (dead canary): the phone's copy of the vault seeds and the guardian or heir
                // slots wait 72 hours of wall AND network time. Typed paper seeds never wait. Asked outside the lock.
                boolean seedRead = link || ((sheets || (takeout && typed == null)) && act.signer.hasSlot("cold"));
                long waitHours = seedRead ? act.signer.fingerWaitHours() : 0;
                if (waitHours != 0) { // < 0: the fingerprint check could not be read just now (retry, no wait)
                    synchronized (act.bridge) { act.signer.shutSigner(); }
                    String w = waitHours < 0 ? Signer.fingerAgain() : link ? Signer.linkWait(waitHours) : Signer.seedWait(waitHours);
                    act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(w));
                    return;
                }
                String missing = null;
                byte[] a = null, b = null, c = null;
                synchronized (act.bridge) {
                    // The safe methods passed the fresh-prompt gate above; they may run on a plain prompt too (no signer key,
                    // e.g. right after a new fingerprint cleared it), since the safe store has its own key.
                    boolean safeFresh = (takeout || sheets || burn || link) && fresh;
                    if (!act.signer.signerOpen() && !safeFresh) {
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
                            } else if (act.signer.hasSlot("cold") && act.signer.hasSlot("cold2") && !act.signer.lastPromptFinger()) {
                                missing = FINGER_ONLY; // a PIN answer never opens the vault seeds on this phone
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
                            } else if (!act.signer.lastPromptFinger()) {
                                missing = FINGER_ONLY; // a PIN answer never opens the vault seeds on this phone
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
                        } else if (link) {
                            boolean guardian = GUARDIAN_METHODS.contains(m);
                            if (!act.signer.lastPromptFinger()) missing = LINK_FINGER_ONLY; // kept beside the vault seeds: fingerprint only
                            else try {
                                a = act.signer.openLink(guardian ? "guardian" : "heir");
                                if (a == null) missing = guardian ? NO_GUARDIAN_KEY : NO_HEIR_KEY;
                            } catch (android.security.keystore.UserNotAuthenticatedException e) {
                                missing = FRESH_FINGER; // the safe key's seconds after the prompt ran out
                            }
                        } else {
                            boolean confirmed;
                            try { confirmed = new JSONObject(args).optBoolean("confirm", false); } catch (org.json.JSONException e) { confirmed = false; }
                            if (!confirmed) missing = BURN_UNCONFIRMED;
                            else if (!act.signer.burnCold()) missing = BURN_NOT_DONE;
                        }
                    } finally {
                        if (sheets || burn || link) act.signer.shutSigner(); else act.signer.signerUsed(); // link: one finger per action
                    }
                }
                if (missing != null) {
                    byte[] all = Signer.joinSecrets(a, b, c); // zero-fills what was read
                    Arrays.fill(all, (byte) 0); // and the joined copy
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
                    android.util.Log.w("KaspaVault", "engine answer unreadable (" + m + "): "
                            + t.getClass().getSimpleName()); // never the message
                    status = 500;
                    text = ENGINE_UNREADABLE;
                }
                // One line per engine call for logcat: method and outcome, never a body (a refusal's plain words only).
                android.util.Log.i("KaspaVault", "engine " + m + " -> " + status + ((status == 200 && (bodyOk(text) || errorOf(text).isEmpty())) ? " ok" : " " + errorOf(text)));
                // The old key files are archived: the sealed keys of that set go too, before the page hears it.
                if ("keys.reset".equals(m) && status == 200 && bodyOk(text)) {
                    synchronized (act.bridge) { act.signer.forgetSignerPrefs(); act.signer.forgetSafe(); } // never beside a migration
                }
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
            act.signer.consumeFresh(); // this keysNew spends the prompt's yes
            act.vaultThread.execute(() -> {
                byte[] k = null;
                String text;
                try {
                    String network = act.prefs.getString("live_network", "");
                    if (network.isEmpty()) network = "mainnet"; // as MainActivity's engine init
                    String dir = act.getFilesDir() + "/vault/" + network;
                    // slots are not split by network: any sealed slot refuses a new set
                    if (act.signer.holdsAnySlot() || new java.io.File(dir, "keys.json").exists()) {
                        android.util.Log.i("KaspaVault", "keysNew -> this phone already has keys");
                        act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(KEYS_EXIST));
                        return;
                    }
                    try {
                        k = VaultLib.keysNew(args);
                    } catch (Throwable t) {
                        android.util.Log.w("KaspaVault", "keysNew failed: "
                                + t.getClass().getSimpleName()); // never the message
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
                                    String[] sc = act.signer.sealSafe(k1, k2, k3); // safe.v2: fingerprint only, survives a new fingerprint
                                    coldSafe = sc != null;
                                    if (coldSafe) {
                                        cold = sc[0];
                                        cold2 = sc[1];
                                        cold3 = sc[2];
                                    } // else (PIN answer, or the seconds ran out): the vault seeds are NOT kept on this phone, never in
                                      // the signer store again (PIN gap); the words still go to the page once, for paper ("seeds_kept":false)
                                }
                            } finally {
                                byte[] all = Signer.joinSecrets(c, st, f, k1, k2, k3); // every part
                                Arrays.fill(all, (byte) 0); // and the joined copy
                                act.signer.shutSigner();
                            }
                        }
                        boolean safeKept = k1 == null || !coldSafe || (cold != null && cold2 != null && cold3 != null);
                        if (coldSafe) act.signer.ensureCanary(); // a new v2 seal: the fingerprint check exists from now on
                        if (card != null && stop != null && funding != null && safeKept) {
                            SharedPreferences.Editor ed = act.prefs.edit().putString("signer.card", card).putString("signer.stop", stop).putString("signer.funding", funding);
                            if (k1 != null) {
                                for (String q : new String[]{"signer.", "safe.", Signer.SAFE2}) // no old set in any store shadows this one
                                    ed.remove(q + "cold").remove(q + "cold2").remove(q + "cold3");
                                if (coldSafe) ed.putString(Signer.SAFE2 + "cold", cold).putString(Signer.SAFE2 + "cold2", cold2).putString(Signer.SAFE2 + "cold3", cold3);
                            }
                            if (!ed.commit()) {
                                text = KEYS_NOT_KEPT; // not on disk: no words go out
                            } else if (k.length > tail) {
                                // the safe sheets follow the secrets: into the answer once, nowhere else
                                String tail1 = new String(k, tail, k.length - tail, java.nio.charset.StandardCharsets.UTF_8);
                                try {
                                    JSONObject answer = new JSONObject(tail1);
                                    answer.put("ok", true);
                                    if (k1 != null && !coldSafe) answer.put("seeds_kept", false); // paper only: the page says so
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
         * Makes this phone's link (docs/LINK_DESIGN.md, the helper side): args {name}. The engine's linkMake answers
         * [guardian 32][heir 32] then UTF-8 JSON {"heir_words":[24 words]} and writes the public link.json. The two keys
         * are sealed under the safe key as safe.guardian + safe.heir in one commit (they survive a new fingerprint,
         * keys.burn and keys.reset), every byte array is zero-filled, and the page gets window[callbackName](id, "200",
         * body) with body {"ok":true,"heir_words":[...]} exactly once (never logged, never stored here), or
         * {"ok":false,"error":<plain words>}; -3 when no in-app prompt just ran (the existing confirmSigner prompt, one
         * finger: a signer prompt or a plain one, as the safe methods); -4 without the engine. The engine refuses with an
         * empty array (its words go to its log), so an existing link reads as "already made". If the keys cannot be
         * sealed, the link.json this call just wrote is removed (public only, never shown) so the user can try again.
         */
        @JavascriptInterface public void linkMake(String callbackName, int id, String argsJson) {
            if (!VaultLib.AVAILABLE) { act.call(callbackName, String.valueOf(id), String.valueOf(ENGINE_MISSING), JSONObject.quote("")); return; }
            final String args = argsJson == null ? "{}" : argsJson;
            synchronized (act.bridge) {
                if (!act.signer.consumeFresh()) { // spends the yes: one prompt, one link
                    act.signer.shutSigner();
                    act.call(callbackName, String.valueOf(id), String.valueOf(LOCKED), JSONObject.quote(""));
                    return;
                }
            }
            long waitHours = act.signer.fingerWaitHours(); // fingerprints changed: the link slots wait too
            if (waitHours != 0) { // < 0: unreadable just now, refused with the retry words
                String w = waitHours < 0 ? Signer.fingerAgain() : Signer.linkWait(waitHours);
                act.call(callbackName, String.valueOf(id), "200", JSONObject.quote(w));
                return;
            }
            act.vaultThread.execute(() -> {
                byte[] k = null, g = null, h = null;
                String text;
                try {
                    String network = act.prefs.getString("live_network", "");
                    if (network.isEmpty()) network = "mainnet"; // as MainActivity's engine init
                    java.io.File linkFile = new java.io.File(act.getFilesDir() + "/vault/" + network, "link.json");
                    boolean hadLink = linkFile.exists() || act.signer.hasLink();
                    if (hadLink) {
                        text = LINK_EXISTS;
                    } else if (!act.signer.lastPromptFinger()) {
                        text = LINK_FINGER_ONLY; // kept beside the vault seeds: the PIN does not open it
                    } else if (!act.signer.safeUsableNow()) {
                        text = FRESH_FINGER; // the safe key's seconds after the prompt ran out: nothing made yet
                    } else {
                        try {
                            k = VaultLib.linkMake(args);
                        } catch (Throwable t) {
                            android.util.Log.w("KaspaVault", "linkMake failed: "
                                    + t.getClass().getSimpleName()); // never the message
                            k = null;
                        }
                        if (k == null || k.length <= 64 || k[64] != '{') {
                            text = linkFile.exists() ? LINK_EXISTS : LINK_NOT_MADE;
                        } else {
                            g = Arrays.copyOfRange(k, 0, 32);
                            h = Arrays.copyOfRange(k, 32, 64);
                            String words = null;
                            try {
                                JSONObject tail = new JSONObject(new String(k, 64, k.length - 64, java.nio.charset.StandardCharsets.UTF_8));
                                JSONArray w = tail.optJSONArray("heir_words");
                                if (w != null && w.length() > 0) {
                                    JSONObject answer = new JSONObject();
                                    answer.put("ok", true);
                                    answer.put("heir_words", w);
                                    words = answer.toString();
                                }
                            } catch (org.json.JSONException e) {
                                words = null;
                            }
                            if (words != null && act.signer.sealLink(g, h)) {
                                text = words;
                            } else {
                                if (!linkFile.delete()) android.util.Log.w("KaspaVault", "link.json not removed after a failed seal");
                                text = LINK_NOT_KEPT;
                            }
                        }
                    }
                } finally {
                    if (g != null) Arrays.fill(g, (byte) 0);
                    if (h != null) Arrays.fill(h, (byte) 0);
                    if (k != null) Arrays.fill(k, (byte) 0);
                    act.signer.shutSigner(); // one finger, one link
                }
                android.util.Log.i("KaspaVault", "linkMake -> " + (bodyOk(text) ? "ok" : errorOf(text))); // never the words
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

    /** True for a rules.change whose args name a people field (PEOPLE_FIELDS or any paper field):
     *  it changes who can stop the account, so it needs heir.set's fresh prompt. Args that cannot
     *  be read count too (the engine refuses them anyway, so asking the finger costs nothing). */
    static boolean peopleChange(String method, String argsJson) {
        if (!"rules.change".equals(method)) return false;
        try {
            java.util.Iterator<String> keys = new JSONObject(argsJson).keys();
            for (int i = 0; i < 1000 && keys.hasNext(); i++) {
                String k = keys.next();
                if (PEOPLE_FIELDS.contains(k) || k.startsWith("paper")) return true;
            }
            return keys.hasNext(); // more fields than any real change has: ask the finger
        } catch (Exception e) {
            return true;
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
