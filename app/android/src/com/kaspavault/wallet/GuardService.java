package com.kaspavault.wallet;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.os.BatteryManager;
import android.os.Build;
import android.os.IBinder;
import android.os.PowerManager;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The guard inside the app: a foreground service that keeps the Rust watcher running all day and shows
 * it as a notification. It calls the engine itself (init is idempotent; START_STICKY may restart this process with no
 * Activity), asks the engine to start the guard with the second node from the "guard_nodes" dial, then its own thread sits
 * on guard.wait and turns every new alert into a loud notification. The persistent notification reads
 * "Guard on. Last heard from the network N s ago". Tapping either opens the app. Without the engine library the service
 * stops itself at once. Nothing here opens a secret: the guard only reads the chain.
 */
public final class GuardService extends Service {
    private static final String TAG = "KaspaVault";
    /** Set while the guard runs (the engine accepted guard.start); the page reads it through Bridge.guardRunning(). */
    static volatile boolean RUNNING = false;

    private static final String CH_GUARD = "guard", CH_ALERTS = "guard_alarms_private";
    private static final int ID_GUARD = 1, ID_REFUSED = 2, ID_ALERT_BASE = 100;
    private static final int WAIT_MS = 30000;
    private static final int POLL_AWAKE = 3, POLL_BATTERY = 15;
    /** The newest alert time already shown, kept so a restart of the service does not alarm twice. */
    private static final String LAST_ALERT_PREF = "guard_last_alert_at";
    /** Watched accounts (this phone is someone else's guardian or heir): notification ids sit above 1000, two per account. */
    private static final int ID_WATCH_BASE = 1000;
    private static final long WATCH_EVERY_MS = 25000;
    /** Per watched id: the last waiting withdrawal already shown ("id|to|kas"), and whether "can be passed to you" was shown. */
    private static final String WATCH_PENDING_PREF = "watch_pending_", WATCH_HEIR_PREF = "watch_heir_due_";
    /** Alarm title and text are cut to these many characters (printable). Dials. */
    private static final int TITLE_MAX = 80;
    static final int TEXT_MAX = 400;
    /** All a locked screen shows of an alarm: no amount, no address, no watched name. */
    private static final String LOCKED_TEXT = "Alert. Unlock to check your account.";

    private final ExecutorService guardThread = Executors.newSingleThreadExecutor();   // sits on guard.wait
    private final ExecutorService controlThread = Executors.newSingleThreadExecutor(); // guard.start / guard.poll
    private SharedPreferences prefs;
    private NotificationManager notifications;
    private volatile boolean looping = false;
    private int currentPoll = -1;
    private int alertCount = 0;
    private long lastWatchCheck = 0;
    private boolean watchErrorLogged = false;

    private final BroadcastReceiver powerReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context c, Intent i) { applyPoll(); }
    };

    @Override public void onCreate() {
        super.onCreate();
        prefs = getSharedPreferences("kaspavault", Context.MODE_PRIVATE);
        notifications = getSystemService(NotificationManager.class);
        NotificationChannel quiet = new NotificationChannel(CH_GUARD, "Alerts on this phone",
                NotificationManager.IMPORTANCE_LOW);
        quiet.setDescription("Shows that this phone is watching your account.");
        NotificationChannel loud = new NotificationChannel(CH_ALERTS, "Alerts",
                NotificationManager.IMPORTANCE_HIGH);
        loud.setDescription("Sounds when money leaves your account or this phone loses touch.");
        // hidden on a secure lock screen whatever the phone's own setting (a channel keeps the
        // visibility it was first made with, so this reaches new installs; the notification's own
        // VISIBILITY_PRIVATE covers phones that hide sensitive content)
        loud.setLockscreenVisibility(Notification.VISIBILITY_PRIVATE);
        notifications.createNotificationChannel(quiet);
        notifications.createNotificationChannel(loud);
        // Android never updates an existing channel: the old public one is replaced by the private one above.
        notifications.deleteNotificationChannel("guard_alerts");
        IntentFilter f = new IntentFilter();
        f.addAction(Intent.ACTION_POWER_CONNECTED);
        f.addAction(Intent.ACTION_POWER_DISCONNECTED);
        f.addAction(Intent.ACTION_SCREEN_ON);
        f.addAction(Intent.ACTION_SCREEN_OFF);
        registerReceiver(powerReceiver, f);
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (!VaultLib.AVAILABLE) { Log.w(TAG, "guard: no wallet engine in this build, stopping"); stopSelf(); return START_NOT_STICKY; }
        // The foreground promise comes first (Android's deadline), then the engine work on our own threads.
        Notification first = guardNotification("Alerts starting. Reaching the network...");
        if (Build.VERSION.SDK_INT >= 34) startForeground(ID_GUARD, first, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        else startForeground(ID_GUARD, first); // API 33 has no specialUse type: the manifest's type applies
        if (!looping) {
            looping = true;
            controlThread.execute(this::startGuard);
        }
        return START_STICKY;
    }

    /** init (idempotent) then guard.start; a refusal is shown with the engine's own words and the service stops. */
    private void startGuard() {
        try {
            String network = prefs.getString("live_network", "");
            if (network.isEmpty()) network = "mainnet"; // live by default; "testnet" only when the setting says so
            String dir = getFilesDir() + "/vault/" + network;
            String initAnswer = VaultLib.init(dir, network);
            Log.i(TAG, "guard: engine init (" + network + ") -> " + brief(initAnswer));

            JSONArray nodes = new JSONArray();
            for (String n : prefs.getString("guard_nodes", "").split(",")) { String t = n.trim(); if (!t.isEmpty()) nodes.put(t); }
            JSONObject args = new JSONObject();
            args.put("nodes", nodes);
            args.put("auto_heir", "1".equals(prefs.getString("auto_heir", "")));
            currentPoll = wantedPoll();
            args.put("poll_seconds", currentPoll);
            String answer = VaultLib.call("guard.start", args.toString());
            Log.i(TAG, "guard.start -> " + brief(answer));
            JSONObject env = new JSONObject(answer);
            int status = env.optInt("status", 0);
            JSONObject body = env.optJSONObject("body");
            String error = body == null ? null : body.optString("error", "");
            if (status == 503) {
                refuse(error == null || error.isEmpty()
                        ? "Alerts can't reach the network. Open KaspaVault and try again." : error);
                return;
            }
            if (status != 200 || body == null || !body.optBoolean("ok", false)) {
                refuse(error == null || error.isEmpty()
                        ? "Alerts could not start. Open KaspaVault and try again." : error);
                return;
            }
            RUNNING = true;
            notifications.notify(ID_GUARD, guardNotification("Alerts on. Reaching the network..."));
            guardThread.execute(this::waitLoop);
        } catch (Throwable t) {
            Log.w(TAG, "guard.start failed: " + t);
            refuse("Alerts could not start. Open KaspaVault and try again.");
        }
    }

    /** The engine refused (fewer than two nodes, no network, ...): the words stay as a loud notification, the service ends. */
    private void refuse(String text) {
        RUNNING = false;
        looping = false;
        notifications.notify(ID_REFUSED, alertNotification("Alerts off", text));
        stopSelf();
    }

    /** Sits on guard.wait (30 s or an alert), refreshes the persistent notification, alarms on every alert newer than the last shown. */
    private void waitLoop() {
        long lastShown = prefs.getLong(LAST_ALERT_PREF, 0);
        while (looping) {
            try {
                String answer = VaultLib.call("guard.wait", "{\"timeout_ms\":" + WAIT_MS + "}");
                if (!looping) break;
                JSONObject env = new JSONObject(answer);
                int status = env.optInt("status", 0);
                JSONObject body = env.optJSONObject("body");
                if (status == 503 || body == null) {
                    notifications.notify(ID_GUARD,
                            guardNotification("Alerts on. Waiting for the network..."));
                    Thread.sleep(3000);
                    continue;
                }
                long now = System.currentTimeMillis() / 1000;
                String heard = body.isNull("seen_at") ? "Nothing heard from the network yet."
                        : "Last heard from the network " + Math.max(0, now - body.optLong("seen_at", now)) + " s ago";
                boolean running = body.optBoolean("running", false);
                String head = running ? "Alerts on. " : "Alerts stopped. ";
                notifications.notify(ID_GUARD, guardNotification(head + heard));

                JSONArray alerts = body.optJSONArray("alerts");
                long newest = lastShown;
                if (alerts != null) for (int i = 0; i < alerts.length(); i++) {
                    JSONObject a = alerts.optJSONObject(i);
                    if (a == null) continue;
                    long at = a.optLong("at", 0);
                    if (at <= lastShown) continue;
                    String text = a.optString("text", "");
                    if (text.isEmpty()) text = "Caught by your alerts. Open KaspaVault.";
                    notifications.notify(ID_ALERT_BASE + (alertCount++ % 50),
                            alertNotification("Caught by your alerts", text));
                    if (at > newest) newest = at;
                }
                if (newest != lastShown) { lastShown = newest; prefs.edit().putLong(LAST_ALERT_PREF, lastShown).apply(); }
                checkWatched();

                if (!running) {
                    String exited = body.isNull("exited") ? "" : body.optString("exited", "");
                    refuse(exited.isEmpty()
                            ? "Alerts stopped. Open KaspaVault to start them again." : exited);
                    return;
                }
            } catch (InterruptedException e) {
                return;
            } catch (Throwable t) {
                Log.w(TAG, "guard.wait failed: " + t);
                try { Thread.sleep(3000); } catch (InterruptedException e) { return; }
            }
        }
    }

    /**
     * Accounts this phone watches for other people (guardian and/or heir): at most every 25 s, watch.list then watch.status
     * per id. A new waiting withdrawal (id + to + kas, kept in prefs so a restart does not repeat it) and heir_due turning
     * true each raise one alarm. Any failure here (an engine without watch.*, no network) is logged once and otherwise
     * ignored, so it never disturbs the own-account guard above.
     */
    private void checkWatched() {
        long nowMs = System.currentTimeMillis();
        if (nowMs - lastWatchCheck < WATCH_EVERY_MS) return;
        lastWatchCheck = nowMs;
        try {
            JSONObject list = watchBody("watch.list", "{}");
            if (list == null) return;
            JSONArray watched = list.optJSONArray("watched");
            if (watched == null) return;
            for (int i = 0; i < watched.length(); i++) {
                JSONObject w = watched.optJSONObject(i);
                if (w == null) continue;
                String id = w.optString("id", "");
                if (id.isEmpty()) continue;
                JSONObject args = new JSONObject();
                args.put("id", id);
                JSONObject st = watchBody("watch.status", args.toString());
                if (st == null) continue;
                String name = st.optString("name", "");
                if (name.isEmpty()) name = w.optString("name", "");
                if (name.isEmpty()) name = "A watched";
                JSONObject roles = st.optJSONObject("roles");
                if (roles == null) roles = w.optJSONObject("roles");
                boolean guardian = roles != null && roles.has("guardian") && !roles.isNull("guardian");
                int base = ID_WATCH_BASE + (id.hashCode() & 0x3fff) * 2;

                JSONObject pending = st.optJSONObject("pending");
                if (pending != null) {
                    String kas = kasText(pending.opt("kas"));
                    String key = id + "|" + pending.optString("to", "") + "|" + kas;
                    if (!key.equals(prefs.getString(WATCH_PENDING_PREF + id, ""))) {
                        String text = "A withdrawal of " + kas + " KAS is waiting." + (guardian ? " Open the wallet to stop it." : "");
                        notifications.notify(base, alertNotification(name + "'s account", text));
                        prefs.edit().putString(WATCH_PENDING_PREF + id, key).apply();
                    }
                }

                boolean due = st.optBoolean("heir_due", false);
                boolean shown = prefs.getBoolean(WATCH_HEIR_PREF + id, false);
                if (due && !shown) {
                    notifications.notify(base + 1, alertNotification(name + "'s account",
                            name + "'s account can now be passed to you. Open the wallet to claim it."));
                    prefs.edit().putBoolean(WATCH_HEIR_PREF + id, true).apply();
                } else if (!due && shown) {
                    prefs.edit().putBoolean(WATCH_HEIR_PREF + id, false).apply();
                }
            }
        } catch (Throwable t) {
            watchFailed("watch check failed: " + t);
        }
    }

    /** One watch.* call: the body when the envelope is 200 and ok, else null (logged once, never thrown). */
    private JSONObject watchBody(String method, String args) {
        try {
            String answer = VaultLib.call(method, args);
            JSONObject env = new JSONObject(answer);
            JSONObject body = env.optJSONObject("body");
            if (env.optInt("status", 0) == 200 && body != null && body.optBoolean("ok", false)) return body;
            watchFailed(method + " -> " + brief(answer));
        } catch (Throwable t) {
            watchFailed(method + " failed: " + t);
        }
        return null;
    }

    private void watchFailed(String what) {
        if (watchErrorLogged) return;
        watchErrorLogged = true;
        Log.w(TAG, "guard: " + what + " (further watch errors are not logged)");
    }

    /** A KAS amount as plain text with no trailing zeros, whether the engine sent a number or a string. */
    private static String kasText(Object kas) {
        if (kas == null || kas == JSONObject.NULL) return "?";
        try { return new java.math.BigDecimal(String.valueOf(kas)).stripTrailingZeros().toPlainString(); }
        catch (Exception e) { return String.valueOf(kas); }
    }

    /** 3 s while charging or the screen is on, 15 s on battery with the screen off (the design's two speeds). */
    private int wantedPoll() {
        BatteryManager bm = getSystemService(BatteryManager.class);
        PowerManager pm = getSystemService(PowerManager.class);
        boolean charging = bm != null && bm.isCharging();
        boolean screenOn = pm != null && pm.isInteractive();
        return (charging || screenOn) ? POLL_AWAKE : POLL_BATTERY;
    }

    private void applyPoll() {
        if (!RUNNING) return;
        int want = wantedPoll();
        if (want == currentPoll) return;
        currentPoll = want;
        controlThread.execute(() -> {
            try { Log.i(TAG, "guard.poll " + want + " -> " + brief(VaultLib.call("guard.poll", "{\"seconds\":" + want + "}"))); }
            catch (Throwable t) { Log.w(TAG, "guard.poll failed: " + t); }
        });
    }

    private PendingIntent openApp() {
        Intent open = new Intent(this, MainActivity.class).setFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        return PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification guardNotification(String text) {
        return new Notification.Builder(this, CH_GUARD)
                .setSmallIcon(R.drawable.icon_mono)
                .setContentTitle("KaspaVault alerts")
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setContentIntent(openApp())
                .build();
    }

    /**
     * An alarm. Title and text may carry the engine's words or a watched name from another phone,
     * so they pass through printable(). A locked screen shows only lockedVersion().
     */
    private Notification alertNotification(String title, String text) {
        String head = printable(title, TITLE_MAX), body = printable(text, TEXT_MAX);
        return new Notification.Builder(this, CH_ALERTS)
                .setSmallIcon(R.drawable.icon_mono)
                .setContentTitle(head)
                .setContentText(body)
                .setStyle(new Notification.BigTextStyle().bigText(body))
                .setCategory(Notification.CATEGORY_ALARM)
                .setVisibility(Notification.VISIBILITY_PRIVATE)
                .setPublicVersion(lockedVersion())
                .setAutoCancel(true)
                .setContentIntent(openApp())
                .build();
    }

    /** Every alarm's public version: the app's name and LOCKED_TEXT, nothing from an account. */
    private Notification lockedVersion() {
        return new Notification.Builder(this, CH_ALERTS)
                .setSmallIcon(R.drawable.icon_mono)
                .setContentTitle("KaspaVault")
                .setContentText(LOCKED_TEXT)
                .setCategory(Notification.CATEGORY_ALARM)
                .setContentIntent(openApp())
                .build();
    }

    /**
     * Text from the engine, the page or another phone, made safe to show: format characters (Cf:
     * bidi overrides and isolates, zero-width marks), lone surrogates and unassigned code points
     * are dropped, so a name cannot reorder or hide the words around it; controls and line breaks
     * become spaces. Cut to max characters.
     */
    static String printable(String s, int max) {
        if (s == null) return "";
        StringBuilder b = new StringBuilder(Math.min(s.length(), max));
        for (int i = 0; i < s.length() && b.length() < max; ) {
            int c = s.codePointAt(i);
            i += Character.charCount(c);
            int t = Character.getType(c);
            boolean hidden = t == Character.FORMAT || t == Character.SURROGATE
                    || t == Character.UNASSIGNED;
            if (hidden) continue;
            boolean breaks = t == Character.CONTROL || t == Character.LINE_SEPARATOR
                    || t == Character.PARAGRAPH_SEPARATOR;
            b.appendCodePoint(breaks ? ' ' : c);
        }
        return b.toString();
    }

    @Override public void onDestroy() {
        looping = false;
        RUNNING = false;
        try { unregisterReceiver(powerReceiver); } catch (Exception ignored) { }
        if (VaultLib.AVAILABLE) {
            Thread stop = new Thread(() -> {
                try { Log.i(TAG, "guard.stop -> " + brief(VaultLib.call("guard.stop", "{}"))); } catch (Throwable t) { Log.w(TAG, "guard.stop failed: " + t); }
            }, "guard-stop");
            stop.start();
        }
        controlThread.shutdownNow();
        guardThread.shutdownNow();
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }

    /** An engine envelope for logcat: its status code and size, never its body. */
    static String brief(String envelope) {
        String code = envelope.contains("\"status\":200") ? "200" : (envelope.contains("\"status\":503") ? "503" : "not 200");
        return code + ", " + envelope.length() + " chars";
    }
}
