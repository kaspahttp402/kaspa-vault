/* The one door to the numbers. Two sources behind it: Sample (in memory, the default) and Live (the wallet
   engine inside the app). Screens only read KVData.state and call the actions; nothing else knows which is on. */
(function () {
  var bridge = window.KaspaVaultBridge || null;
  /* The wallet engine inside the app (the Rust program as a library, no laptop). Java says whether the library loaded. */
  var engine = !!(bridge && bridge.engine && bridge.engine());
  function isLocal() {
    return engine && getSetting('live_local') === '1';
  }
  var RATE = 0.085,
    FEE = 0.02,
    DAY = 86400,
    GRACE = 60; /* GRACE: seconds an on-the-spot send waits on the phone, stoppable (stop any send after it was sent) */
  var listeners = [];
  /* Developer preview only: ?demo=alert or ?demo=quiet in the page address shows the watcher's banners on Sample data, so a
     prover can screenshot them with no service. The phone never opens the page with a query, so Sample mode is unchanged. */
  var DEMO = (function () {
    var m = /[?&]demo=(alert|quiet|sample)/.exec(location.search || '');
    return m ? m[1] : '';
  })();
  var ALARM_KINDS = { withdrawal_pending: 1, lineage_mismatch: 1, spend_stranger: 1, spend: 1 },
    QUIET_AFTER = 180; /* spend kinds */

  /* ---- settings: phone storage through the bridge, browser storage otherwise ---- */
  function getSetting(k) {
    try {
      return (bridge ? bridge.getSetting(k) : localStorage.getItem('kv_' + k)) || '';
    } catch (e) {
      return '';
    }
  }
  function setSetting(k, v) {
    try {
      if (bridge) bridge.setSetting(k, v);
      else localStorage.setItem('kv_' + k, v);
    } catch (e) {}
  }

  /* ---- Sample lives in demo.js, which only demo.html loads (before this file). The phone page never loads it: there
     demo is null and every Sample half below answers that it is not on this phone. ---- */
  var demo = window.KVDemo
    ? window.KVDemo.install({
        get state() {
          return state;
        },
        set state(s) {
          state = s;
        },
        emit: emit,
        wait: wait,
        when: when,
        round: round,
        getSetting: getSetting,
        setSetting: setSetting,
        FEE: FEE,
        DAY: DAY,
        GRACE: GRACE,
        RATE: RATE,
        DEMO: DEMO,
      })
    : null;
  function notHere() {
    return Promise.resolve({ ok: false, error: 'Not on this phone.' });
  }

  /* ---- words that must never reach a screen, whatever the service sends ---- */
  var BANNED = /contract|\bkeys?\b|utxo|script|\bdaa\b|sompi|on-?chain|blockchain|crypto/i;
  function plain(text, fallback) {
    if (!text) return fallback;
    return BANNED.test(text) ? fallback : String(text);
  }

  /* ---- KAS price history: Sample's seeded walk is in demo.js; live comes from the price feed (null hides the chart) ---- */
  /* the real KAS price in dollars (CoinGecko, no key): one series per range, kept ten minutes: every [time, price] point for the
     chart and 72 calm points for the scene's line. CoinGecko gives 5-minute points for a day, hourly up to
     90 days, daily for a year; the phone caps an answer at 4000 points. On the phone it goes through Bridge.price, which reaches that
     one host only and sends nothing of ours. */
  var priceLive = {},
    priceRest = 0 /* no price asks before this time, after a refused one */,
    PRICE_DAYS = {
      day: 1,
      week: 7,
      month: 30,
      quarter: 90,
      year: 365,
      all: 'max',
    }; /* all = CoinGecko days=max, daily since the start */
  function fetchPrices(range) {
    var days = PRICE_DAYS[range] || 30,
      had = priceLive[range];
    if (had && Date.now() - had.at < 600000) return Promise.resolve(had.values);
    if (Date.now() < priceRest) return Promise.resolve(had ? had.values : null); /* the feed said no (429 = too many asks): rest, keep the last numbers */
    var url = 'https://api.coingecko.com/api/v3/coins/kaspa/market_chart?vs_currency=usd&days=' + days;
    return new Promise(function (resolve) {
      function got(status, text) {
        var j = null;
        try {
          j = JSON.parse(text);
        } catch (e) {}
        if (status !== 200) priceRest = Date.now() + 120000;
        resolve(status === 200 ? j : null);
      }
      if (bridge && bridge.price) {
        var id = ++seq;
        waiting[id] = got;
        bridge.price('__kvHttp', id, url);
        return;
      }
      fetch(url)
        .then(function (r) {
          return r.text().then(function (t) {
            got(r.status, t);
          });
        })
        .catch(function () {
          resolve(null);
        });
    })
      .then(function (j) {
        var points = ((j && j.prices) || [])
          .filter(function (p) {
            return p && typeof p[0] === 'number' && typeof p[1] === 'number' && p[1] > 0;
          })
          .map(function (p) {
            return [p[0], p[1]];
          });
        var pts = points.map(function (p) {
          return p[1];
        });
        if (pts.length < 10) return null;
        var n = 72,
          out = [],
          stepN = pts.length / n;
        for (var i = 0; i < n; i++) {
          var a = Math.floor(i * stepN),
            b = Math.max(a + 1, Math.floor((i + 1) * stepN)),
            sum = 0;
          for (var k = a; k < b; k++) sum += pts[k];
          out.push(sum / (b - a));
        }
        out[n - 1] = pts[pts.length - 1];
        priceLive[range] = { at: Date.now(), values: out, points: points };
        state.rate = out[n - 1];
        emit();
        return out;
      })
      .catch(function () {
        return null;
      });
  }
  /* ---- state seen by the screens ---- */
  /* the empty account the Live path starts from: boot() fills it in, then the engine's status. Sample's account is in demo.js. */
  function liveEmptyState() {
    return {
      source: 'sample' /* until boot() turns Live on; a page with neither the engine nor demo.js stays empty */,
      connected: true,
      error: '',
      updated: Date.now(),
      account: '',
      balance: null,
      limit: null,
      spent: null,
      left: null,
      refreshSeconds: 0,
      noticeSeconds: 0,
      pending: null,
      stopped: false,
      safe: null,
      rate: RATE /* dollars per KAS for the money lines until a real price lands */,
      fee: null,
      stopKeys: null,
      heir: false,
      sheets: null,
      locked: false,
      network: '',
      notice: '',
      heirSecondsLeft: null,
      heirDelaySeconds: null,
      heirWarnShare: 0.9,
      hardLimit: false,
      outgoing: null,
      everyday: null,
      coins: [],
      extra: [],
      history: [],
      stops: [],
      alerts: [],
      watcherSeenAt: undefined,
    };
  }
  var state = demo ? demo.sampleState() : liveEmptyState();
  var pollTimer = null,
    tickTimer = null,
    sendId = null;
  var account = 1; /* the account every status poll and money call is about (1..8) */

  function emit() {
    listeners.forEach(function (f) {
      f(state);
    });
  }
  function when() {
    var d = new Date();
    return 'Today, ' + d.getHours() + ':' + ('0' + d.getMinutes()).slice(-2);
  }

  /* ---- Live transport ---- */
  var seq = 0,
    waiting = {};
  var NEED_FINGER = 'Fingerprint needed. Try again when the sensor is ready.';
  /* One fingerprint through the phone's prompt; a desktop browser has no sensor and says yes. */
  function unlock(why) {
    if (!bridge) return Promise.resolve(true);
    return new Promise(function (resolve) {
      window.__kvUnlock = function (st) {
        window.__kvUnlock = null;
        resolve(st === 'ok');
      };
      bridge.confirmWithBiometric('__kvUnlock', why);
    });
  }
  window.__kvHttp = function (id, status, text) {
    var w = waiting[id];
    if (!w) return;
    delete waiting[id];
    w(status, text);
  };
  /* Every request goes to the engine in the app, through Java (bridge.vault) and back on __kvHttp. There is no remote
     service: without the engine a request simply fails as unreachable (status 0). */
  function request(method, path, body) {
    return new Promise(function (resolve) {
      if (!isLocal()) {
        resolve({ status: 0, text: '' });
        return;
      }
      var m = localMethod(method, path, body);
      if (!m) {
        resolve({ status: 404, text: '' });
        return;
      }
      var lid = ++seq;
      waiting[lid] = function (status, text) {
        resolve({ status: status, text: text });
      };
      bridge.vault('__kvHttp', lid, m.name, JSON.stringify(m.args));
    });
  }
  /* Route -> engine method. GET /api/status -> status {}, GET /api/quote?kind=..&amount=..&to=.. -> quote {kind, amount, to},
     POST /api/send|finish|stop|decline -> that word with the body as args. Anything else is not a thing the engine does. */
  function localMethod(method, path, body) {
    var q = path.indexOf('?'),
      p = q < 0 ? path : path.slice(0, q),
      args = {};
    if (method === 'GET' && p === '/api/status') {
      /* ?account=N picks the account */
      var am = /[?&]account=(\d+)/.exec(path);
      return { name: 'status', args: am ? { account: parseInt(am[1], 10) } : {} };
    }
    if (method === 'GET' && p === '/api/keys')
      return { name: 'keys.public', args: {} }; /* this phone's rule block; answers without keys too */
    if (method === 'GET' && p === '/api/funding') return { name: 'funding', args: {} };
    if (method === 'POST' && p === '/api/sheets') return { name: 'keys.sheets', args: {} }; /* the sealed sheets, after a fingerprint */
    if (method === 'POST' && p === '/api/burn') return { name: 'keys.burn', args: body || {} }; /* wipe the sealed sheets on purpose */
    if (method === 'POST' && p === '/api/pursepay') return { name: 'purse.pay', args: body || {} };
    if (method === 'POST' && p === '/api/reset')
      return { name: 'keys.reset', args: body || {} }; /* the purse: where test KAS is received before the account opens */
    if (method === 'GET' && p === '/api/quote') {
      if (q >= 0)
        path
          .slice(q + 1)
          .split('&')
          .forEach(function (kv) {
            var i = kv.indexOf('=');
            if (i < 0) return;
            args[decodeURIComponent(kv.slice(0, i))] = decodeURIComponent(kv.slice(i + 1).replace(/\+/g, ' '));
          });
      return { name: 'quote', args: args };
    }
    if (method === 'POST' && /^\/api\/(send|finish|stop|decline|fund|topup|takeout)$/.test(p))
      return { name: p.slice(5), args: body || {} };
    if (method === 'POST' && p === '/api/rules.change')
      return { name: 'rules.change', args: body || {} }; /* keyless; stores the new dials, moves nothing */
    if (method === 'POST' && p === '/api/heir.set')
      return { name: 'heir.set', args: body || {} }; /* keyless; only while every account is stopped and empty */
    return null;
  }
  function reachError(status) {
    if (status === -4) return 'The wallet engine is not ready. Close and reopen the app.'; /* the library did not load */
    if (status === -3) return 'Unlock first: touch the sensor and try again.'; /* the phone's store was shut; nothing was sent */
    if (status === 0 || status === 503) return "Can't reach the Kaspa network from this phone. Check its internet and try again.";
    return 'Something went wrong (' + status + '). Try again in a moment.';
  }
  function liveJson(r) {
    try {
      return JSON.parse(r.text);
    } catch (e) {
      return null;
    }
  }

  function adoptStatus(s) {
    state.connected = true;
    state.error = '';
    state.updated = Date.now();
    state.balance = s.balance_kas;
    state.limit = s.limit_kas;
    state.spent = s.spent_kas;
    state.left = s.left_kas;
    state.refreshSeconds = s.refresh_seconds;
    state.noticeSeconds = s.notice_seconds;
    state.stopKeys = s.stop_keys || 1;
    state.heir = !!s.heir;
    state.sheets = s.sheets || 1;
    /* v6: the heir clock as the engine counts it; null without an heir or from an older service. The whole delay
       (heir_delay_seconds) is optional: an engine that does not send it makes Live warn by share only. */
    state.heirSecondsLeft = typeof s.heir_seconds_left === 'number' ? s.heir_seconds_left : null;
    state.heirDelaySeconds = typeof s.heir_delay_seconds === 'number' && s.heir_delay_seconds > 0 ? s.heir_delay_seconds : null;
    state.heirWarnShare = typeof s.heir_warn_share === 'number' ? s.heir_warn_share : 0.9;
    state.hardLimit = !!s.hard_limit;
    state.stopped = !!s.stopped;
    state.safe = s.safe_kas;
    if (typeof s.purse_kas === 'number') state.purse = s.purse_kas; /* the purse rides in the status: Home shows every balance */
    if (typeof s.purse_address === 'string') state.purseAddress = s.purse_address;
    state.sheetsOnPhone = !!s.sheets_on_phone;
    state.safeAddress = typeof s.safe_address === 'string' ? s.safe_address : '';
    state.stale = !!s.stale;
    if (state.stale)
      state.connected = false; /* while connecting the engine answers the last run's status with stale:true; money calls still 503 */
    if (isLocal() && !state.stale)
      setSetting(
        'last_balances',
        JSON.stringify({
          balance: state.balance,
          limit: state.limit,
          left: state.left,
          safe: state.safe,
          purse: state.purse,
          stopped: state.stopped,
        })
      ); /* the balance must not go blank for 23 s at every open */
    state.unopened =
      isLocal() &&
      !!s.stopped &&
      !(s.safe_kas > 0) &&
      !(s.balance_kas > 0); /* keys made, nothing in the covenant yet; the engine says "stopped", the app says "not open yet" */
    adoptAccounts(s);
    adoptActivity(s);
  }
  /* several accounts per key set. accountId is the engine's number; state.account stays the name Home shows */
  function adoptAccounts(s) {
    if (typeof s.account === 'number') {
      account = s.account;
      state.accountId = s.account;
    } else state.accountId = account;
    state.accounts = s.accounts && s.accounts.length ? s.accounts.slice() : [1];
    state.nextAccount = typeof s.next_account === 'number' ? s.next_account : null;
    state.rulesNext = s.rules_next || null;
    state.oldRules = !!s.old_rules;
    state.oldRulesNotice = state.oldRules ? plain(s.notice, 'This account was opened by a newer version of the app.') : '';
    if (state.oldRules) state.unopened = false; /* nothing alarming: the record exists, this app just cannot read its rules */
    if (isLocal()) state.account = state.accountId > 1 ? 'Account ' + state.accountId : 'My account';
  }
  /* what is happening on the account: the payment waiting out its notice, the statement lines, the guard's alerts */
  function adoptActivity(s) {
    state.pending = s.pending
      ? {
          to: s.pending.to,
          amount: s.pending.amount_kas,
          secondsLeft: s.pending.seconds_left,
          total: Math.max(s.notice_seconds || 0, s.pending.seconds_left || 0),
          tx: s.pending.tx,
          who: s.pending.to || s.pending.amount_kas != null ? 'You' : '',
          dismissed: state.pending && state.pending.dismissed,
        }
      : null;
    state.history = (s.history || []).map(function (h) {
      return { text: plain(h.text, 'Account activity'), tx: h.tx, go: '', when: '' };
    });
    /* field contract: both optional; an older service leaves them out and the app stays quiet about them */
    state.alerts = Array.isArray(s.alerts)
      ? s.alerts
          .filter(function (a) {
            return a && typeof a.at === 'number' && typeof a.text === 'string';
          })
          .map(function (a) {
            return { at: a.at, kind: String(a.kind || ''), text: plain(a.text, 'Something needs your attention.') };
          })
      : [];
    state.watcherSeenAt = 'watcher_seen_at' in s ? s.watcher_seen_at : undefined;
    state.network = typeof s.network === 'string' ? plain(s.network, '') : ''; /* which network the engine is on; the screen shows it */
  }

  /* The page's one state model: which phase the current account is in. First match wins, in the order Home's line has always
     used. Whether money can move right now is a separate question, answered by canMove() below: a money screen asks it first.
       locked      Live is sealed until a fingerprint opens it.
       oldRules    the account was opened by a newer app; this one cannot read its rules.
       rulesNext   new rules are stored and wait for the account to open again (not while the numbers are stale).
       noKeys      this phone has no keys yet.
       unopened    keys made, nothing in the account yet.
       stopped     the account is stopped; everything is in the vault.
       stale       the last numbers seen, until the engine answers.
       connecting  Live, and the network has not answered yet.
       open        the account is open and current (pending and outgoing are details of this one).
     The flags themselves stay on state; the scene, the purse walk and Send still read some of them directly. */
  function phase() {
    if (state.locked) return 'locked';
    if (state.oldRules) return 'oldRules';
    if (state.rulesNext && !state.stale) return 'rulesNext';
    if (state.noKeys) return 'noKeys';
    if (state.unopened) return 'unopened';
    if (state.stopped) return 'stopped';
    if (state.stale) return 'stale';
    if (state.source === 'live' && state.connected === false) return 'connecting';
    return 'open';
  }
  /* The second question, apart from the phase: can money move right now? Only with a fresh answer from the network.
     An account can be 'stopped' and still unreachable at the same moment, so this is its own test, read by every money screen. */
  function canMove() {
    return state.source !== 'live' || (state.connected !== false && !state.stale);
  }

  /* Which alert the phone must show: the newest withdrawal_pending or lineage_mismatch not yet acknowledged. The acknowledgement
     is a plain preference (alert_ack_at = that alert's own time), so older alerts are hidden with it and a newer one shows again.
     The quiet guard: Live on and the watcher's last good answer missing (null) or older than 180 s. */
  function alertToShow() {
    if (state.source !== 'live' && !DEMO) return null;
    var ack = +getSetting('alert_ack_at') || 0,
      best = null;
    (state.alerts || []).forEach(function (a) {
      if (ALARM_KINDS[a.kind] && a.at > ack && (!best || a.at > best.at)) best = a;
    });
    return best;
  }
  function guardQuiet() {
    if (state.source !== 'live' && !DEMO) return false;
    if (state.watcherSeenAt === undefined) return false;
    return state.watcherSeenAt === null || state.watcherSeenAt < Date.now() / 1000 - QUIET_AFTER;
  }
  /* an unacknowledged withdrawal keeps the 5 s poll going even while the page is hidden */
  function urgent() {
    var a = alertToShow();
    return !!(a && a.kind === 'withdrawal_pending');
  }

  function poll() {
    if (state.source !== 'live' || (document.hidden && !urgent())) return Promise.resolve(false);
    return request('GET', '/api/status?account=' + account).then(function (r) {
      /* the status of the chosen account */
      var j = r.status >= 200 && r.status < 300 ? liveJson(r) : null,
        cj = r.status === 503 ? liveJson(r) : null;
      if (cj && cj.connecting) {
        state.connected = false;
        state.error = 'Connecting to the network…';
        emit();
        return false;
      } /* the engine on this phone is still reaching the network; polling goes on */
      if (j && j.ok === false) {
        /* the engine on this phone refuses plainly; "no keys yet" is a state, not an error */
        state.connected = true;
        state.noKeys = /no keys yet/i.test(j.error || '');
        state.error = state.noKeys ? '' : plain(j.error, 'Your account could not be read.');
        emit();
        return false;
      }
      if (j) {
        state.noKeys = false;
        adoptStatus(j);
      } else {
        state.connected = false;
        state.error = j === null && r.status === 200 ? 'Your account sent something unreadable.' : reachError(r.status);
      }
      emit();
      return !!j;
    });
  }
  function startPolling() {
    stopPolling();
    poll();
    pollTimer = setInterval(poll, 5000);
  }
  function stopPolling() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = null;
  }
  document.addEventListener('visibilitychange', function () {
    if (!document.hidden) poll();
  });

  function liveAction(path, body) {
    return request('POST', path, body).then(function (r) {
      var j = liveJson(r);
      if (r.status === 0 || !j) return { ok: false, error: reachError(r.status) };
      if (!j.ok) return { ok: false, error: plain(j.error, 'That could not be done. Nothing has moved.') };
      return poll().then(function () {
        return j;
      });
    });
  }
  /* keyless engine calls (rules.change, heir.set): no fingerprint, the engine's answer as given, a status poll on ok */
  function plainAction(path, body, fallback) {
    return request('POST', path, body).then(function (r) {
      var j = liveJson(r);
      if (r.status === 0 || !j) return { ok: false, error: reachError(r.status) };
      if (!j.ok) return { ok: false, error: plain(j.error, fallback) };
      return poll().then(function () {
        return j;
      });
    });
  }
  /* Money on Live (send, finish, emergency stop). The sealed signer on this phone opens for ONE
     engine call after a fingerprint; Java takes the key for that move itself, so no key ever reaches this page. Without
     confirmSigner (an older shell) the plain fingerprint prompt stands in. */
  function signerUnlock(why) {
    if (!bridge) return Promise.resolve(true);
    return new Promise(function (resolve) {
      window.__kvUnlock = function (st) {
        window.__kvUnlock = null;
        resolve(st === 'ok');
      };
      if (bridge.confirmSigner) bridge.confirmSigner('__kvUnlock', why);
      else bridge.confirmWithBiometric('__kvUnlock', why);
    });
  }
  var FRESH_FINGER =
    'This one needs your fingerprint again. Try once more.'; /* the engine's answer when an irreversible move wants a fingerprint newer than the signer window */
  function moneyAction(why, path, body) {
    body = body || {};
    if (body.account == null) body.account = account; /* every money call names its account */
    function once() {
      return signerUnlock(why).then(function (ok) {
        return ok ? liveAction(path, body) : { ok: false, error: NEED_FINGER };
      });
    }
    return once().then(function (r) {
      return !r.ok && r.error === FRESH_FINGER ? once() : r;
    }); /* one more fingerprint and one more call; a second refusal stands as it is */
  }
  /* the engine's own "wait" answer ends in AGAIN (a coin it has not seen yet): try every 3 s for up to a minute, showing
     its words; any other answer, a refused fingerprint included, stands at once */
  var AGAIN = 'Trying again in a moment.';
  function retrying(f, say) {
    var until = Date.now() + 60000;
    function once() {
      return f().then(function (r) {
        var e = (r && r.ok === false && r.error) || '';
        if (e.slice(-AGAIN.length) !== AGAIN || Date.now() > until) return r;
        say('wait', e);
        return wait(3000).then(once);
      });
    }
    return once();
  }
  /* the rule change in flight on this account (setting rules_move), or null */
  function moveMarker() {
    var m = null;
    try {
      m = JSON.parse(getSetting('rules_move') || 'null');
    } catch (e) {
      m = null;
    }
    return m && m.account === account ? m : null;
  }

  /* ---- one clock for countdowns; Sample runs its own half of it in demo.js ---- */
  function tick() {
    if (demo && state.source === 'sample') demo.tick();
    else {
      if (state.refreshSeconds > 0) state.refreshSeconds--;
      var p = state.pending;
      if (p && p.secondsLeft > 0) p.secondsLeft--;
      if (state.heirSecondsLeft > 0) state.heirSecondsLeft--; /* the heir clock runs between polls too */
    }
    emit();
  }
  tickTimer = setInterval(tick, 1000);

  function round(n) {
    return Math.round(n * 100) / 100;
  }
  function randomHex(n) {
    var b = crypto.getRandomValues(new Uint8Array(n / 2)),
      s = '';
    for (var i = 0; i < b.length; i++) s += ('0' + b[i].toString(16)).slice(-2);
    return s;
  }
  function wait(ms) {
    return new Promise(function (r) {
      setTimeout(r, ms);
    });
  }

  /* ---- what the screens call ---- */
  window.KVData = {
    get state() {
      return state;
    },
    /* the sample walk is for the demo only; the phone shows nothing until real prices arrive */
    priceSeries: function (range) {
      var l = priceLive[range];
      return l ? l.values : state.source === 'live' || isLocal() || !demo ? null : demo.samplePrices(range).values;
    },
    pricePoints: function (range) {
      var l = priceLive[range];
      return l ? l.points : state.source === 'live' || isLocal() || !demo ? null : demo.samplePrices(range).points;
    } /* every [time, price] of the range, for the chart */,
    fetchPrices: fetchPrices,
    priceIsLive: function (range) {
      return !!priceLive[range];
    },
    getSetting: getSetting,
    setSetting: setSetting,
    plain: plain,
    subscribe: function (f) {
      listeners.push(f);
    },
    isLive: function () {
      return state.source === 'live';
    },
    phase: phase /* the page's one state model; see the list above phase() */,
    canMove: canMove /* money screens: a fresh network answer, whatever the phase */,
    isLocal: isLocal /* Live on this phone's own engine */,
    isMainnet: function () {
      return isLocal() && getSetting('live_network') !== 'testnet';
    } /* the shell's default network is mainnet */,
    /* the engine on this phone. keysPublic answers without keys too (the rule block code, the purse address, the
       public file to carry); funding = the purse and what it holds; fund opens the account from the purse (fingerprint);
       makeKeys has Java make and seal the three keys after a fingerprint, the key bytes never reach this page. */
    keysPublic: function () {
      return request('GET', '/api/keys').then(function (r) {
        return liveJson(r) || { ok: false, error: reachError(r.status) };
      });
    },
    funding: function () {
      return request('GET', '/api/funding').then(function (r) {
        return liveJson(r) || { ok: false, error: reachError(r.status) };
      });
    },
    fund: function (kas, dials, acctId) {
      /* acctId opens that account (default the current one) */
      var a = dials || {};
      a.kas = kas;
      var n = typeof acctId === 'number' ? acctId : account;
      a.account = n;
      return moneyAction('Open ' + (n > 1 ? 'account ' + n : 'your account') + ' with ' + kas + ' KAS', '/api/fund', a);
    },
    /* the new dials for this account; the engine keeps them and status.rules_next shows them until the account is opened again */
    changeRules: function (dials) {
      if (state.source !== 'live') return Promise.resolve({ ok: false, error: 'Not in Sample.' });
      var a = { account: account },
        d = dials || {};
      for (var k in d) if (d.hasOwnProperty(k) && d[k] != null) a[k] = d[k];
      return plainAction('/api/rules.change', a, 'The rules could not be changed. Nothing has moved.');
    },
    /* the heir on this phone's keys: a kaspa: address or a 64-hex x-only key; '' removes the heir */
    setHeir: function (input, autoHeir) {
      if (state.source !== 'live') return Promise.resolve({ ok: false, error: 'Not in Sample.' });
      var v = String(input || '').trim(),
        a = {};
      if (v === '') a.heir_x = '';
      else if (/^[0-9a-fA-F]{64}$/.test(v)) a.heir_x = v.toLowerCase();
      else if (/^kaspa(test)?:[a-z0-9]{20,}$/.test(v.toLowerCase())) a.heir_address = v.toLowerCase();
      else return Promise.resolve({ ok: false, error: 'That is not a Kaspa address.' });
      if (typeof autoHeir === 'boolean') a.auto_heir = autoHeir;
      return plainAction('/api/heir.set', a, 'The heir could not be set. Nothing has moved.');
    },
    /* which account the page is on; setAccount switches and polls at once */
    get account() {
      return account;
    },
    setAccount: function (id) {
      var n = parseInt(id, 10);
      if (!(n >= 1 && n <= 8)) return Promise.resolve(false);
      account = n;
      state.accountId = n;
      if (isLocal()) state.account = n > 1 ? 'Account ' + n : 'My account';
      sendId = null;
      emit();
      return poll();
    },
    makeKeys: function (args) {
      if (!bridge || !bridge.keysNew) return Promise.resolve({ ok: false, error: 'Keys can only be made on the phone.' });
      return signerUnlock('Make the keys for this phone')
        .then(function (ok) {
          if (!ok) return { ok: false, error: NEED_FINGER };
          return new Promise(function (resolve) {
            var id = ++seq;
            waiting[id] = function (status, text) {
              var j = +status === 200 ? liveJson({ text: text }) : null;
              resolve(j || { ok: false, error: reachError(+status) });
            };
            bridge.keysNew('__kvHttp', id, JSON.stringify(args));
          });
        })
        .then(function (r) {
          if (r.ok) {
            state.noKeys = false;
            poll();
          }
          return r;
        });
    },
    demo: DEMO,
    alertToShow: alertToShow,
    guardQuiet: guardQuiet,
    ackAlert: function () {
      var a = alertToShow();
      if (a) setSetting('alert_ack_at', String(a.at));
      emit();
    },

    /* true when a Send goes straight through */
    fitsLimit: function (kas) {
      return kas + FEE <= (state.left || 0);
    } /* the engine counts the fee against the limit too; 0.02 over-reserves, which is safe */,

    /* Decline: a pending withdrawal is withdrawn; the money stays Locked, ready as before (contract v5 `decline`; Live: the engine's decline, a stop key signs) */
    declinePending: function () {
      var p = state.pending;
      if (!p) return { ok: true };
      if (state.source === 'live') return moneyAction('Decline this withdrawal', '/api/decline', {});
      return demo ? demo.declinePending() : { ok: false, error: 'Not on this phone.' };
    },
    /* Coins: Everyday's small coins become one (sample); the engine has no consolidate call yet */
    combineCoins: function () {
      if (state.source === 'live') return { ok: false, error: 'Combining coins is not in this build yet.' };
      return demo ? demo.combineCoins() : { ok: false, error: 'Not on this phone.' };
    },
    send: function (to, kas, label, mustWait) {
      if (state.source === 'live') {
        /* the id lets the engine pay once when its answer was lost and the user slides again */
        if (!sendId || sendId.to !== to || sendId.kas !== kas) sendId = { to: to, kas: kas, id: randomHex(16) };
        var id = sendId.id;
        return moneyAction('Send ' + kas + ' KAS', '/api/send', { to: to, kas: kas, id: id }).then(function (r) {
          if (r.ok && sendId && sendId.id === id) sendId = null;
          return r;
        });
      }
      return demo ? demo.send(to, kas, label, mustWait) : notHere();
    },

    /* the Send draft was dropped: the next send is a new payment with its own id */
    dropSend: function () {
      sendId = null;
    },
    finish: function () {
      if (state.source === 'live') return moneyAction('Finish your withdrawal', '/api/finish', {});
      return demo ? demo.finish() : notHere();
    },

    /* "That was me": the money keeps its course; only the red bell and banner go quiet (Alerts still lists it, with Decline) */
    dismissPending: function () {
      if (state.pending) state.pending.dismissed = true;
      emit();
    },
    /* Stop on a send still in its grace window: nothing left the phone, the money is back as if nothing happened */
    stopOutgoing: function () {
      var o = state.outgoing;
      if (!o) return { ok: true };
      if (state.source === 'live')
        return { ok: false, error: 'Your account sends at once; the minute to change your mind is not there yet.' };
      return demo ? demo.stopOutgoing() : { ok: false, error: 'Not on this phone.' };
    },

    stop: function () {
      if (state.source === 'live') return moneyAction('Emergency stop', '/api/stop', {});
      return demo ? demo.stop() : notHere();
    },

    /* A rule change on this phone. rules.change has stored the new rules and answered the move: first the money goes to the
       purse (a payment; above the daily limit it waits the notice period like any large one), then finishRules closes the
       old record (stop) and opens the same account from the purse; fund with no rules applies the stored ones. */
    moveRules: function (r) {
      return this.send(r.to, r.move_kas, 'New rules').then(function (x) {
        if (x && x.ok) setSetting('rules_move', JSON.stringify({ account: account, kas: +r.move_kas || 0 }));
        return x;
      });
    },
    /* ready to finish: the payment to the purse is due, or it has landed (the purse holds it, or the record is already
       stopped after it). A payment the person stopped or declined leaves the purse without it, so the rules just wait. */
    rulesReady: function () {
      if (!isLocal() || !state.rulesNext) return false;
      var p = this.rulesPending(),
        m = moveMarker();
      if (p) return !(p.secondsLeft > 0);
      return !!(m && (m.now || (state.purse || 0) >= m.kas * 0.9));
    },
    /* the payment to the purse that carries a rule change, while it waits; null when there is none */
    rulesPending: function () {
      var p = state.pending;
      return state.rulesNext && p && state.purseAddress && p.to === state.purseAddress ? p : null;
    },
    /* finish (if a payment waits), stop (unless stopped), then fund from the purse less a fee margin. Each step checks the
       state first, so after a refusal the same call picks up where it stopped. onStep('finish'|'stop'|'fund') names the step. */
    finishRules: function (onStep) {
      var api = this,
        say = onStep || function () {};
      if ((moveMarker() || {}).now) return this.updateNow(onStep); /* an instant update the app left half done */
      function next(r, f) {
        return r && r.ok === false ? r : f();
      }
      return Promise.resolve({ ok: true })
        .then(function (r) {
          return next(r, function () {
            if (!state.pending) return { ok: true };
            say('finish');
            return api.finish();
          });
        })
        .then(function (r) {
          return next(r, function () {
            if (state.stopped) return { ok: true };
            say('stop');
            return api.stop();
          });
        })
        .then(function (r) {
          return next(r, function () {
            if (!state.rulesNext) return { ok: true, done: true };
            say('fund');
            return api.funding().then(function (f) {
              if (!f || f.ok === false) return { ok: false, error: (f && f.error) || 'The purse could not be read. Try again.' };
              var floor = typeof f.min_kas === 'number' ? f.min_kas : 1.05,
                kas = Math.floor(((+f.kas || 0) - 0.05) * 10000) / 10000;
              if (!(kas >= floor))
                return { ok: false, error: 'Your purse holds too little to update your rules. It needs at least ' + floor + ' KAS.' };
              return api.fund(kas, null, account).then(function (x) {
                if (x && x.ok) setSetting('rules_move', '');
                return x;
              });
            });
          });
        });
    },
    /* "Update now" (rules.change answered now.ok): stop, take this account's own stop coin out of the safe to the purse,
       open the same account from it under the stored rules. The marker records each step, so a refusal or a closed app
       picks up where it stopped (a second take-out would wait for a coin already taken). onStep as in finishRules. */
    updateNow: function (onStep) {
      var api = this,
        say = onStep || function () {},
        m = moveMarker();
      if (!m || !m.now) m = { account: account, kas: 0, now: true };
      function mark(k, v) {
        if (k) m[k] = v;
        setSetting('rules_move', JSON.stringify(m));
      }
      function stopStep() {
        if (state.stopped || m.out) return Promise.resolve({ ok: true });
        say('stop');
        return api.stop();
      }
      function outStep() {
        if (m.out) return Promise.resolve({ ok: true });
        say('takeout');
        var take = function () {
          return moneyAction('Update your rules now', '/api/takeout', { from_stop: true });
        };
        return retrying(take, say).then(function (x) {
          if (x && x.ok) {
            mark('open', +x.open_kas || 0);
            mark('out', true);
          }
          return x;
        });
      }
      function openStep() {
        if (!state.rulesNext) return Promise.resolve({ ok: true, done: true });
        if (!(m.open > 0))
          return api.funding().then(function (f) {
            mark('open', Math.floor(((+(f && f.kas) || 0) - 0.05) * 10000) / 10000); /* the take-out's answer was lost: the purse less a fee margin */
            return m.open > 0 ? openStep() : { ok: false, error: 'Your purse could not be read. Try again.' };
          });
        say('fund');
        var open = function () {
          return api.fund(m.open, null, account);
        };
        return retrying(open, say).then(function (x) {
          if (x && x.ok) setSetting('rules_move', '');
          return x;
        });
      }
      mark();
      return stopStep()
        .then(function (r) {
          return r && r.ok === false ? r : outStep();
        })
        .then(function (r) {
          return r && r.ok === false ? r : openStep();
        });
    },

    /* Sample conveniences; Live has no equivalent */
    skipAhead: function () {
      if (demo) demo.skipAhead();
    },
    /* Sample only: the heir clock starts again, as any payment would; skipHeir runs a fifth of the silence off so the warning can be seen */
    renewClock: function () {
      if (state.source === 'live') return;
      if (demo) demo.renewClock();
    },
    skipHeir: function () {
      if (state.source === 'live') return;
      if (demo) demo.skipHeir();
    },
    /* Out of the vault: two of the three sheets, typed once, go to the engine for this one call and are
       not kept; the engine signs the 2-of-3 spend from the vault to `to` (empty = this phone's purse) of `kas` (0 = everything). */
    takeOut: function (sheets, to, kas) {
      if (state.source !== 'live') {
        this.takeOutOfSafe();
        return Promise.resolve({ ok: true });
      }
      var a = {};
      if (sheets && sheets.length) a.sheets = sheets;
      if (to) a.to = to;
      if (kas > 0) a.kas = kas; /* no sheets = the phone signs with its own sealed ones */
      return moneyAction('Take money out of your vault', '/api/takeout', a).then(function (r) {
        a.sheets = null;
        sheets.length = 0;
        return r;
      });
    },
    /* Purse -> the open account (engine topup, the funding key) */
    topup: function (kas) {
      if (state.source !== 'live') return Promise.resolve({ ok: false, error: 'Not in Sample.' });
      return moneyAction('Add ' + kas + ' KAS to your account', '/api/topup', { kas: kas });
    },
    sheets: function () {
      return moneyAction('Show your sheets', '/api/sheets', {});
    },
    burnSheets: function () {
      return moneyAction('Burn the sheets on this phone', '/api/burn', { confirm: true });
    },
    pursePay: function (to, kas) {
      var a = { to: to };
      if (kas > 0) a.kas = kas;
      return moneyAction('Pay from your purse', '/api/pursepay', a);
    },
    resetKeys: function (leaveSafe) {
      return moneyAction('Start fresh keys', '/api/reset', leaveSafe ? { confirm: true, leave_safe: true } : { confirm: true }).then(
        function (r) {
          if (r.ok) {
            state.noKeys = true;
            emit();
          }
          return r;
        }
      );
    },
    takeOutOfSafe: function () {
      if (demo) demo.takeOutOfSafe();
    },
    addAccount: function (a) {
      if (demo) demo.addAccount(a);
    },
    moveIn: function (kas, name) {
      if (demo) demo.moveIn(kas, name);
    },
    addHistory: function (text, go) {
      state.history.unshift({ text: text, when: when(), go: go, kind: 'people' });
      emit();
    },
    resetSample: function () {
      if (demo) demo.resetSample();
    },

    /* Live, locked, until a fingerprint opens it. Stale numbers are never carried over: they start empty (the protection
       pips too: they fill from the engine's status), only the last seen balances show until the first status lands. A
       refused fingerprint leaves Live locked: Home says so and offers another try; it never falls back to Sample quietly. */
    boot: function () {
      /* With the engine in the app, Live is the account on this phone: on by default the first time. A page with no engine
         (a desktop browser) is Sample. */
      if (engine && getSetting('live_local') === '') setSetting('live_local', '1');
      if (!isLocal()) return;
      state = liveEmptyState();
      state.source = 'live';
      state.account = 'My account';
      state.everyday = null;
      state.fee = null;
      state.balance = state.limit = state.left = state.spent = state.safe = null;
      state.history = [];
      state.connected = false;
      try {
        var lb = JSON.parse(getSetting('last_balances') || 'null');
        if (lb && typeof lb === 'object') {
          state.balance = lb.balance;
          state.limit = lb.limit;
          state.left = lb.left;
          state.safe = lb.safe;
          state.purse = lb.purse;
          state.stopped = !!lb.stopped;
          state.stale = true;
        }
      } catch (e) {} /* last seen numbers until the first status lands */
      state.refreshSeconds = state.noticeSeconds = 0;
      state.stopKeys = state.sheets = null;
      state.heir = false;
      state.locked = true;
      state.error = '';
      emit(); /* no fingerprint prompt by itself; Home shows the lock seal and a button, and only that press opens it (retryUnlock) */
    },
    /* The button on Home's locked state. */
    retryUnlock: function () {
      return state.source === 'live' && state.locked ? openLive() : Promise.resolve(true);
    } /* resolves true once open (the lock screen uses it, so one fingerprint opens the engine and Home together) */,
  };
  function openLive() {
    return unlock('Open your account').then(function (ok) {
      if (!ok) {
        state.locked = true;
        state.error = '';
        emit();
        return false;
      }
      state.locked = false;
      startPolling();
      emit();
      return true;
    });
  }
})();
