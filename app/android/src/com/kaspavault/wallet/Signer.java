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
    /** The only Keystore-wrapped secrets: the laptop's token and the SHA-256 of its certificate (the pin). */
    static final Set<String> SECRETS = new HashSet<>(Arrays.asList("live_token", "live_pin"));
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
    /** How long after an in-app fingerprint the page may still read (once per secret) or write the stored secrets. Dial. */
    static final int UNLOCK_SECONDS = 120;
    static final String KEY_ALIAS = "kaspavault.secrets.v3"; /* v3: one use per in-app fingerprint or PIN prompt, never the phone's own unlock */
    /**
     * Older keys: v1 was fingerprint-only, v2 stayed open for 30 s after any phone unlock.
     * A phone that still has one forgets its secrets once and pairs again.
     */
    static final String KEY_ALIAS_V1 = "kaspavault.secrets", KEY_ALIAS_V2 = "kaspavault.secrets.v2";
    /** Why the phone must pair again, in the page's words: after this update, or after the phone's fingerprints changed. */
    static final String REPAIR_NOTICE = "This update changed how your phone keeps its laptop key. Pair your phone with your laptop again from the code the laptop shows.";
    static final String ENROL_NOTICE = "Your phone's fingerprints changed, so the laptop key kept on this phone was cleared. Pair your phone with your laptop again from the code the laptop shows.";

    /**
     * The wrapping key: AES in the phone's secure hardware that works for ONE operation per in-app fingerprint or PIN
     * prompt (timeout 0), and only through the cipher that prompt was shown for (BiometricPrompt.CryptoObject). The
     * phone's own unlock never opens it, and neither does a prompt the page did not just show. It wraps a random data key
     * that encrypts the stored secrets; after a prompt the data key stays in memory for UNLOCK_SECONDS and each secret may
     * be read once. A new fingerprint enrolment kills the wrapping key, and with it every secret.
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
    /** The data key while open (null when shut), when it was opened, and which secrets the page may still read this time. All under the bridge lock. */
    private byte[] dataKey = null;
    private long unlockedAt = 0;
    private final Set<String> readable = new HashSet<>();

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
            unlockedAt = android.os.SystemClock.elapsedRealtime();
            readable.addAll(SECRETS);
            if (act.livePin.isEmpty()) act.livePin = open("live_pin");
        }
    }

    /** True while the data key is open; shuts it once UNLOCK_SECONDS have passed. */
    boolean unlocked() {
        synchronized (act.bridge) {
            if (dataKey != null && android.os.SystemClock.elapsedRealtime() - unlockedAt > UNLOCK_SECONDS * 1000L) shut();
            return dataKey != null;
        }
    }

    void shut() {
        synchronized (act.bridge) {
            if (dataKey != null) Arrays.fill(dataKey, (byte) 0);
            dataKey = null;
            readable.clear();
        }
    }

    /** One stored secret, decrypted with the open data key; "" when there is none or the store is shut. Not a page read. */
    String open(String key) {
        synchronized (act.bridge) {
            if (!unlocked()) return "";
            String v = act.prefs.getString("secret." + key, "");
            int at = v.indexOf(':');
            if (at < 0) return "";
            try {
                Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
                c.init(Cipher.DECRYPT_MODE, new SecretKeySpec(dataKey, "AES"), new GCMParameterSpec(128, Base64.decode(v.substring(0, at), Base64.NO_WRAP)));
                return new String(c.doFinal(Base64.decode(v.substring(at + 1), Base64.NO_WRAP)), StandardCharsets.UTF_8);
            } catch (Exception e) {
                return "";
            }
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
        synchronized (act.bridge) { shut(); act.livePin = ""; act.pendingPin = ""; act.pendingHost = ""; }
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
    // Nothing here is a @JavascriptInterface; getSecret keeps refusing every "signer.*" name to the page.
    // ---------------------------------------------------------------------------------------------------------------
    static final String SIGNER_ALIAS = "kaspavault.signer.v1";
    static final String SIGNER_WRAPPED_KEY = "signer.wrapped_key";
    /** The three slots, as pref suffixes: signer.card, signer.stop, signer.funding. */
    static final Set<String> SIGNER_SLOTS = new HashSet<>(Arrays.asList("card", "stop", "funding", "cold", "cold2", "cold3"));
    static final String SIGNER_ENROL_NOTICE = "Your phone's fingerprints changed, so the card key, the emergency stop key and the funding key kept on this phone were cleared. The safe sheets kept on this phone are still there, and nothing in the safe changed. Stop this account and open a new one to keep using this phone.";
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
    /** When a plain prompt (no signer cipher could be built) said yes; 0 when none. Under the bridge lock. */
    private long plainPromptAt = 0;

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
        String w = act.prefs.getString(SAFE_WRAPPED_KEY, "");
        int at = w.indexOf(':');
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        if (at > 0) {
            c.init(Cipher.DECRYPT_MODE, safeKey(), new GCMParameterSpec(128, Base64.decode(w.substring(0, at), Base64.NO_WRAP)));
            return c.doFinal(Base64.decode(w.substring(at + 1), Base64.NO_WRAP));
        }
        if (!create) throw new IllegalStateException("no safe data key");
        c.init(Cipher.ENCRYPT_MODE, safeKey());
        byte[] k = new byte[32];
        new SecureRandom().nextBytes(k);
        try {
            byte[] ct = c.doFinal(k);
            if (!act.prefs.edit().putString(SAFE_WRAPPED_KEY, Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP)).commit())
                throw new IllegalStateException("safe data key not stored");
        } catch (Exception x) {
            Arrays.fill(k, (byte) 0);
            throw x;
        }
        return k;
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
     * keysNew: the safe keys sealed under the safe data key, in order, or null when the safe key cannot be used now
     * (its SAFE_SECONDS since the prompt ran out): keysNew then seals them in the old signer store and migrateCold()
     * moves them at the next prompt. The inputs are not zero-filled here (keysNew does that).
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
            try {
                for (int i = 0; i < names.length; i++) {
                    String v = act.prefs.getString("safe." + names[i], "");
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
            if (signerDataKey == null) return;
            boolean legacy = false;
            for (String n : COLD_SLOTS) {
                if (act.prefs.getString("safe." + n, "").indexOf(':') > 0) return;
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
                SharedPreferences.Editor ed = act.prefs.edit();
                for (int i = 0; i < COLD_SLOTS.length; i++) {
                    if (sealed[i] == null) continue;
                    byte[] back = openWith(dk2, sealed[i]);
                    boolean same = java.security.MessageDigest.isEqual(back, plain[i]);
                    Arrays.fill(back, (byte) 0);
                    if (!same) throw new IllegalStateException("safe copy differs");
                    ed.putString("safe." + COLD_SLOTS[i], sealed[i]).remove("signer." + COLD_SLOTS[i]);
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
    void burnCold() {
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String n : COLD_SLOTS) ed.remove("safe." + n).remove("signer." + n);
        ed.commit();
    }

    /** keys.reset (engine ok): the old safe goes too, alias and every "safe." pref, so it never shadows the next set. */
    void forgetSafe() {
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            ks.deleteEntry(SAFE_ALIAS);
        } catch (Exception ignored) { }
        SharedPreferences.Editor ed = act.prefs.edit();
        for (String k : act.prefs.getAll().keySet()) if (k.startsWith("safe.")) ed.remove(k);
        ed.apply();
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
            act.prefs.edit().putString(SIGNER_WRAPPED_KEY, Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP)).apply();
        }
        synchronized (act.bridge) {
            shutSigner();
            signerDataKey = k;
            signerOpenedAt = android.os.SystemClock.elapsedRealtime();
            signerReadable.addAll(SIGNER_SLOTS);
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

    /** True only within FRESH_PROMPT_MS of a real prompt (the window shortcut never moves signerOpenedAt); a plain prompt (no signer cipher) counts too. */
    boolean promptedRecently() {
        synchronized (act.bridge) {
            long now = android.os.SystemClock.elapsedRealtime();
            return (signerDataKey != null && now - signerOpenedAt <= FRESH_PROMPT_MS) || (plainPromptAt > 0 && now - plainPromptAt <= FRESH_PROMPT_MS);
        }
    }

    /** Drops the signer data key: called the moment vault() has the bytes for the engine, or keysNew has sealed them. */
    void shutSigner() {
        synchronized (act.bridge) {
            if (signerDataKey != null) Arrays.fill(signerDataKey, (byte) 0);
            signerDataKey = null;
            signerReadable.clear();
            plainPromptAt = 0;
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
    boolean hasSlot(String name) { return act.prefs.getString("signer." + name, "").indexOf(':') > 0 || act.prefs.getString("safe." + name, "").indexOf(':') > 0; }

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

    boolean setSecret(String key, String value) {
            if (!SECRETS.contains(key)) return false;
            String k = "secret." + key;
            if (value == null || value.isEmpty()) {
                if ("live_pin".equals(key)) { act.pendingPin = ""; act.pendingHost = ""; act.livePin = ""; }
                act.prefs.edit().remove(k).apply();
                return true;
            }
            if (!unlocked()) return false;
            try {
                Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
                c.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(dataKey, "AES"));
                byte[] ct = c.doFinal(value.getBytes(StandardCharsets.UTF_8));
                act.prefs.edit().putString(k, Base64.encodeToString(c.getIV(), Base64.NO_WRAP) + ":" + Base64.encodeToString(ct, Base64.NO_WRAP)).apply();
            } catch (Exception e) {
                return false;
            }
            if ("live_pin".equals(key)) { act.pendingPin = ""; act.pendingHost = ""; act.livePin = value; } // decided: the stored pin rules from here
            return true;
        }

    String getSecret(String key) {
            if (!SECRETS.contains(key) || !unlocked() || !readable.remove(key)) return "";
            return open(key);
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
                BiometricPrompt prompt = new BiometricPrompt.Builder(act)
                        .setTitle("KaspaVault").setSubtitle(subtitle)
                        .setConfirmationRequired(false)
                        .setAllowedAuthenticators(AUTHENTICATORS).build();
                BiometricPrompt.AuthenticationCallback answer = new BiometricPrompt.AuthenticationCallback() {
                    @Override public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                        try {
                            if (shown == null) { // plain prompt: only the safe store (FRESH_PROMPT_METHODS) may follow
                                synchronized (act.bridge) {
                                    shutSigner();
                                    plainPromptAt = android.os.SystemClock.elapsedRealtime();
                                }
                                act.call(callbackName, "\"ok\"");
                                return;
                            }
                            BiometricPrompt.CryptoObject co = r.getCryptoObject();
                            openSignerWith(co != null && co.getCipher() != null ? co.getCipher() : shown);
                            migrateCold(); // both keys open right now: move old signer.cold* into the safe store (verified, never lossy)
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
