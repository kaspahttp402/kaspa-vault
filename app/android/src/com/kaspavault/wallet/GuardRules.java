package com.kaspavault.wallet;

/**
 * The decisions of the alerts service, kept free of Android so they run in a plain Java test
 * (app/android/tests/GuardRulesTest.java). The service only acts on what these answer.
 * The rule: alerts end by the user's switch or by a closed account, never by a passing fault.
 */
final class GuardRules {
    private GuardRules() {}

    /** The engine's answer when its guard is already running here (core.rs GUARD_RUNNING). */
    static final String ALREADY = "The guard is already running.";
    /** The engine's word for a guard that ended because the account closed or it was stopped. */
    static final String FINISHED = "finished";

    /** The two standing lines. */
    static final String ON = "Alerts on", STARTING = "Alerts starting";

    /** The standing line: "on" only once the running guard has been heard from since its start
     *  (an accepted start can still end a moment later; that must never read as on). */
    static String standing(boolean running, long seenAt, long startedAt) {
        return running && seenAt >= startedAt && startedAt > 0 ? ON : STARTING;
    }

    /** Whether a guard.start answer means the guard is running now. */
    static boolean started(int status, boolean ok, String error) {
        return (status == 200 && ok) || ALREADY.equals(error);
    }

    /** Whether a guard that ended by itself is started again: always, but for a closed account. */
    static boolean retryAfterEnd(String exited) {
        return !FINISHED.equals(exited);
    }

    /** Alert kinds that say something about the network and not about the money (the same four
     *  as watch.rs about_network): they never ring and never show as a notification. */
    static boolean aboutNetwork(String kind) {
        return "watcher_silent".equals(kind) || "unchecked".equals(kind)
                || "unwatched".equals(kind) || "lineage_mismatch".equals(kind);
    }

    /** The engine's sentence when it is plain; `fallback` when it is empty or carries an address,
     *  a file name or connection words nobody should have to read. */
    static String plainWords(String text, String fallback) {
        if (text == null || text.isEmpty()) return fallback;
        String t = text.toLowerCase();
        boolean raw = t.contains("://") || t.contains("wrpc") || t.contains("websocket")
                || t.contains("error:") || t.contains(".json") || t.contains("node")
                || t.contains("network") || t.contains("daa");
        return raw ? fallback : text;
    }
}
