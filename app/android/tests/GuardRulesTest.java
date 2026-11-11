package com.kaspavault.wallet;

/**
 * Plain Java checks of the alerts rules (no Android). Run: bash tools/java_tests.sh
 * Each row is something a phone has shown or the engine can answer.
 */
public final class GuardRulesTest {
    private static int failed = 0, ran = 0;

    private static void check(String what, boolean ok) {
        ran++;
        if (!ok) { failed++; System.out.println("FAIL " + what); }
    }

    /** What the engine said when one public node answered with a server error. */
    private static final String DEAD_NODE = "Can't reach the network right now "
            + "(wss://ivy.kaspa.green/kaspa/mainnet/wrpc/borsh): no secure node answered: "
            + "wRPC -> WebSocket -> WebSocket error: HTTP error: 500 Internal Server Error";
    private static final String NO_RECORD = "state.json is missing from /data/x: the guard "
            + "starts from the vault's covenant id and last known state.";
    private static final String STOPPED = "Alerts stopped. Open KaspaVault to start them again.";

    private static void starts() {
        check("a good start", GuardRules.started(200, true, ""));
        check("already running counts as started", GuardRules.started(400, false, GuardRules.ALREADY));
        check("503 is not a start", !GuardRules.started(503, false, ""));
        check("200 without ok is not a start", !GuardRules.started(200, false, ""));
        check("a refusal is not a start", !GuardRules.started(400, false, DEAD_NODE));
    }

    private static void endings() {
        check("a dead node is tried again", GuardRules.retryAfterEnd(DEAD_NODE));
        check("no words is tried again", GuardRules.retryAfterEnd(""));
        check("a missing record is tried again", GuardRules.retryAfterEnd(NO_RECORD));
        check("an unknown ending is tried again", GuardRules.retryAfterEnd("anything else"));
        check("a closed account is a real stop", !GuardRules.retryAfterEnd(GuardRules.FINISHED));
    }

    private static void kinds() {
        String[] network = {"watcher_silent", "unchecked", "unwatched", "lineage_mismatch"};
        for (String k : network) check(k + " never shows", GuardRules.aboutNetwork(k));
        String[] money = {"withdrawal_pending", "withdrawal_completed", "withdrawal_declined",
            "stopped", "heir_clock", "spend", "spend_stranger", "a_kind_added_later"};
        for (String k : money) check(k + " shows", !GuardRules.aboutNetwork(k));
    }

    private static void words() {
        check("a dead node's words are never shown", STOPPED.equals(GuardRules.plainWords(DEAD_NODE, STOPPED)));
        check("a file name is never shown", STOPPED.equals(GuardRules.plainWords(NO_RECORD, STOPPED)));
        check("empty words fall back", STOPPED.equals(GuardRules.plainWords("", STOPPED)));
        check("null words fall back", STOPPED.equals(GuardRules.plainWords(null, STOPPED)));
        String plain = "This account was closed.";
        check("a plain sentence is kept", plain.equals(GuardRules.plainWords(plain, STOPPED)));
    }

    private static void standing() {
        check("heard since the start is on", GuardRules.ON.equals(GuardRules.standing(true, 105, 100)));
        check("heard in the start's second is on", GuardRules.ON.equals(GuardRules.standing(true, 100, 100)));
        check("an accepted start not yet heard is starting", GuardRules.STARTING.equals(GuardRules.standing(true, 0, 100)));
        check("an old heartbeat is starting", GuardRules.STARTING.equals(GuardRules.standing(true, 90, 100)));
        check("an ended guard is starting", GuardRules.STARTING.equals(GuardRules.standing(false, 105, 100)));
        check("no start time is starting", GuardRules.STARTING.equals(GuardRules.standing(true, 105, 0)));
    }

    public static void main(String[] args) {
        starts();
        standing();
        endings();
        kinds();
        words();
        System.out.println("GuardRulesTest: " + (ran - failed) + "/" + ran + " pass");
        if (failed > 0) System.exit(1);
    }
}
