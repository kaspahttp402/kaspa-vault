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


/** The Android Keystore, biometric prompt and sealed-secret code, moved out of MainActivity unchanged. */
final class Signer {
    private final MainActivity act;
    Signer(MainActivity a) { act = a; }

    /** Fingerprint class, or the phone's own PIN / pattern / password (the phone's fingerprint OR PIN prompt). No class-2 face. */
    static final int AUTHENTICATORS = BiometricManager.Authenticators.BIOMETRIC_STRONG | BiometricManager.Authenticators.DEVICE_CREDENTIAL;
    /** A signer prompt that is not used within this many seconds is forgotten (the floor of the window below). */
    static final int SIGNER_SECONDS = 60;
    /**
     * The fingerprint window: after one in-app fingerprint, money
     * calls sign without a new prompt for this many seconds (page setting "signer_window", seconds; 0 = one
     * fingerprint per payment). Showing the sheets always takes a fresh fingerprint. The data key stays in process
     * memory for the window, exactly as it did for SIGNER_SECONDS; the Keystore key itself stays one-use.
     */
    static final int SIGNER_WINDOW_DEFAULT = 300;
    static final int SIGNER_WINDOW_MAX = 3600;
    /** A real fingerprint within this long is "fresh": the moves that cannot be undone (take-out, sheets, burn, reset, heir)
     *  need one, whatever the window says (a page that set a long window could ride one prompt for an hour). */
    static final long FRESH_PROMPT_MS = 90_000L;
    static final String KEY_ALIAS = "kaspavault.secrets.v3"; /* v3: one use per in-app fingerprint or PIN prompt, never the phone's own unlock */
    /**
     * Older keys: v1 was fingerprint-only, v2 stayed open for 30 s after any phone unlock.
     * A phone that still has one forgets its secrets once and pairs again.
     */
    static final String KEY_ALIAS_V1 = "kaspavault.secrets", KEY_ALIAS_V2 = "kaspavault.secrets.v2";
    /** Why the phone must pair again, in the page's words: after this update, or after the phone's fingerprints changed. */
    static final String REPAIR_NOTICE =
            "This update renewed how KaspaVault asks for your fingerprint. Nothing else changed.";
    static final String ENROL_NOTICE =
            "Your phone's fingerprints changed. KaspaVault will ask for your fingerprint again.";

    /**
     * The wrapping key: AES in the phone's secure hardware that works for ONE operation per in-app fingerprint or PIN
     * prompt (timeout 0), and only through the cipher that prompt was shown for (BiometricPrompt.CryptoObject). The
     * phone's own unlock never opens it, and neither does a prompt the page did not just show.
     * It wraps a random data key; confirmWithBiometric's prompt is good only if that one
     * operation succeeds. The laptop secrets the data key once sealed are gone. A new
     * fingerprint enrolment kills the wrapping key.
     */
    static SecretKey wrappingKey() throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        KeyStore.Entry e = ks.getEntry(KEY_ALIAS, null);
        if (e instanceof KeyStore.SecretKeyEntry) return ((KeyStore.SecretKeyEntry) e).getSecretKey();
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(new KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG | KeyProperties.AUTH_DEVICE_CREDENTIAL)
                .setInvalidatedByBiometricEnrollment(true)
                .setKeySize(256).build());
        return g.generateKey();
    }

    /** The data key wrapped by the Keystore key, "IV:ciphertext"; removed with the other "secret." entries. */
    static final String WRAPPED_KEY = "secret.wrapped_key";
    /** The data key after the last prompt (null when shut). Under the bridge lock. */
    private byte[] dataKey = null;

    /** The cipher the next prompt is shown for: it unwraps the stored data key, or wraps a fresh one when there is none yet. */
    Cipher prepareCipher() throws Exception {
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        String w = act.prefs.getString(WRAPPED_KEY, "");
        int at = w.indexOf(':');
        if (at > 0) c.init(Cipher.DECRYPT_MODE, wrappingKey(), new GCMParameterSpec(128, Base64.decode(w.substring(0, at), Base64.NO_WRAP)));
        else c.init(Cipher.ENCRYPT_MODE, wrappingKey());
        return c;
    }

    /** After the prompt said yes: the one authorised operation takes the data key out of the wrapped blob, or puts a new one in. */
    void unlockWith(Cipher c) throws Exception {
        String w = act.prefs.getString(WRAPPED_KEY, "");
        int at = w.indexOf(':');
        byte[] k;
        if (at > 0) {
            k = c.doFinal(Base64.decode(w.substring(at + 1), Base64.NO_WRAP));
        } else {
            k = new byte[32];
            new SecureRandom().nextBytes(k);
            byte[] ct = c.doFinal(k);
            act.prefs.edit().putString(WRAPPED_KEY, Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP)).apply();
        }
        synchronized (act.bridge) {
            shut();
            dataKey = k;
        }
    }

    void shut() {
        synchronized (act.bridge) {
            if (dataKey != null) Arrays.fill(dataKey, (byte) 0);
            dataKey = null;
        }
    }

    /** The key was invalidated (fingerprints changed): drop it and every secret, leave Live, so the user pairs again. */
    void forgetSecrets() {
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            ks.deleteEntry(KEY_ALIAS);
            ks.deleteEntry(KEY_ALIAS_V1);
            ks.deleteEntry(KEY_ALIAS_V2);
        } catch (Exception ignored) { }
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("secret.")) ed.remove(k);
        ed.putString("live_on", "").apply();
        synchronized (act.bridge) { shut(); }
        act.main.post(act::applySecure);
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The signer store: the three money keys made on this phone (the card key, emergency stop key 1 and the
    // funding key, 32 bytes each) sealed under their OWN Keystore key, kaspavault.signer.v1, with their own wrapped data
    // key and their own prefs (signer.card, signer.stop, signer.funding, signer.wrapped_key). Kept apart from the pairing
    // store on purpose: forgetSecrets() clears KEY_ALIAS and every "secret." pref on its own housekeeping (old aliases,
    // a changed fingerprint), and the money keys must never ride on that. The pairing prompt cannot open this store:
    // its CryptoObject authorises one operation on KEY_ALIAS only. One fingerprint = one signature: a confirmSigner
    // prompt opens the data key, vault() takes each slot once and shuts it the moment the engine has the bytes.
    // Nothing here is a @JavascriptInterface, and no page call reads a sealed slot.
    // ---------------------------------------------------------------------------------------------------------------
    static final String SIGNER_ALIAS = "kaspavault.signer.v1";
    static final String SIGNER_WRAPPED_KEY = "signer.wrapped_key";
    /** The three slots, as pref suffixes: signer.card, signer.stop, signer.funding. */
    static final Set<String> SIGNER_SLOTS = new HashSet<>(Arrays.asList("card", "stop", "funding", "cold", "cold2", "cold3"));
    static final String SIGNER_ENROL_NOTICE = "Your phone's fingerprints changed, so the everyday card,"
            + " emergency stop button 1 and the purse kept on this phone were cleared. This phone can no"
            + " longer pay from or stop this account. A guardian, your heir or your paper stop can still"
            + " move its funds into your vault, where your vault seeds take over. Funds already in your"
            + " vault are not affected.";
    /** The signer data key while open (null when shut), when the prompt opened it, and which slots may still be read this time. All under the bridge lock. */
    private byte[] signerDataKey = null;
    private long signerOpenedAt = 0;
    private final Set<String> signerReadable = new HashSet<>();

    /** The signer wrapping key: same recipe as wrappingKey() (one operation per in-app prompt, dies with a new fingerprint enrolment), its own alias. */
    static SecretKey signerKey() throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        KeyStore.Entry e = ks.getEntry(SIGNER_ALIAS, null);
        if (e instanceof KeyStore.SecretKeyEntry) return ((KeyStore.SecretKeyEntry) e).getSecretKey();
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(new KeyGenParameterSpec.Builder(SIGNER_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG | KeyProperties.AUTH_DEVICE_CREDENTIAL)
                .setInvalidatedByBiometricEnrollment(true)
                .setKeySize(256).build());
        return g.generateKey();
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The safe store: the three safe keys (cold, cold2, cold3: "Show my sheets" and a paperless take-out) under their
    // OWN Keystore key, kaspavault.safe.v1, with their own wrapped data key (safe.wrapped_key) and their own prefs
    // (safe.cold, safe.cold2, safe.cold3), same "IV:ciphertext" form as sealBytes. Unlike the signer key it SURVIVES a
    // new fingerprint enrolment (setInvalidatedByBiometricEnrollment(false)): a new finger clears only the everyday
    // keys (card, stop, funding), never this phone's copy of the safe sheets. forgetSigner()/forgetSignerPrefs() never
    // touch "safe."; only keys.burn (burnCold) and keys.reset (forgetSafe) do.
    // Auth mode, chosen on purpose: a short validity window (SAFE_SECONDS = 15 s), not per-operation. A per-operation
    // key needs its own CryptoObject, a BiometricPrompt carries ONE, and the page only calls confirmSigner without
    // saying which method follows, so per-operation would cost a second prompt (or a page change) for takeout and
    // keys.sheets. With the window, the one confirmSigner prompt (signer cipher, or a plain prompt when no signer cipher
    // can be built) also authorises the safe key, and the key is only used right after that prompt: the safe methods
    // are FRESH_PROMPT_METHODS (promptedRecently()), migrateCold() runs straight after the prompt, and keysNew falls
    // back to the old store when its 15 s have passed. A use past the 15 s fails with UserNotAuthenticatedException and
    // the page is asked for a fresh finger; nothing is lost. setUnlockedDeviceRequired: never usable while locked.
    // Old phones kept these as signer.cold*: readers prefer safe.* and fall back to signer.*; migrateCold() moves them
    // across (re-opened and compared before the old copy goes) at the first prompt where both keys open.
    // ---------------------------------------------------------------------------------------------------------------
    static final String SAFE_ALIAS = "kaspavault.safe.v1";
    static final String SAFE_WRAPPED_KEY = "safe.wrapped_key";
    static final int SAFE_SECONDS = 15;
    static final String[] COLD_SLOTS = {"cold", "cold2", "cold3"};
    // safe.v2 (docs/PIN_GAP_DESIGN.md, Engine 16 decision): the same store under its own alias kaspavault.safe.v2,
    // FINGERPRINT ONLY (AUTH_BIOMETRIC_STRONG, no PIN), never while locked, NOT invalidated by a new fingerprint, own
    // prefs safe2.* ("safe2." never starts with "safe."). Readers use safe2.* only when safe2.state is "proven", else
    // safe.* (v1). New seals go only into v2, never into v1 again. migrateSafeV2() moves v1 -> v2 (section 3 order).
    static final String SAFE2_ALIAS = "kaspavault.safe.v2";
    static final String SAFE2 = "safe2.";
    static final String SAFE2_WRAPPED_KEY = "safe2.wrapped_key";
    static final String SAFE2_STATE = "safe2.state";
    /** The last confirmSigner prompt was answered with a FINGER (AUTHENTICATION_RESULT_TYPE_BIOMETRIC), not the PIN. A hint; the v2 key is the gate. */
    private volatile boolean lastPromptFinger = false;
    /** The last answer was neither named FINGER nor PIN (AUTHENTICATION_RESULT_TYPE_UNKNOWN): the Keystore decides. */
    private volatile boolean lastPromptUnknown = false;
    /** When a plain prompt (no signer cipher could be built) said yes; 0 when none. Under the bridge lock. */
    private long plainPromptAt = 0;
    /**
     * A real prompt's yes, not yet spent: set when a prompt opens the signer or a plain prompt
     * says yes; cleared by consumeFresh() (one irreversible move), signerUsed() (one money call)
     * and shutSigner(). Under the bridge lock.
     */
    private boolean freshUnused = false;

    /** The safe wrapping key: usable for SAFE_SECONDS after an in-app prompt, survives a new fingerprint, never while locked. */
    static SecretKey safeKey() throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        KeyStore.Entry e = ks.getEntry(SAFE_ALIAS, null);
        if (e instanceof KeyStore.SecretKeyEntry) return ((KeyStore.SecretKeyEntry) e).getSecretKey();
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(new KeyGenParameterSpec.Builder(SAFE_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(SAFE_SECONDS, KeyProperties.AUTH_BIOMETRIC_STRONG | KeyProperties.AUTH_DEVICE_CREDENTIAL)
                .setInvalidatedByBiometricEnrollment(false)
                .setUnlockedDeviceRequired(true)
                .setKeySize(256).build());
        return g.generateKey();
    }

    /** The v2 safe wrapping key: usable for SAFE_SECONDS after a FINGER (a PIN answer gives it no token), never while locked, survives a new fingerprint. */
    static SecretKey safeKeyV2() throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        KeyStore.Entry e = ks.getEntry(SAFE2_ALIAS, null);
        if (e instanceof KeyStore.SecretKeyEntry) return ((KeyStore.SecretKeyEntry) e).getSecretKey();
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(new KeyGenParameterSpec.Builder(SAFE2_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(SAFE_SECONDS, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(false)
                .setUnlockedDeviceRequired(true)
                .setKeySize(256).build());
        return g.generateKey();
    }

    // ---- Fingerprint-change canary (docs/PIN_GAP_DESIGN.md, Engine 16 decision; built in Engine 17) ----
    // A tiny AES key, fingerprint only, that Android kills when a finger is added or removed. Made at the first finger
    // prompt (fingerSeen); checked with a bare Cipher.init (no prompt) before every vault-seed and guardian or heir action
    // and before Guard off. Dead = the fingerprints changed: those actions wait FINGER_WAIT_MS of wall time AND
    // FINGER_WAIT_DAA of network time (the engine status's daa_now, 10 per second), the wall count from when the death
    // was seen, the network count from the first fresh daa_now after it; then a fresh canary is made. A moved phone clock
    // cannot shorten the wait (the network count still runs); a network change restarts the network count (longer only).
    // Paper take-out (typed seeds) never asks.
    static final String CANARY_ALIAS = "kaspavault.finger.v1";
    static final String DEAD_WALL = "finger.dead_wall", DEAD_DAA = "finger.dead_daa", DEAD_NET = "finger.dead_net";
    static final String FINGER_PROMPTS = "finger.prompts";
    // Set once a canary was made, never removed: keystore2 may DROP an invalidated key, and absent-after-made is dead.
    static final String CANARY_MADE = "finger.canary_made";
    // fingerWaitHours when the check could not be read just now (a passing Keystore error): this action is refused,
    // no wait starts. The page matches the retry suffix word for word (data.js AGAIN).
    static final long FINGER_UNREADABLE = -1;
    static final String FINGER_AGAIN = "The phone could not check its fingerprints just now. Trying again in a moment.";
    static final String SAFE2_PROVEN_N = "safe2.proven_n";
    static final long FINGER_WAIT_MS = 72L * 3600 * 1000, FINGER_WAIT_DAA = 72L * 3600 * 10;

    /**
     * 0 none made yet, 1 alive, 2 dead (a finger was added or removed since it was made: invalidated, unrecoverable, or
     * gone after CANARY_MADE), 3 unreadable just now (any other error: the action is refused, no wait). Never prompts.
     */
    static int canaryState(SharedPreferences p) {
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            KeyStore.Entry e = ks.getEntry(CANARY_ALIAS, null);
            if (!(e instanceof KeyStore.SecretKeyEntry)) return p.getInt(CANARY_MADE, 0) == 1 ? 2 : 0;
            Cipher.getInstance("AES/GCM/NoPadding").init(Cipher.ENCRYPT_MODE, ((KeyStore.SecretKeyEntry) e).getSecretKey());
            return 1;
        } catch (android.security.keystore.UserNotAuthenticatedException e) {
            return 1; // alive: it only wants a finger
        } catch (KeyPermanentlyInvalidatedException | java.security.UnrecoverableKeyException e) {
            android.util.Log.w("KaspaVault", "fingerprint check dead: " + e.getClass().getSimpleName());
            return 2;
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "fingerprint check unreadable: " + e.getClass().getSimpleName());
            return 3;
        }
    }

    /** A fresh canary for the fingers enrolled now (throws with none enrolled). */
    static void makeCanary(SharedPreferences p) throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        if (ks.containsAlias(CANARY_ALIAS)) ks.deleteEntry(CANARY_ALIAS);
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(new KeyGenParameterSpec.Builder(CANARY_ALIAS, KeyProperties.PURPOSE_ENCRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(true)
                .setKeySize(128).build());
        g.generateKey();
        p.edit().putInt(CANARY_MADE, 1).commit();
    }

    /** A finger (or an answer the phone did not name) said yes: count the prompt; make the canary if none and no wait. */
    void fingerSeen() {
        synchronized (act.bridge) {
            act.prefs.edit().putLong(FINGER_PROMPTS, act.prefs.getLong(FINGER_PROMPTS, 0) + 1).commit();
            if (act.prefs.getLong(DEAD_WALL, 0) != 0 || canaryState(act.prefs) != 0) return;
            try {
                makeCanary(act.prefs);
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "fingerprint check not made: " + e.getClass().getSimpleName());
            }
        }
    }

    /**
     * App start and keysNew: makes the canary with no prompt (key generation needs none), so it holds the owner's
     * fingers before anyone can add one. Only with a strong fingerprint enrolled, no wait running and none made yet.
     */
    void ensureCanary() {
        try {
            BiometricManager bm = act.getSystemService(BiometricManager.class);
            int can = bm == null ? -1 : bm.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG);
            if (can != BiometricManager.BIOMETRIC_SUCCESS) return;
            synchronized (act.bridge) {
                if (act.prefs.getLong(DEAD_WALL, 0) != 0 || canaryState(act.prefs) != 0) return;
                makeCanary(act.prefs);
            }
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "fingerprint check not made at start: " + e.getClass().getSimpleName());
        }
    }

    /**
     * 0 when the fingerprints did not change since the canary was made (or the wait just ended: a fresh canary is made),
     * else the whole hours left, 1 to 72, or FINGER_UNREADABLE (-1: refuse this action, no wait written).
     * Asks the engine for network time only while a wait runs. Call it on a bridge thread: never the main thread, never the vault thread (the status call runs there).
     */
    long fingerWaitHours() {
        long wall0;
        synchronized (act.bridge) {
            wall0 = act.prefs.getLong(DEAD_WALL, 0);
            int state = wall0 == 0 ? canaryState(act.prefs) : 2;
            if (state == 3) return FINGER_UNREADABLE; // a passing Keystore error: refused now, never 72 hours
            if (state != 2) return 0;
            if (wall0 == 0) {
                wall0 = System.currentTimeMillis();
                act.prefs.edit().putLong(DEAD_WALL, wall0).remove(DEAD_DAA).remove(DEAD_NET).commit();
                android.util.Log.i("KaspaVault", "fingerprints changed: the phone's vault seed copy waits");
            }
        }
        String[] net = {act.prefs.getString("live_network", "")};
        long daa = daaNow(net);
        synchronized (act.bridge) {
            if (act.prefs.getLong(DEAD_WALL, 0) == 0) return 0; // another thread just ended it
            long start = act.prefs.getLong(DEAD_DAA, -1);
            if (daa >= 0 && (start < 0 || !net[0].equals(act.prefs.getString(DEAD_NET, "")))) {
                act.prefs.edit().putLong(DEAD_DAA, daa).putString(DEAD_NET, net[0]).commit(); // later start: only longer
                start = daa;
            }
            long msLeft = wall0 + FINGER_WAIT_MS - System.currentTimeMillis();
            long netMsLeft = start < 0 ? FINGER_WAIT_MS : daa < 0 ? 0 : (start + FINGER_WAIT_DAA - daa) * 100;
            if (msLeft <= 0 && start >= 0 && daa >= 0 && netMsLeft <= 0) return endWait();
            return Math.min(72, Math.max(1, (Math.max(msLeft, netMsLeft) + 3599999) / 3600000));
        }
    }

    /** The wait is over: a fresh canary. With no finger enrolled it starts over, so a finger added later waits too. */
    private long endWait() {
        try {
            makeCanary(act.prefs);
        } catch (Exception e) {
            act.prefs.edit().putLong(DEAD_WALL, System.currentTimeMillis()).remove(DEAD_DAA).remove(DEAD_NET).commit();
            return 72;
        }
        act.prefs.edit().remove(DEAD_WALL).remove(DEAD_DAA).remove(DEAD_NET).commit();
        android.util.Log.i("KaspaVault", "fingerprint wait over");
        return 0;
    }

    /** The engine's network time (status daa_now) on a fresh answer, else -1; net[0] becomes the network it names. */
    private long daaNow(String[] net) {
        if (!VaultLib.AVAILABLE) return -1;
        try {
            java.util.concurrent.Callable<String> ask = () -> VaultLib.call("status", "{}");
            String env = act.vaultThread.submit(ask).get(20, java.util.concurrent.TimeUnit.SECONDS);
            JSONObject o = new JSONObject(env);
            JSONObject body = o.optJSONObject("body"); // a 503 carries a fresh daa_now too (guardian-only phone)
            Object d = body == null ? null : body.opt("daa_now");
            if (!(d instanceof Number) || ((Number) d).longValue() < 0) return -1; // cached or stale answers carry none
            net[0] = body.optString("network", net[0]);
            return ((Number) d).longValue();
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "network time not read: " + e.getClass().getSimpleName());
            return -1;
        }
    }

    private static String about(long h) { return "about " + h + (h == 1 ? " hour" : " hours"); }
    /** Bodies for a dead canary (bank words; the page shows them as they are). */
    static String seedWait(long h) {
        return "{\"ok\":false,\"error\":\"The fingerprints on this phone changed, so the phone's copy of the vault seeds opens in "
                + about(h) + ". Two paper vault seeds work now.\"}";
    }
    static String linkWait(long h) {
        return "{\"ok\":false,\"error\":\"The fingerprints on this phone changed, so this guardian or heir step opens in "
                + about(h) + ".\"}";
    }
    static String fingerAgain() { return "{\"ok\":false,\"error\":\"" + FINGER_AGAIN + "\"}"; }
    static String guardWait(long h) {
        return "The fingerprints on this phone changed, so alerts can be turned off in " + about(h) + ".";
    }

    /**
     * Review: v1 goes only after v2 opened on a SECOND, later finger prompt than the one that proved
     * it, so a v2 that dies on this phone model leaves v1 in place. Under the bridge lock, straight after a finger answer.
     */
    private boolean v2OpenedLater() {
        boolean v1Left = false;
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("safe.")) v1Left = true;
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            v1Left |= ks.containsAlias(SAFE_ALIAS);
        } catch (Exception ignored) { }
        if (!v1Left) return false;
        long n = act.prefs.getLong(FINGER_PROMPTS, 0), at = act.prefs.getLong(SAFE2_PROVEN_N, -1);
        if (at < 0) { act.prefs.edit().putLong(SAFE2_PROVEN_N, n).commit(); return false; } // proven before this rule
        if (n <= at) return false;
        byte[] dk = null;
        try {
            dk = unwrapWith(safeKeyV2(), act.prefs.getString(SAFE2_WRAPPED_KEY, ""));
            return true;
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "safe.v2 did not open again, old copy kept: " + e.getClass().getSimpleName());
            return false;
        } finally {
            if (dk != null) Arrays.fill(dk, (byte) 0);
        }
    }

    boolean v2Proven() { return "proven".equals(act.prefs.getString(SAFE2_STATE, "")); }
    /** The prefs prefix the readers use now: "safe2." once safe2.state is "proven", else "safe." (v1). */
    String safePrefix() { return v2Proven() ? SAFE2 : "safe."; }
    /** A finger answer; an unnamed one counts only once readers are on safe.v2 (its key gives a PIN no token). */
    boolean lastPromptFinger() { return lastPromptFinger && (!lastPromptUnknown || v2Proven()); }

    /** A slot sealed in the store the readers use (v1 while v2 is not proven; v2 once it is; v1 leftovers count too). */
    private boolean sealedSafe(String name) {
        return act.prefs.getString("safe." + name, "").indexOf(':') > 0 || (v2Proven() && act.prefs.getString(SAFE2 + name, "").indexOf(':') > 0);
    }

    /** Every sealed slot under a prefix ("safe." or "safe2."), name -> "IV:ciphertext"; never the wrapped key or the state. */
    private java.util.TreeMap<String, String> slotsUnder(String prefix) {
        java.util.TreeMap<String, String> out = new java.util.TreeMap<>();
        for (java.util.Map.Entry<String, ?> e : act.prefs.getAll().entrySet()) {
            String k = e.getKey();
            if (!k.startsWith(prefix) || k.equals(SAFE_WRAPPED_KEY) || k.equals(SAFE2_WRAPPED_KEY) || k.equals(SAFE2_STATE)) continue;
            if (e.getValue() instanceof String && ((String) e.getValue()).indexOf(':') > 0) out.put(k.substring(prefix.length()), (String) e.getValue());
        }
        return out;
    }

    /** Removes every safe2.* pref (a failed or stray v2 copy). Never touches safe.*. The v2 alias stays (it holds nothing alone). */
    private boolean clearV2Prefs() {
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith(SAFE2)) ed.remove(k);
        return ed.commit();
    }

    /** The second commit after "proven": every safe.* pref goes, then alias v1. Does nothing unless safe2.state is "proven". */
    private void dropV1() {
        if (!v2Proven()) return;
        SharedPreferences.Editor ed = act.prefs.edit();
        boolean any = false;
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("safe.")) { ed.remove(k); any = true; }
        if (any && !ed.commit()) { android.util.Log.w("KaspaVault", "old vault seed copy not removed yet"); return; } // alias v1 stays while any safe.* does
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            if (ks.containsAlias(SAFE_ALIAS)) ks.deleteEntry(SAFE_ALIAS);
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "old safe alias not deleted: " + e.getClass().getSimpleName());
        }
    }

    /** "IV:ciphertext" pref -> the data key it wraps under a Keystore key. */
    private static byte[] unwrapWith(SecretKey key, String w) throws Exception {
        int at = w.indexOf(':');
        if (at <= 0) throw new IllegalStateException("no safe data key");
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        c.init(Cipher.DECRYPT_MODE, key, new GCMParameterSpec(128, Base64.decode(w.substring(0, at), Base64.NO_WRAP)));
        return c.doFinal(Base64.decode(w.substring(at + 1), Base64.NO_WRAP));
    }

    /** A data key wrapped under a Keystore key, as "IV:ciphertext". */
    private static String wrapWith(SecretKey key, byte[] k) throws Exception {
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        c.init(Cipher.ENCRYPT_MODE, key);
        byte[] ct = c.doFinal(k);
        return Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP);
    }

    /** The v2 data key, read from prefs. Refused outright after a PIN answer (the Keystore refuses it too). */
    private byte[] dataKeyV2() throws Exception {
        if (!lastPromptFinger) throw new android.security.keystore.UserNotAuthenticatedException("the PIN never opens safe.v2");
        return unwrapWith(safeKeyV2(), act.prefs.getString(SAFE2_WRAPPED_KEY, ""));
    }

    /**
     * A fresh v2 store when v1 holds no slot: new data key wrapped under v2, committed with state "written", unwrapped
     * AGAIN from prefs and compared, then state "proven" (then the empty v1 goes). Finger answers only.
     */
    private byte[] newV2() throws Exception {
        if (!lastPromptFinger) throw new android.security.keystore.UserNotAuthenticatedException("the PIN never opens safe.v2");
        if (!clearV2Prefs()) throw new IllegalStateException("old v2 copy not cleared");
        byte[] k = new byte[32];
        new SecureRandom().nextBytes(k);
        byte[] back = null;
        try {
            if (!act.prefs.edit().putString(SAFE2_WRAPPED_KEY, wrapWith(safeKeyV2(), k)).putString(SAFE2_STATE, "written").commit())
                throw new IllegalStateException("safe data key not stored");
            back = unwrapWith(safeKeyV2(), act.prefs.getString(SAFE2_WRAPPED_KEY, ""));
            if (!java.security.MessageDigest.isEqual(back, k)) throw new IllegalStateException("safe data key differs");
            if (!act.prefs.edit().putString(SAFE2_STATE, "proven").putLong(SAFE2_PROVEN_N, act.prefs.getLong(FINGER_PROMPTS, 0)).commit())
                throw new IllegalStateException("safe data key not proven");
        } catch (Exception x) {
            Arrays.fill(k, (byte) 0);
            if (!v2Proven()) clearV2Prefs();
            throw x;
        } finally {
            if (back != null) Arrays.fill(back, (byte) 0);
        }
        // v1 held no slot: its wrapped key and alias go once v2 opens on a later finger prompt (v2OpenedLater)
        return k;
    }

    /**
     * docs/PIN_GAP_DESIGN.md section 3, real funds: v1 is never deleted before v2 is written AND proven. Runs straight
     * after a confirmSigner prompt answered with a FINGER (both safe keys inside SAFE_SECONDS); a PIN answer never
     * migrates. States: (none) -> step 3-4: new data key under v2, every safe.* slot sealed, ONE commit of
     * safe2.wrapped_key + safe2.* + safe2.state=written (safe.* untouched) -> step 5: safe2.wrapped_key re-read FROM
     * PREFS, unwrapped with v2, every safe2.* opened and compared byte for byte with the v1 bytes (same slot set) ->
     * step 6: commit safe2.state=proven, and a SECOND commit (safe.* and alias v1 go, dropV1) waits for a LATER finger prompt that opens v2 again. Killed before
     * "written": nothing new on disk. Killed at "written": both copies, readers on v1, the next finger redoes 5-6.
     * Killed at "proven" before the second commit: both copies, readers on v2, the next finger finishes dropV1. Any
     * failure before "proven" removes only safe2.* (never safe.*) and the next finger tries again.
     */
    void migrateSafeV2() {
        synchronized (act.bridge) {
            if (!lastPromptFinger) return; // a PIN answer never migrates
            if (v2Proven()) { if (v2OpenedLater()) dropV1(); return; } // the second commit, on a LATER finger prompt
            java.util.TreeMap<String, String> v1 = slotsUnder("safe.");
            if (v1.isEmpty()) return; // nothing to move; a new seal makes v2 itself (newV2)
            java.util.TreeMap<String, byte[]> plain = new java.util.TreeMap<>();
            byte[] dk1 = null, dk2 = null, dk3 = null;
            try {
                dk1 = unwrapWith(safeKey(), act.prefs.getString(SAFE_WRAPPED_KEY, "")); // step 2: v1 window, same prompt
                for (java.util.Map.Entry<String, String> e : v1.entrySet()) plain.put(e.getKey(), openWith(dk1, e.getValue()));
                if (!"written".equals(act.prefs.getString(SAFE2_STATE, ""))) {
                    if (!clearV2Prefs()) throw new IllegalStateException("old v2 copy not cleared"); // stray safe2.* from a failed try
                    dk2 = new byte[32]; // step 3
                    new SecureRandom().nextBytes(dk2);
                    SharedPreferences.Editor ed = act.prefs.edit().putString(SAFE2_WRAPPED_KEY, wrapWith(safeKeyV2(), dk2));
                    for (java.util.Map.Entry<String, byte[]> e : plain.entrySet()) ed.putString(SAFE2 + e.getKey(), sealWith(dk2, e.getValue()));
                    ed.putString(SAFE2_STATE, "written");
                    if (!ed.commit()) throw new IllegalStateException("v2 copy not stored"); // step 4: ONE commit
                }
                dk3 = unwrapWith(safeKeyV2(), act.prefs.getString(SAFE2_WRAPPED_KEY, "")); // step 5: from prefs, as every later reader
                java.util.TreeMap<String, String> v2 = slotsUnder(SAFE2);
                if (!v2.keySet().equals(plain.keySet())) throw new IllegalStateException("v2 slots differ");
                for (java.util.Map.Entry<String, byte[]> e : plain.entrySet()) {
                    byte[] back = openWith(dk3, v2.get(e.getKey()));
                    boolean same = java.security.MessageDigest.isEqual(back, e.getValue());
                    Arrays.fill(back, (byte) 0);
                    if (!same) throw new IllegalStateException("v2 copy differs");
                }
                if (!act.prefs.edit().putString(SAFE2_STATE, "proven").putLong(SAFE2_PROVEN_N, act.prefs.getLong(FINGER_PROMPTS, 0)).commit())
                    throw new IllegalStateException("v2 not proven"); // step 6
                android.util.Log.i("KaspaVault", "vault seeds moved");
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "vault seeds stay in safe.v1: " + e.getClass().getSimpleName());
                if (!v2Proven()) clearV2Prefs(); // only safe2.*, never safe.*
                return;
            } finally {
                for (byte[] p : plain.values()) if (p != null) Arrays.fill(p, (byte) 0);
                if (dk1 != null) Arrays.fill(dk1, (byte) 0);
                if (dk2 != null) Arrays.fill(dk2, (byte) 0);
                if (dk3 != null) Arrays.fill(dk3, (byte) 0);
            }
            // no dropV1 here: v1 stays until v2 opens again on a LATER finger prompt (review)
        }
    }

    /** True when any safe slot is sealed on this phone (safe.* or the old signer.*). */
    boolean hasAnyCold() {
        for (String n : COLD_SLOTS) if (hasSlot(n)) return true;
        return false;
    }

    /**
     * The safe data key out of safe.wrapped_key, or (create, and none stored yet) a fresh one wrapped and committed.
     * Throws UserNotAuthenticatedException more than SAFE_SECONDS after the last prompt. The caller zero-fills it.
     */
    private byte[] safeDataKey(boolean create) throws Exception {
        if (v2Proven()) return dataKeyV2();
        if (!create) return unwrapWith(safeKey(), act.prefs.getString(SAFE_WRAPPED_KEY, "")); // v1, read only
        if (!slotsUnder("safe.").isEmpty()) throw new IllegalStateException("vault seeds not moved to safe.v2 yet"); // never a new seal in v1
        return newV2();
    }

    /** "IV:ciphertext" of v under a data key (AES-GCM). */
    static String sealWith(byte[] dk, byte[] v) throws Exception {
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        c.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(dk, "AES"));
        byte[] ct = c.doFinal(v);
        return Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP);
    }

    /** The bytes of an "IV:ciphertext" under a data key; throws when it is not there or does not decrypt. */
    static byte[] openWith(byte[] dk, String sealed) throws Exception {
        int at = sealed.indexOf(':');
        if (at <= 0) throw new IllegalStateException("not sealed");
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        c.init(Cipher.DECRYPT_MODE, new SecretKeySpec(dk, "AES"), new GCMParameterSpec(128, Base64.decode(sealed.substring(0, at), Base64.NO_WRAP)));
        return c.doFinal(Base64.decode(sealed.substring(at + 1), Base64.NO_WRAP));
    }

    /**
     * keysNew: the safe keys sealed under the v2 safe data key (prefs safe2.*), in order, or null when v2 cannot be
     * used now (a PIN answer, its SAFE_SECONDS ran out, or v1 not moved yet): keysNew then keeps no vault seeds on this
     * phone (never the signer store again). The inputs are not zero-filled here (keysNew does that).
     */
    String[] sealSafe(byte[]... v) {
        synchronized (act.bridge) {
            byte[] dk = null;
            try {
                dk = safeDataKey(true);
                String[] out = new String[v.length];
                for (int i = 0; i < v.length; i++) out[i] = sealWith(dk, v[i]);
                return out;
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "safe store not usable now: " + e.getClass().getSimpleName());
                return null;
            } finally {
                if (dk != null) Arrays.fill(dk, (byte) 0);
            }
        }
    }

    /**
     * vault(): the named safe slots' bytes, safe.* first, else the old signer.* (that needs the signer store open, once
     * per slot per prompt, as openBytes). A null entry when a slot is missing or does not decrypt. Throws
     * UserNotAuthenticatedException when the safe key's SAFE_SECONDS after the prompt are over (the page then asks for
     * a fresh finger). The caller zero-fills every array.
     */
    byte[][] openCold(String... names) throws android.security.keystore.UserNotAuthenticatedException {
        synchronized (act.bridge) {
            byte[][] out = new byte[names.length][];
            byte[] dk = null;
            final String pre = safePrefix();
            try {
                for (int i = 0; i < names.length; i++) {
                    String v = act.prefs.getString(pre + names[i], "");
                    if (v.indexOf(':') > 0) {
                        try {
                            if (dk == null) dk = safeDataKey(false);
                            out[i] = openWith(dk, v);
                        } catch (android.security.keystore.UserNotAuthenticatedException e) {
                            for (byte[] o : out) if (o != null) Arrays.fill(o, (byte) 0);
                            throw e;
                        } catch (Exception e) {
                            out[i] = null;
                        }
                    } else {
                        out[i] = openBytes(names[i]);
                    }
                }
                return out;
            } finally {
                if (dk != null) Arrays.fill(dk, (byte) 0);
            }
        }
    }

    /**
     * Moves old signer.cold* into safe.cold*, never lossy. Runs straight after a signer prompt (both keys open: the
     * signer data key in memory, the safe key inside its SAFE_SECONDS), only when some signer.cold* exist and no
     * safe.cold* do. Each value is sealed under the safe data key, then the data key is unwrapped AGAIN from the stored
     * safe.wrapped_key and every sealed value re-opened and compared byte for byte; only then are safe.* written and
     * signer.cold* removed, in one commit. Any failure leaves signer.cold* untouched (the readers fall back to them).
     */
    void migrateCold() {
        synchronized (act.bridge) {
            if (signerDataKey == null || !lastPromptFinger) return; // into safe.v2 only, on a finger answer
            boolean legacy = false;
            for (String n : COLD_SLOTS) {
                if (sealedSafe(n)) return;
                if (act.prefs.getString("signer." + n, "").indexOf(':') > 0) legacy = true;
            }
            if (!legacy) return;
            byte[] dk = null, dk2 = null;
            byte[][] plain = new byte[COLD_SLOTS.length][];
            String[] sealed = new String[COLD_SLOTS.length];
            try {
                for (int i = 0; i < COLD_SLOTS.length; i++) {
                    String old = act.prefs.getString("signer." + COLD_SLOTS[i], "");
                    if (old.indexOf(':') <= 0) continue;
                    plain[i] = openWith(signerDataKey, old);
                    if (dk == null) dk = safeDataKey(true);
                    sealed[i] = sealWith(dk, plain[i]);
                }
                dk2 = safeDataKey(false); // from the stored blob, as every later reader will
                if (!v2Proven()) throw new IllegalStateException("safe.v2 not proven"); // never into v1 again
                SharedPreferences.Editor ed = act.prefs.edit();
                for (int i = 0; i < COLD_SLOTS.length; i++) {
                    if (sealed[i] == null) continue;
                    byte[] back = openWith(dk2, sealed[i]);
                    boolean same = java.security.MessageDigest.isEqual(back, plain[i]);
                    Arrays.fill(back, (byte) 0);
                    if (!same) throw new IllegalStateException("safe copy differs");
                    ed.putString(SAFE2 + COLD_SLOTS[i], sealed[i]).remove("signer." + COLD_SLOTS[i]);
                }
                if (!ed.commit()) throw new IllegalStateException("safe copy not stored");
                android.util.Log.i("KaspaVault", "safe keys moved to the safe store");
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "safe keys stay in the old store: " + e.getClass().getSimpleName());
            } finally {
                for (byte[] p : plain) if (p != null) Arrays.fill(p, (byte) 0);
                if (dk != null) Arrays.fill(dk, (byte) 0);
                if (dk2 != null) Arrays.fill(dk2, (byte) 0);
            }
        }
    }

    /** keys.burn: only the sealed safe keys, in both stores (safe.cold*, old signer.cold*). */
    boolean burnCold() {
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String n : COLD_SLOTS) ed.remove("safe." + n).remove(SAFE2 + n).remove("signer." + n);
        return ed.commit(); // false: not on disk, and the page is told
    }

    /**
     * keys.reset (engine ok): the old safe goes too, alias and every "safe." pref, so it never shadows the next set.
     * While a link is sealed (safe.guardian / safe.heir) the link stays: the alias, safe.wrapped_key and the two link
     * slots are kept and only the wallet's own safe slots go (the link is separate from the wallet's keys).
     */
    void forgetSafe() {
        if (hasLink()) {
            SharedPreferences.Editor ed = act.prefs.edit();
            for (String k : act.prefs.getAll().keySet()) {
                if (!(k.startsWith("safe.") || k.startsWith(SAFE2)) || SAFE_WRAPPED_KEY.equals(k) || SAFE2_WRAPPED_KEY.equals(k) || SAFE2_STATE.equals(k)) continue;
                boolean link = false;
                for (String n : LINK_SLOTS) if (k.equals("safe." + n) || k.equals(SAFE2 + n)) link = true;
                if (!link) ed.remove(k);
            }
            ed.apply();
            return;
        }
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            ks.deleteEntry(SAFE_ALIAS);
            ks.deleteEntry(SAFE2_ALIAS);
        } catch (Exception ignored) { }
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("safe.") || k.startsWith(SAFE2)) ed.remove(k);
        ed.apply();
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The link (guardians and heirs, docs/LINK_DESIGN.md): this phone's guardian key and heir key, the helper's side,
    // sealed under the SAME safe key and safe data key as safe.cold* (prefs safe.guardian, safe.heir). So they survive
    // a new fingerprint (signer clearing never touches "safe."), keys.burn (burnCold removes only the cold slots) and
    // keys.reset (forgetSafe keeps them while a link is sealed). Sealed together in one commit, read one at a time
    // (each read inside the safe key's SAFE_SECONDS after the prompt), forgotten together (forgetLink).
    // ---------------------------------------------------------------------------------------------------------------
    static final String[] LINK_SLOTS = {"guardian", "heir"};

    /** True when either link key is sealed on this phone. */
    boolean hasLink() {
        for (String n : LINK_SLOTS) if (sealedSafe(n)) return true;
        return false;
    }

    /** linkMake, before the engine runs: true when the safe data key opens (or is made) now, inside SAFE_SECONDS. */
    boolean safeUsableNow() {
        synchronized (act.bridge) {
            byte[] dk = null;
            try {
                dk = safeDataKey(true);
                return true;
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "safe store not usable now: " + e.getClass().getSimpleName());
                return false;
            } finally {
                if (dk != null) Arrays.fill(dk, (byte) 0);
            }
        }
    }

    /**
     * linkMake: seals both link keys under the safe data key, re-opens each from the stored safe.wrapped_key and
     * compares byte for byte (as migrateCold), then writes safe.guardian + safe.heir in ONE commit. False when any
     * step fails (nothing written then). The inputs are not zero-filled here (the caller does that).
     */
    boolean sealLink(byte[] guardian, byte[] heir) {
        synchronized (act.bridge) {
            byte[] dk = null, dk2 = null;
            try {
                dk = safeDataKey(true);
                String g = sealWith(dk, guardian), h = sealWith(dk, heir);
                dk2 = safeDataKey(false); // from the stored blob, as every later reader will
                byte[] bg = openWith(dk2, g), bh = openWith(dk2, h);
                boolean same = java.security.MessageDigest.isEqual(bg, guardian) && java.security.MessageDigest.isEqual(bh, heir);
                Arrays.fill(bg, (byte) 0);
                Arrays.fill(bh, (byte) 0);
                if (!same) throw new IllegalStateException("link copy differs");
                if (!v2Proven()) throw new IllegalStateException("safe.v2 not proven"); // never into v1 again
                if (!act.prefs.edit().putString(SAFE2 + "guardian", g).putString(SAFE2 + "heir", h).commit()) throw new IllegalStateException("link not stored");
                return true;
            } catch (Exception e) {
                android.util.Log.w("KaspaVault", "link keys not kept: " + e.getClass().getSimpleName());
                return false;
            } finally {
                if (dk != null) Arrays.fill(dk, (byte) 0);
                if (dk2 != null) Arrays.fill(dk2, (byte) 0);
            }
        }
    }

    /**
     * vault(): one link key ("guardian" or "heir"), or null when it is not sealed or does not decrypt. Throws
     * UserNotAuthenticatedException when the safe key's SAFE_SECONDS after the prompt are over. The caller zero-fills it.
     */
    byte[] openLink(String name) throws android.security.keystore.UserNotAuthenticatedException {
        if (!"guardian".equals(name) && !"heir".equals(name)) return null;
        synchronized (act.bridge) {
            String v = act.prefs.getString(safePrefix() + name, "");
            if (v.indexOf(':') <= 0) return null;
            byte[] dk = null;
            try {
                dk = safeDataKey(false);
                return openWith(dk, v);
            } catch (android.security.keystore.UserNotAuthenticatedException e) {
                throw e;
            } catch (Exception e) {
                return null;
            } finally {
                if (dk != null) Arrays.fill(dk, (byte) 0);
            }
        }
    }

    /** Drops both link keys together (only when the link itself is gone; keys.reset and keys.burn never call this). */
    void forgetLink() {
        act.prefs.edit().remove("safe.guardian").remove("safe.heir").remove(SAFE2 + "guardian").remove(SAFE2 + "heir").commit();
    }

    /** The cipher the next signer prompt is shown for: unwraps the signer data key, or wraps a fresh one when there is none yet (first keysNew). */
    Cipher signerCipher() throws Exception {
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        String w = act.prefs.getString(SIGNER_WRAPPED_KEY, "");
        int at = w.indexOf(':');
        if (at > 0) c.init(Cipher.DECRYPT_MODE, signerKey(), new GCMParameterSpec(128, Base64.decode(w.substring(0, at), Base64.NO_WRAP)));
        else c.init(Cipher.ENCRYPT_MODE, signerKey());
        return c;
    }

    /** After the signer prompt said yes: the one authorised operation takes the signer data key out of its wrapped blob, or puts a new one in. */
    void openSignerWith(Cipher c) throws Exception {
        String w = act.prefs.getString(SIGNER_WRAPPED_KEY, "");
        int at = w.indexOf(':');
        byte[] k;
        if (at > 0) {
            k = c.doFinal(Base64.decode(w.substring(at + 1), Base64.NO_WRAP));
        } else {
            k = new byte[32];
            new SecureRandom().nextBytes(k);
            byte[] ct = c.doFinal(k);
            String wrapped = Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":"
                    + Base64.encodeToString(ct, Base64.NO_WRAP);
            if (!act.prefs.edit().putString(SIGNER_WRAPPED_KEY, wrapped).commit()) {
                Arrays.fill(k, (byte) 0); // never seal under a key that is not on disk
                throw new IllegalStateException("signer key not stored");
            }
        }
        synchronized (act.bridge) {
            shutSigner();
            signerDataKey = k;
            signerOpenedAt = android.os.SystemClock.elapsedRealtime();
            signerReadable.addAll(SIGNER_SLOTS);
            freshUnused = true; // a real prompt: ONE irreversible move may follow (consumeFresh)
        }
    }

    /** The fingerprint window in seconds from the page setting, clamped to 0..SIGNER_WINDOW_MAX. */
    int signerWindowSeconds() {
        try {
            String v = act.prefs.getString("signer_window", "");
            return v.isEmpty() ? SIGNER_WINDOW_DEFAULT : Math.max(0, Math.min(SIGNER_WINDOW_MAX, Integer.parseInt(v.trim())));
        } catch (NumberFormatException e) {
            return SIGNER_WINDOW_DEFAULT;
        }
    }

    /** After a money call took its bytes: inside the window the slots become readable again, else the key is dropped. */
    void signerUsed() {
        synchronized (act.bridge) {
            freshUnused = false; // the money call spent the prompt's yes
            if (signerWindowSeconds() > 0 && signerOpen()) signerReadable.addAll(SIGNER_SLOTS);
            else shutSigner();
        }
    }

    /** True while a signer prompt's data key is open; it is forgotten after the window (at least SIGNER_SECONDS). */
    boolean signerOpen() {
        synchronized (act.bridge) {
            long keep = Math.max(SIGNER_SECONDS, signerWindowSeconds()) * 1000L;
            if (signerDataKey != null && android.os.SystemClock.elapsedRealtime() - signerOpenedAt > keep) shutSigner();
            return signerDataKey != null;
        }
    }

    /**
     * True only within FRESH_PROMPT_MS of a real prompt whose yes is unspent (freshUnused; the
     * window shortcut neither moves signerOpenedAt nor sets it); a plain prompt (no signer
     * cipher) counts too. Reads, never spends.
     */
    boolean promptedRecently() {
        synchronized (act.bridge) {
            if (!freshUnused) return false;
            long now = android.os.SystemClock.elapsedRealtime();
            return (signerDataKey != null && now - signerOpenedAt <= FRESH_PROMPT_MS) || (plainPromptAt > 0 && now - plainPromptAt <= FRESH_PROMPT_MS);
        }
    }

    /** promptedRecently(), then the yes is spent: one prompt, one irreversible move. */
    boolean consumeFresh() {
        synchronized (act.bridge) {
            boolean ok = promptedRecently();
            freshUnused = false;
            return ok;
        }
    }

    /** Drops the signer data key: called the moment vault() has the bytes for the engine, or keysNew has sealed them. */
    void shutSigner() {
        synchronized (act.bridge) {
            if (signerDataKey != null) Arrays.fill(signerDataKey, (byte) 0);
            signerDataKey = null;
            signerReadable.clear();
            plainPromptAt = 0;
            freshUnused = false;
        }
    }

    /**
     * One slot's sealed form, "IV:ciphertext" under the open signer data key; null when the store is shut or the slot
     * name is not one of the three. Writes nothing itself: keysNew seals all three and commits them in one edit, so a
     * half-made set never lands. Never a page call.
     */
    String sealBytes(String name, byte[] v) {
        synchronized (act.bridge) {
            if (!SIGNER_SLOTS.contains(name) || signerDataKey == null || v == null) return null;
            try {
                Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
                c.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(signerDataKey, "AES"));
                byte[] ct = c.doFinal(v);
                return Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP);
            } catch (Exception e) {
                return null;
            }
        }
    }

    /**
     * One slot's bytes, decrypted under the open signer data key: once per slot per prompt. Null when the store is shut,
     * the slot was already read since the prompt, nothing is stored under it, or it does not decrypt. The caller fills
     * the array with zeros as soon as the engine has it. Never a page call.
     */
    byte[] openBytes(String name) {
        synchronized (act.bridge) {
            if (!SIGNER_SLOTS.contains(name) || !signerOpen() || !signerReadable.remove(name)) return null;
            String v = act.prefs.getString("signer." + name, "");
            int at = v.indexOf(':');
            if (at < 0) return null;
            try {
                Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
                c.init(Cipher.DECRYPT_MODE, new SecretKeySpec(signerDataKey, "AES"), new GCMParameterSpec(128, Base64.decode(v.substring(0, at), Base64.NO_WRAP)));
                return c.doFinal(Base64.decode(v.substring(at + 1), Base64.NO_WRAP));
            } catch (Exception e) {
                return null;
            }
        }
    }

    /** True when this phone has a sealed key under the slot (the engine has made keys here); the safe slots count under safe.* or the old signer.*. */
    boolean hasSlot(String name) { return act.prefs.getString("signer." + name, "").indexOf(':') > 0 || sealedSafe(name); }

    /** An account key is sealed here (card, stop, funding or a cold slot), on any network. */
    boolean holdsAnySlot() {
        return hasSlot("card") || hasSlot("stop") || hasSlot("funding") || hasAnyCold();
    }

    /**
     * The signer key was invalidated (fingerprints changed): drop that alias and the everyday sealed keys (card, stop,
     * funding, plus any old signer.cold* that migrateCold had not moved yet: unreadable without that alias anyway),
     * nothing else. The safe store (SAFE_ALIAS, "safe." prefs) survives a new fingerprint and is not touched; nor is the
     * pairing store (KEY_ALIAS, "secret." prefs, live_on); forgetSecrets() never touches this one.
     */
    void forgetSigner() {
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            ks.deleteEntry(SIGNER_ALIAS);
        } catch (Exception ignored) { }
        forgetSignerPrefs();
    }

    /**
     * Drops only the sealed keys (every "signer." pref: card, stop, funding, and old cold, cold2, cold3) and shuts the
     * store; the signer alias stays, so the next confirmSigner needs no re-enrolment. keys.reset uses this (with
     * forgetSafe) once the engine has archived the old key files, so the old sealed keys never shadow the next set.
     * Nothing outside "signer." is touched: the "safe." prefs stay.
     */
    void forgetSignerPrefs() {
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("signer.")) ed.remove(k);
        ed.apply();
        shutSigner();
    }

    /** One block out of the parts that are there, in order; every part is zero-filled on the way. */
    static byte[] joinSecrets(byte[]... parts) {
        int n = 0;
        for (byte[] p : parts) if (p != null) n += p.length;
        byte[] out = new byte[n];
        int at = 0;
        for (byte[] p : parts) {
            if (p == null) continue;
            System.arraycopy(p, 0, out, at, p.length);
            at += p.length;
            Arrays.fill(p, (byte) 0);
        }
        return out;
    }

    /**
     * The laptop pairing is gone: an old install's sealed laptop token and pin
     * ("secret.live_*") are dropped at start. "secret.wrapped_key" stays:
     * confirmWithBiometric's prompt still uses it.
     */
    void dropLaptopSecrets() {
        SharedPreferences.Editor ed = act.prefs.edit();
        boolean any = false;
        for (String k : act.prefs.getAll().keySet()) {
            if (k.startsWith("secret.live_")) { ed.remove(k); any = true; }
        }
        if (any) ed.apply();
    }

    void confirmWithBiometric(String callbackName, String subtitle) {
            act.main.post(() -> {
                BiometricManager bm = act.getSystemService(BiometricManager.class);
                if (bm == null || bm.canAuthenticate(AUTHENTICATORS) != BiometricManager.BIOMETRIC_SUCCESS) {
                    act.call(callbackName, act.live() ? "\"refused\"" : "\"unavailable\"");
                    return;
                }
                Cipher cipher; // the one operation this prompt authorises: it opens the stored secrets, and nothing else can
                try {
                    cipher = prepareCipher();
                } catch (KeyPermanentlyInvalidatedException e) {
                    forgetSecrets(); act.notice = ENROL_NOTICE; act.pushNotice();
                    act.call(callbackName, act.live() ? "\"refused\"" : "\"unavailable\"");
                    return;
                } catch (Exception e) {
                    act.call(callbackName, act.live() ? "\"refused\"" : "\"unavailable\"");
                    return;
                }
                BiometricPrompt prompt = new BiometricPrompt.Builder(act)
                        .setTitle("KaspaVault").setSubtitle(subtitle)
                        /* no negative button: with DEVICE_CREDENTIAL allowed the prompt offers the PIN there; the user leaves with back */
                        .setConfirmationRequired(false)
                        .setAllowedAuthenticators(AUTHENTICATORS).build();
                prompt.authenticate(new BiometricPrompt.CryptoObject(cipher), new CancellationSignal(), act.getMainExecutor(), new BiometricPrompt.AuthenticationCallback() {
                    @Override public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                        try {
                            BiometricPrompt.CryptoObject co = r.getCryptoObject();
                            unlockWith(co != null && co.getCipher() != null ? co.getCipher() : cipher);
                            act.call(callbackName, "\"ok\"");
                        } catch (KeyPermanentlyInvalidatedException e) {
                            forgetSecrets(); act.notice = ENROL_NOTICE; act.pushNotice();
                            act.call(callbackName, act.live() ? "\"refused\"" : "\"ok\""); // Sample mode: the gesture itself was real and nothing is at stake
                        } catch (Exception e) {
                            act.call(callbackName, act.live() ? "\"refused\"" : "\"ok\"");
                        }
                    }
                    @Override public void onAuthenticationError(int code, CharSequence msg) {
                        boolean userLeft = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED
                                || code == BiometricPrompt.BIOMETRIC_ERROR_TIMEOUT;
                        act.call(callbackName, userLeft ? "\"cancel\"" : code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED ? "\"away\"" : act.live() ? "\"refused\"" : "\"unavailable\"");
                    }
                });
            });
        }

    void confirmSigner(String callbackName, String subtitle) {
            if (signerWindowSeconds() > 0 && signerOpen()) { act.call(callbackName, "\"ok\""); return; } // inside the fingerprint window
            act.main.post(() -> {
                BiometricManager bm = act.getSystemService(BiometricManager.class);
                if (bm == null || bm.canAuthenticate(AUTHENTICATORS) != BiometricManager.BIOMETRIC_SUCCESS) {
                    act.call(callbackName, "\"unavailable\"");
                    return;
                }
                // The one operation this prompt authorises: it opens the signer data key, and nothing else can. The same
                // prompt also starts the safe key's SAFE_SECONDS, so the safe store never depends on this cipher: when a new
                // fingerprint killed the signer key, the dead alias goes (forgetSigner, everyday keys only), a fresh signer
                // key is made for the prompt, and if even that cannot be built while safe keys are kept here, a plain prompt
                // (no CryptoObject) still lets "Show my sheets" and a paperless take-out through. One prompt either way.
                Cipher cipher;
                try {
                    cipher = signerCipher();
                } catch (KeyPermanentlyInvalidatedException e) {
                    forgetSigner(); act.notice = SIGNER_ENROL_NOTICE; act.pushNotice();
                    try { cipher = signerCipher(); } catch (Exception e2) { cipher = null; }
                } catch (Exception e) {
                    cipher = null;
                }
                if (cipher == null && !hasAnyCold()) {
                    act.call(callbackName, "\"refused\"");
                    return;
                }
                final Cipher shown = cipher;
                lastPromptFinger = false; // set again by this prompt's answer
                lastPromptUnknown = false;
                BiometricPrompt prompt = new BiometricPrompt.Builder(act)
                        .setTitle("KaspaVault").setSubtitle(subtitle)
                        .setConfirmationRequired(false)
                        .setAllowedAuthenticators(AUTHENTICATORS).build();
                BiometricPrompt.AuthenticationCallback answer = new BiometricPrompt.AuthenticationCallback() {
                    @Override public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                        // Only a NAMED PIN is refused here. FINGER or UNKNOWN (some phones never say): the Keystore decides,
                        // since safe.v2 gives a PIN answer no token. lastPromptFinger() still needs a named finger on v1.
                        int type = r.getAuthenticationType();
                        lastPromptFinger = type != BiometricPrompt.AUTHENTICATION_RESULT_TYPE_DEVICE_CREDENTIAL;
                        lastPromptUnknown = lastPromptFinger && type != BiometricPrompt.AUTHENTICATION_RESULT_TYPE_BIOMETRIC;
                        if (lastPromptFinger) fingerSeen(); // counts the prompt, makes the canary the first time
                        try {
                            if (shown == null) { // plain prompt: only the safe store (FRESH_PROMPT_METHODS) may follow
                                synchronized (act.bridge) {
                                    shutSigner();
                                    plainPromptAt = android.os.SystemClock.elapsedRealtime();
                                    freshUnused = true; // ONE irreversible move may follow
                                }
                                migrateSafeV2(); // both safe keys inside SAFE_SECONDS: v1 -> v2 (written, re-read, proven), finger only
                                act.call(callbackName, "\"ok\"");
                                return;
                            }
                            BiometricPrompt.CryptoObject co = r.getCryptoObject();
                            openSignerWith(co != null && co.getCipher() != null ? co.getCipher() : shown);
                            migrateSafeV2(); // v1 -> v2 first (written, re-read, proven, then v1 goes), finger only
                            migrateCold(); // both keys open right now: move old signer.cold* into safe.v2 (verified, never lossy), finger only
                            act.call(callbackName, "\"ok\"");
                        } catch (KeyPermanentlyInvalidatedException e) {
                            forgetSigner(); act.notice = SIGNER_ENROL_NOTICE; act.pushNotice();
                            act.call(callbackName, "\"refused\"");
                        } catch (Exception e) {
                            act.call(callbackName, "\"refused\"");
                        }
                    }
                    @Override public void onAuthenticationError(int code, CharSequence msg) {
                        boolean userLeft = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED
                                || code == BiometricPrompt.BIOMETRIC_ERROR_TIMEOUT;
                        act.call(callbackName, userLeft ? "\"cancel\"" : code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED ? "\"away\"" : "\"refused\"");
                    }
                };
                if (shown != null) prompt.authenticate(new BiometricPrompt.CryptoObject(shown), new CancellationSignal(), act.getMainExecutor(), answer);
                else prompt.authenticate(new CancellationSignal(), act.getMainExecutor(), answer);
            });
        }
}
