package com.kaspavault.wallet;

import android.util.Log;

/**
 * The Rust wallet engine (libvault_core.so), loaded once. An APK built without the library (a laptop-bridge build)
 * keeps working: AVAILABLE is false and Bridge.vault answers status -4 ("the wallet engine is not ready").
 * Every native answers the envelope string {"status":<int>,"body":<json>} except keysNew, which returns raw bytes.
 * Never call these on the UI thread: each one blocks on the engine's tokio runtime (MainActivity.vaultThread).
 */
final class VaultLib {
    static final boolean AVAILABLE;

    static {
        boolean ok;
        try {
            System.loadLibrary("vault_core");
            ok = true;
        } catch (UnsatisfiedLinkError e) {
            Log.w("KaspaVault", "no wallet engine in this build (libvault_core.so missing): " + e.getMessage());
            ok = false;
        }
        AVAILABLE = ok;
    }

    private VaultLib() { }

    /** JNI symbol Java_com_kaspavault_wallet_VaultLib_init. Opens the state dir for the network and starts connecting. Envelope string. */
    static native String init(String dir, String network);

    /** JNI symbol Java_com_kaspavault_wallet_VaultLib_call. Runs one keyless method (status, quote, ...). Envelope string. */
    static native String call(String method, String argsJson);

    /** JNI symbol Java_com_kaspavault_wallet_VaultLib_callWithSecret. A signing method with its 32- or 64-byte secret. Envelope string. */
    static native String callWithSecret(String method, String argsJson, byte[] secret);

    /** JNI symbol Java_com_kaspavault_wallet_VaultLib_keysNew. Makes the three keys; 96 raw bytes, never a string. */
    static native byte[] keysNew(String argsJson);
}
