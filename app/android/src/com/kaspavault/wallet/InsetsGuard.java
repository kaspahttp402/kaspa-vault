package com.kaspavault.wallet;

import android.content.Intent;
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


/** Edge-to-edge and keyboard inset handling, and the GuardService on/off helpers, moved out of MainActivity unchanged. */
final class InsetsGuard {
    private final MainActivity act;
    private String insetsJs = "";
    InsetsGuard(MainActivity a) { act = a; }

    /** Pref (Java only, not in SETTINGS): the user wants the guard on; GuardRestart and the app's start bring it back. */
    static final String GUARD_WANTED = "guard_wanted";

    void pushInsets() { if (!insetsJs.isEmpty()) act.web.evaluateJavascript(insetsJs, null); }

    android.widget.FrameLayout frame() {
        final WebView web = act.web;
        // Edge-to-edge (setDecorFitsSystemWindows false) means adjustResize never shrinks the WebView under the keyboard.
        // So the frame around the WebView takes the keyboard's height as bottom padding: the whole page shifts up and the
        // browser engine keeps the focused field in view itself, so the screen only needs to shift up when it appears.
        // The page still pads the system bars; while the keyboard is up the bottom bar sits under it, so it gets 0 then.
        android.widget.FrameLayout frame = new android.widget.FrameLayout(act);
        frame.addView(web, new android.widget.FrameLayout.LayoutParams(android.view.ViewGroup.LayoutParams.MATCH_PARENT, android.view.ViewGroup.LayoutParams.MATCH_PARENT));
        frame.setOnApplyWindowInsetsListener((v, ins) -> {
            Insets bars = ins.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            int imePx = ins.isVisible(WindowInsets.Type.ime()) ? ins.getInsets(WindowInsets.Type.ime()).bottom : 0;
            v.setPadding(0, 0, 0, imePx);
            float d = act.getResources().getDisplayMetrics().density;
            int bottom = imePx > 0 ? 0 : bars.bottom;
            insetsJs = "window.KV_insets&&KV_insets(" + Math.round(bars.top / d) + "," + Math.round(bars.right / d) + ","
                    + Math.round(bottom / d) + "," + Math.round(bars.left / d) + ",0)";
            pushInsets();
            return WindowInsets.CONSUMED;
        });
        return frame;
    }

    void guardOn(boolean on) {
        // commit, not apply: a process killed right after the tap must not lose the user's choice
        act.prefs.edit().putString(GUARD_WANTED, on ? "1" : "").commit();
        act.main.post(() -> {
            Intent svc = new Intent(act, GuardService.class);
            if (!on) { act.stopService(svc); return; }
            askNotifications();
            askBatteryExemption();
            try { act.startForegroundService(svc); }
            catch (Exception e) { android.util.Log.w("KaspaVault", "guard start refused: " + e); }
        });
    }

    /** API 33+ permission for the guard's alerts, asked once (answered to KV_notifications). */
    private void askNotifications() {
        if (act.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)
                == android.content.pm.PackageManager.PERMISSION_GRANTED) return;
        if ("1".equals(act.prefs.getString("guard_notif_asked", ""))) return;
        act.prefs.edit().putString("guard_notif_asked", "1").apply();
        act.requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS}, 8);
    }

    /** Opens the phone's battery-saving list once (no exemption asked: the guard alarm arrives in
     *  deep Doze without it, and the direct ask is a Play-policy risk). */
    private void askBatteryExemption() {
        if ("1".equals(act.prefs.getString("guard_battery_asked", ""))) return;
        act.prefs.edit().putString("guard_battery_asked", "1").apply();
        android.os.PowerManager pm = act.getSystemService(android.os.PowerManager.class);
        if (pm == null || pm.isIgnoringBatteryOptimizations(act.getPackageName())) return;
        try {
            act.startActivity(new Intent(
                    android.provider.Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS));
        } catch (Exception e) {
            android.util.Log.w("KaspaVault", "battery settings refused: " + e);
        }
    }

    boolean guardRunning() { return GuardService.RUNNING; }
}
