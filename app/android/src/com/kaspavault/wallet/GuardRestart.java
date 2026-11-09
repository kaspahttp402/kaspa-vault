package com.kaspavault.wallet;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

/**
 * Brings the guard back after the phone restarts or the app is updated, when the user had it on ("guard_wanted",
 * written by InsetsGuard.guardOn; never page-writable). Both actions are protected broadcasts: only the system can
 * send them, so another app cannot start or stop anything here. Starting a specialUse foreground service from these
 * two broadcasts is allowed on Android 12+.
 */
public class GuardRestart extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        String a = intent == null ? null : intent.getAction();
        if (!Intent.ACTION_BOOT_COMPLETED.equals(a) && !Intent.ACTION_MY_PACKAGE_REPLACED.equals(a)) return;
        if (!"1".equals(context.getSharedPreferences("kaspavault", Context.MODE_PRIVATE).getString(InsetsGuard.GUARD_WANTED, ""))) return;
        try {
            context.startForegroundService(new Intent(context, GuardService.class));
            Log.i("KaspaVault", "guard restarted after " + a);
        } catch (Exception e) {
            Log.w("KaspaVault", "guard restart refused: " + e);
        }
    }
}
