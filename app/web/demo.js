/* Sample: the in-memory demo account for a desktop browser. Only demo.html loads it (before data.js); the phone page
   (index.html) never does, so none of this code is on the phone. data.js hands install() a small seam and calls back here
   for every Sample action, the Sample prices and the Sample half of its one-second clock. */
(function () {
  window.KVDemo = {
    install: function (api) {
      var emit = api.emit,
        when = api.when,
        wait = api.wait,
        round = api.round,
        getSetting = api.getSetting,
        FEE = api.FEE,
        DAY = api.DAY,
        GRACE = api.GRACE,
        RATE = api.RATE,
        DEMO = api.DEMO;
      var priceCache = {};
      /* sample walk per range: [points, seed, start as a share of today's rate, step size, seconds between points]; seeded, so the same every time */
      var SAMPLE = {
        day: [96, 7, 0.985, 0.012, 900],
        week: [112, 11, 0.93, 0.03, 5400],
        month: [120, 13, 1.08, 0.03, 21600],
        quarter: [120, 17, 1.21, 0.035, 64800],
        year: [120, 19, 0.74, 0.045, 262800],
      };
      function samplePrices(range) {
        /* -> { values: calm series for the scene, points: [[time, price], ...] for the chart } */
        if (priceCache[range]) return priceCache[range];
        var d = SAMPLE[range] || SAMPLE.month,
          n = d[0],
          seed = d[1],
          raw = [],
          v = RATE * d[2];
        for (var i = 0; i < n; i++) {
          seed = (seed * 9301 + 49297) % 233280;
          v = v * (1 + (seed / 233280 - 0.5) * d[3]);
          raw.push(v);
        }
        raw[n - 1] = RATE;
        var out = raw.slice(),
          now = Date.now();
        for (var k = 0; k < 2; k++) for (var j = 1; j < n - 1; j++) out[j] = (out[j - 1] + out[j] + out[j + 1]) / 3; // calm, no jitter
        priceCache[range] = {
          values: out,
          points: raw.map(function (p, idx) {
            return [now - (n - 1 - idx) * d[4] * 1000, p];
          }),
        };
        return priceCache[range];
      }

      function sampleState() {
        var now = Date.now();
        return {
          source: 'sample',
          connected: true,
          error: '',
          updated: now,
          account: 'Savings',
          balance: 1240,
          limit: 100,
          spent: 15,
          left: 85,
          refreshSeconds: 6 * 3600,
          noticeSeconds: 24 * 3600,
          pending: null,
          stopped: false,
          safe: 0,
          rate: RATE,
          fee: FEE,
          plan: 'Standard',
          stopKeys: 2,
          heir: false,
          sheets: 3,
          locked: false,
          network: '',
          notice: '',
          heirSecondsLeft: null,
          heirDelaySeconds: null,
          heirWarnShare: 0.9,
          hardLimit: false,
          lastActivity: now /* the heir clock: Sample counts the silence from the last activity */,
          outgoing: null /* a send in its grace window: {to, amount, secondsLeft, total} */,
          everyday: { name: 'Everyday', balance: 86.4 },
          coins: [40, 20.5, 12.1, 6.3, 3.2, 2.1, 1.4, 0.8] /* Everyday's coins (sample); the vault is one coin */,
          extra: [],
          history: [
            { text: 'Limit refreshed, 100 KAS ready', when: 'Yesterday, 9:12', go: 'home', kind: 'refresh' },
            { text: 'Paid 15 KAS to Corner Cafe', when: 'Today, 8:40', go: 'home', kind: 'paid' },
            { text: 'Mum added you as a guardian', when: 'Monday', go: 'people', kind: 'people' },
          ].reverse(),
          stops: [],
          /* the watcher's word: alerts newest first; watcherSeenAt undefined = the service never said, null = no watcher file */
          alerts:
            DEMO === 'alert'
              ? [
                  {
                    at: Math.floor(now / 1000),
                    kind: 'withdrawal_pending',
                    text: 'A withdrawal of 400 KAS was asked for at 14:02 and leaves in 23 h 40 min unless you stop it.',
                  },
                ]
              : [],
          watcherSeenAt: DEMO === 'quiet' ? null : undefined,
        };
      }

      /* the grace window ran out: the money leaves now */
      function payOutgoing() {
        var o = api.state.outgoing;
        if (!o) return;
        api.state.outgoing = null;
        renew();
        api.state.balance = round(api.state.balance - o.amount - FEE);
        api.state.spent = round(api.state.spent + o.amount + FEE);
        api.state.left = round(api.state.limit - api.state.spent);
        api.state.refreshSeconds = DAY;
        api.state.history = api.state.history.filter(function (h) {
          return h.kind !== 'outgoing';
        });
        api.state.history.unshift({ text: 'Paid ' + o.amount + ' KAS to ' + o.to, when: when(), go: 'home', kind: 'paid' });
      }
      function landPending() {
        var p = api.state.pending;
        if (!p) return;
        renew();
        api.state.balance = round(api.state.balance - p.amount - FEE);
        api.state.history.unshift({ text: 'Your ' + p.amount + ' KAS to ' + p.to + ' arrived', when: when(), go: 'home', kind: 'paid' });
        api.state.pending = null;
        var nq = api.state.queue && api.state.queue.shift();
        if (nq) {
          api.state.pending = {
            to: nq.to,
            amount: nq.amount,
            secondsLeft: api.state.noticeSeconds,
            total: api.state.noticeSeconds,
            who: nq.who,
            tx: 'sample',
          };
          api.state.history = api.state.history.filter(function (h) {
            return h.kind !== 'queued' || h.text.indexOf(nq.amount + ' KAS to ' + nq.to) < 0;
          });
          api.state.history.unshift({
            text: 'Pending withdrawal of ' + nq.amount + ' KAS to ' + nq.to,
            when: when(),
            go: 'pending',
            kind: 'pending',
          });
        }
      }
      /* the heir clock in Sample: the delay in hours from the account rules (the heir dial's months; 8760 h when unset)
         minus the silence since the last activity. Live gets the same numbers from the engine. */
      function heirDelaySeconds() {
        return (api.state.heirMonths ? api.state.heirMonths * 730 : 8760) * 3600;
      }
      function heirClock() {
        if (!api.state.heir) {
          api.state.heirSecondsLeft = null;
          api.state.heirDelaySeconds = null;
          return;
        }
        var total = heirDelaySeconds();
        api.state.heirDelaySeconds = total;
        api.state.heirSecondsLeft = Math.max(0, total - Math.floor((Date.now() - (api.state.lastActivity || Date.now())) / 1000));
      }
      /* any spend, request, decline, finish or top-up renews the coin on the network, so the heir clock starts again */
      function renew() {
        if (api.state.source === 'sample') {
          api.state.lastActivity = Date.now();
          heirClock();
        }
      }

      return {
        sampleState: sampleState,
        samplePrices: samplePrices,
        /* the Sample half of the one-second clock; data.js emits after it */
        tick: function () {
          if (api.state.refreshSeconds > 0) {
            api.state.refreshSeconds--;
            if (api.state.refreshSeconds === 0 && !api.state.stopped) {
              api.state.spent = 0;
              api.state.left = api.state.limit;
            }
          }
          var p = api.state.pending;
          if (p && p.secondsLeft > 0) {
            p.secondsLeft--;
            if (p.secondsLeft === 0) landPending();
          }
          var o = api.state.outgoing;
          if (o && o.secondsLeft > 0) {
            o.secondsLeft--;
            if (o.secondsLeft === 0) payOutgoing();
          }
          heirClock();
        },
        declinePending: function () {
          var p = api.state.pending;
          if (!p) return { ok: true };
          api.state.history.unshift({
            text: 'Declined the withdrawal of ' + p.amount + ' KAS to ' + p.to + '. The money stays.',
            when: when(),
            go: 'home',
            kind: 'decline',
          });
          api.state.pending = null;
          renew();
          emit();
          return { ok: true };
        },
        combineCoins: function () {
          var c = api.state.coins || [];
          if (c.length < 2) return { ok: true };
          var total = c.reduce(function (a, x) {
              return a + x;
            }, 0),
            fee = Math.round(c.length * 0.4) / 100;
          api.state.coins = [round(total - fee)];
          if (api.state.everyday) api.state.everyday.balance = api.state.coins[0];
          api.state.history.unshift({
            text: 'Combined ' + c.length + ' Everyday coins into one, fee ' + fee + ' KAS',
            when: when(),
            go: 'home',
            kind: 'coins',
          });
          emit();
          return { ok: true };
        },
        send: function (to, kas, label, mustWait) {
          if (api.state.stopped) return Promise.resolve({ ok: false, error: 'This account is stopped. Everything is in your vault.' });
          if (kas + FEE > api.state.balance) return Promise.resolve({ ok: false, error: 'That is more than this account holds.' });
          var name = label || to.slice(0, 12) + '…';
          return wait(1400).then(function () {
            if (!mustWait && kas + FEE <= api.state.left) {
              if (getSetting('send_grace') !== 'on') {
                /* straight out, final on the network within a second */
                api.state.balance = round(api.state.balance - kas - FEE);
                api.state.spent = round(api.state.spent + kas + FEE);
                api.state.left = round(api.state.limit - api.state.spent);
                api.state.refreshSeconds = DAY;
                renew();
                api.state.history.unshift({ text: 'Paid ' + kas + ' KAS to ' + name, when: when(), go: 'home', kind: 'paid' });
                emit();
                return { ok: true, kind: 'now', tx: 'sample' };
              }
              if (api.state.outgoing) payOutgoing(); /* one at a time: the earlier one leaves now */
              api.state.outgoing = { to: name, amount: kas, secondsLeft: GRACE, total: GRACE };
              api.state.history.unshift({ text: 'Sending ' + kas + ' KAS to ' + name, when: when(), go: 'home', kind: 'outgoing' });
              emit();
              return { ok: true, kind: 'now', tx: 'sample' };
            }
            if (api.state.pending && !api.state.pending.dismissed) {
              /* a second large payment waits in line: its own notice period starts when the first has left */
              api.state.queue = api.state.queue || [];
              api.state.queue.push({ to: name, amount: kas, who: 'You' });
              api.state.history.unshift({
                text: 'In line: ' + kas + ' KAS to ' + name + ', after the one waiting',
                when: when(),
                go: 'pending',
                kind: 'queued',
              });
              emit();
              return { ok: true, kind: 'queued', tx: 'sample' };
            }
            renew(); /* a request renews the coin too */
            api.state.pending = { to: name, amount: kas, secondsLeft: api.state.noticeSeconds, total: api.state.noticeSeconds, who: 'You', tx: 'sample' };
            api.state.history.unshift({ text: 'Pending withdrawal of ' + kas + ' KAS to ' + name, when: when(), go: 'pending', kind: 'pending' });
            emit();
            return { ok: true, kind: 'pending', tx: 'sample' };
          });
        },
        finish: function () {
          landPending();
          emit();
          return Promise.resolve({ ok: true });
        },
        stopOutgoing: function () {
          var o = api.state.outgoing;
          if (!o) return { ok: true };
          api.state.outgoing = null;
          api.state.history = api.state.history.filter(function (h) {
            return h.kind !== 'outgoing';
          });
          api.state.history.unshift({
            text: 'Stopped the ' + o.amount + ' KAS to ' + o.to + ' before it left',
            when: when(),
            go: 'home',
            kind: 'stopped',
          });
          emit();
          return { ok: true };
        },
        stop: function () {
          return wait(1800).then(function () {
            var moved = api.state.balance;
            api.state.safe = round(api.state.safe + moved);
            api.state.balance = 0;
            api.state.left = 0;
            api.state.spent = api.state.limit;
            api.state.stopped = true;
            api.state.pending = null;
            api.state.queue = [];
            renew();
            api.state.stops.unshift({ text: 'Moved ' + moved.toLocaleString('en-US') + ' KAS into your vault', when: when() });
            api.state.history.unshift({
              text: 'Emergency stop: ' + moved.toLocaleString('en-US') + ' KAS moved to your vault',
              when: when(),
              go: 'safe',
              kind: 'stop',
            });
            emit();
            return { ok: true, tx: 'sample' };
          });
        },
        skipAhead: function () {
          landPending();
          emit();
        },
        renewClock: function () {
          renew();
          api.state.history.unshift({ text: 'Clock restarted', when: when(), go: 'heirclock', kind: 'heir' });
          emit();
        },
        skipHeir: function () {
          api.state.lastActivity = (api.state.lastActivity || Date.now()) - heirDelaySeconds() * 200;
          heirClock();
          emit();
        },
        takeOutOfSafe: function () {
          api.state.balance = round(api.state.balance + api.state.safe);
          api.state.safe = 0;
          api.state.stopped = false;
          api.state.spent = 0;
          api.state.left = api.state.limit;
          api.state.refreshSeconds = 0;
          renew(); /* money coming back is a top-up */
          api.state.history.unshift({ text: 'Took your money out of the vault', when: when(), go: 'home', kind: 'safe' });
          emit();
        },
        addAccount: function (a) {
          api.state.extra.push(a);
          emit();
        },
        moveIn: function (kas, name) {
          var kept = Math.min(kas, api.state.everyday.balance);
          api.state.everyday.balance = round(api.state.everyday.balance - kept);
          api.state.extra.forEach(function (a) {
            if (a.name === name) a.balance = round(a.balance + kept);
          });
          emit();
        },
        resetSample: function () {
          if (api.state.source === 'sample') {
            api.state = sampleState();
            emit();
          }
        },
      };
    },
  };
})();
