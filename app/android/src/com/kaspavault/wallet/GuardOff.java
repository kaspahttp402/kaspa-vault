package com.kaspavault.wallet;

import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.CancellationSignal;
import android.util.Log;

/**
 * The only way to turn the guard off (Bridge.guardOff): a prompt of its own, shown for this one
 * act, with a title and subtitle written here (never taken from the page). Only
 * onAuthenticationSucceeded stops GuardService and clears guard_wanted; an earlier prompt (a
 * payment, an unlock) never counts, and this one opens nothing else. Answers
 * window[callbackName]("ok" | "cancel" | "refused") through act.call.
 */
final class GuardOff {
    private static final String TAG = "KaspaVault";
    static final String TITLE = "Turn off alerts on this phone?";
    static final String SUBTITLE = "This phone stops watching until you turn alerts back on.";
    /** A fingerprint (strong) only, never the PIN (PIN gap, Engine 16 decision); no CryptoObject: the gesture is the gate. */
    static final int AUTHENTICATORS = BiometricManager.Authenticators.BIOMETRIC_STRONG;

    private final MainActivity act;

    GuardOff(MainActivity a) { act = a; }

    /**
     * Shows the prompt on the main thread; "refused" at once with no fingerprint enrolled, or while the fingerprints-
     * changed wait runs (a notice says how long). Called on the bridge thread: the wait check may ask the network.
     */
    void ask(String callbackName) {
        long waitHours = act.signer.fingerWaitHours();
        if (waitHours != 0) {
            Log.i(TAG, "guardOff refused: fingerprints changed or unreadable, waiting");
            act.notice = waitHours < 0 ? Signer.FINGER_AGAIN : Signer.guardWait(waitHours);
            act.pushNotice();
            act.call(callbackName, "\"refused\"");
            return;
        }
        act.main.post(() -> {
            BiometricManager bm = act.getSystemService(BiometricManager.class);
            int can = bm == null ? -1 : bm.canAuthenticate(AUTHENTICATORS);
            if (can != BiometricManager.BIOMETRIC_SUCCESS) {
                Log.i(TAG, "guardOff refused: no fingerprint on this phone");
                act.call(callbackName, "\"refused\"");
                return;
            }
            BiometricPrompt prompt = new BiometricPrompt.Builder(act)
                    .setTitle(TITLE).setSubtitle(SUBTITLE)
                    .setNegativeButton("Cancel", act.getMainExecutor(), (d, w) -> act.call(callbackName, "\"cancel\""))
                    .setConfirmationRequired(false)
                    .setAllowedAuthenticators(AUTHENTICATORS).build();
            prompt.authenticate(new CancellationSignal(), act.getMainExecutor(),
                    answer(callbackName));
        });
    }

    /**
     * The user leaving (back, timeout) or the phone taking the prompt down (app put away) is
     * "cancel"; lockout, sensor errors and the rest are "refused". The guard stays on in both.
     */
    private BiometricPrompt.AuthenticationCallback answer(String callbackName) {
        return new BiometricPrompt.AuthenticationCallback() {
            @Override
            public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                if (Signer.canaryState(act.prefs) >= 2) { act.call(callbackName, "\"refused\""); return; } // changed meanwhile
                act.signer.fingerSeen(); // a finger prompt: makes the canary the first time
                act.insets.guardOn(false);
                Log.i(TAG, "guard turned off after its own prompt");
                act.call(callbackName, "\"ok\"");
            }
            @Override
            public void onAuthenticationError(int code, CharSequence msg) {
                boolean userLeft = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED
                        || code == BiometricPrompt.BIOMETRIC_ERROR_TIMEOUT
                        || code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED;
                act.call(callbackName, userLeft ? "\"cancel\"" : "\"refused\"");
            }
        };
    }
}
