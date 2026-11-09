/* Screens, flow and gestures. Numbers come only from KVData; the background only through KVScene.setState. */
(function () {
  var D = KVData,
    bridge = window.KaspaVaultBridge || null;
  var stage = document.getElementById('stage'),
    tabs = document.getElementById('tabs'),
    sheet = document.getElementById('sheet');
  var WORDS =
    'amber river candle meadow orbit velvet harbor pencil lantern copper willow spruce marble thistle ember quiet garden nickel summit bramble falcon tundra pebble saddle'.split(
      ' '
    );
  var ADDRESS = 'kaspa:qqdemoreceiveaddressnotrealnobodyownsthisonlyforthedemo000000';
  function rcvAddr() {
    return D.isLocal() ? ui.purse || '' : ADDRESS;
  } /* on this phone's own engine, Receive shows the purse */
  function sendPayees() {
    var s = D.state;
    return D.isLive() ? (s.purseAddress ? [['My purse', s.purseAddress]] : []) : payees();
  } /* the purse is a payee on Send */
  function payees() {
    return CONTACTS.concat(
      ui.people
        .filter(function (q) {
          return q.role === 'Trusted payee';
        })
        .map(function (q) {
          return [q.name, q.addr || ''];
        })
    );
  }
  var CONTACTS = [
    ['Landlord', 'kaspa:qqdemopayeeonenotrealnobodyownsthisonlyforthedemo0000000000000'],
    ['Anna', 'kaspa:qqdemopayeetwonotrealnobodyownsthisonlyforthedemo0000000000000'],
    ['Mum', 'kaspa:qqdemopayeethreenotrealnobodyownsthisonlyforthedemo00000000000'],
  ];
  /* every time dial is kept in hours (fractions allowed, so a custom 30 minutes is .5); presets are the calm choices, Custom takes any number */
  var TIME = {
    notice: {
      label: 'Notice period',
      presets: [1, 6, 12, 24, 48, 72, 168],
      none: 'None',
      zero: 'No waiting time. Large payments leave at once, so a stolen phone could empty this account before you notice.',
    },
    window: {
      label: 'Daily limit refreshes every',
      presets: [6, 12, 24, 48, 168],
      none: 'Always',
      zero: 'The limit resets immediately, so it offers no protection.',
    },
    heir: {
      label: 'Heir can claim after an inactivity period of',
      presets: [2160, 4320, 8760, 17520],
      none: 'No inactivity period',
      zero: 'Your heir could claim the money today.',
    },
  };
  var UNIT = { minutes: 1 / 60, hours: 1, days: 24 };
  var HEIR_WARN = {
    amber: 0.75,
    red: 0.9,
  }; /* share of the heir silence run before the clock warns; Live takes red from the engine (heir_warn_share) */
  /* the KAS chart (price, high/low marks and moving averages): sizes in SVG units (one unit = one CSS pixel), colours live in style.css */
  var CHART = {
    h: 204,
    top: 20,
    bottom: 32,
    left: 0,
    right: 46 /* drawing height and the margins: price labels sit in the right margin, time labels in the bottom one */,
    grid: 4,
    times: 4 /* horizontal price lines and time labels along the bottom */,
    dot: 3 /* radius of the dot on the last point */,
    ma: [
      { key: 'chart_ma20', n: 20, cls: 'ma20', label: 'MA 20' },
      { key: 'chart_ma50', n: 50, cls: 'ma50', label: 'MA 50' },
    ] /* simple moving averages: setting key, window in points, class, chip text */,
    ranges: [
      ['day', '1D'],
      ['week', '1W'],
      ['month', '1M'],
      ['quarter', '3M'],
      ['year', '1Y'],
      ['all', 'All'],
    ],
    retries: 3,
    retryMs: 125000 /* when the price feed is busy and nothing is saved: ask again this many times, this far apart */,
  };

  var ui = {
    liveRules: {
      limit: 100,
      notice: 24,
      window: 24,
    } /* the dials Open the account passes to the engine (mainnet defaults; the Home bars change them before opening) */,
    stack: [],
    el: null,
    cur: null,
    acct: 'main',
    draft: { to: '', label: '', amt: '', note: '', err: '' },
    drag: 0,
    stopping: false,
    plan: 'Standard',
    fresh: null,
    weekOver: false,
    notice: '',
    /* Sample's heir named from the heir dial (persondetail); the People screen shows demo.js
       D.state.helpers */
    people: [],
  };

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return '&#' + c.charCodeAt(0) + ';';
    });
  }
  /* text read when shown (esc calls toString), so it can follow the mode */
  function lazyText(f) {
    return { toString: f };
  }
  /* what Settings > Keys lists in each mode (the rows in SCR.settings) */
  function keysHolds() {
    return D.isLocal()
      ? 'your covenant code, your vault seeds, the purse, your guardians and your alerts'
      : 'recovery, your alerts and a second device';
  }
  function plainNum(n) {
    /* at most 8 decimals, never 1e-7: data.js formats what is sent the same way */
    return D.kasText(n);
  }
  /* exact, never rounded here: Live numbers are shown as the vault gave them */
  function shortAddr(a) {
    return a.length > 26 ? a.slice(0, 14) + '\u2026' + a.slice(-8) : a;
  }
  /* a send confirmation shows the whole address, in groups of 4 after its prefix */
  function addrGroups(a) {
    var m = /^([a-z]+:)?(.*)$/i.exec(String(a || ''));
    return (m[1] ? m[1] + ' ' : '') + m[2].replace(/(.{4})(?=.)/g, '$1 ');
  }
  function fmt(n) {
    if (n == null) return '—';
    var p = plainNum(n).split('.');
    p[0] = p[0].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return p.join('.');
  }
  /* an amount box (data.js): '' = everything (0); ".5" and "5." too; at most 8 decimals; else NaN
     (refused) */
  function amountOf(text) {
    return D.amountOf(text);
  }
  /* the one read of the BIP-39 list (bip39.js gives an array): anything but all 2048 words reads
     as empty, so every word check fails closed */
  function bip39List() {
    var list = [].concat(window.KV_BIP39 || []),
      whole = list.length === 2048;
    return whole ? list : [];
  }
  var AMOUNT_BAD = 'Enter an amount like 12.5, with up to 8 decimals.';
  function money(kas) {
    var eur = D.getSetting('currency') === 'eur',
      v = kas * D.state.rate * (eur ? 0.92 : 1);
    return (eur ? '€' : '$') + v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }
  function under(kas) {
    return D.isLive() ? 'KAS' : money(kas);
  }
  function dur(s) {
    if (s >= 31536000 && s % 31536000 === 0) return s / 31536000 + (s === 31536000 ? ' year' : ' years');
    var m = Math.round(s / 60),
      d = Math.floor(m / 1440),
      h = Math.floor((m % 1440) / 60),
      r = m % 60;
    if (m >= 2880) return d + ' days' + (h ? ' ' + h + ' h' : ''); /* a custom 2.5 days reads as it was typed */
    if (m >= 60) return d * 24 + h + ' h' + (r ? ' ' + r + ' min' : '');
    if (m >= 1) return m + ' min';
    return Math.max(0, Math.round(s)) + ' s';
  }
  function clock(s) {
    s = Math.max(0, Math.floor(s || 0));
    var d = Math.floor(s / 86400),
      h = Math.floor((s % 86400) / 3600),
      m = Math.floor((s % 3600) / 60),
      x = s % 60;
    function two(v) {
      return ('0' + v).slice(-2);
    }
    return (d ? d + ' d ' : '') + two(h) + ':' + two(m) + ':' + two(x);
  }
  function tag() {
    return D.isLive() ? '' : '<span class="tag">Sample</span>';
  } /* a live phone carries no badge */
  function glassify(el) {
    /* wrap every child after the top bar in the .glass sheet; keeps the lower-left layout */
    var kids = [].slice.call(el.children),
      start = 0;
    while (start < kids.length && (kids[start].classList.contains('top') || kids[start].classList.contains('alarm'))) start++;
    if (kids.length - start < 1) return;
    var g = document.createElement('div');
    g.className = 'glass';
    kids.slice(start).forEach(function (k) {
      if (k.classList.contains('sealtap')) return;
      g.appendChild(k);
    }); /* the seal button stays a child of the screen: it is placed on the seal's centre, not in the glass sheet */
    el.appendChild(g);
  }
  function menuBtn() {
    return '<button class="menu" data-menu aria-label="Menu"><i></i><i></i><i></i></button>';
  } /* the menu stays top right on every main screen */
  function topbar(extra) {
    return '<div class="top"><button class="back" data-back>Back</button><span class="topright">' + (extra || '') + '</span></div>';
  }
  function bigKas(n) {
    return fmt(n) + '<small>KAS</small>';
  }
  function fit(el) {
    /* long Live balances shrink to stay on one line */
    var len = el.textContent.length,
      room = Math.min(innerWidth, 590) - 56,
      base = Math.min(innerWidth * 0.21, 84);
    el.style.setProperty('--big', Math.min(base, room / (len * 0.56)) + 'px');
  }
  function setText(root, sel, text) {
    var e = root.querySelector(sel);
    if (e && e.textContent !== text) e.textContent = text;
    return e;
  }
  function setHtml(root, sel, html) {
    var e = root.querySelector(sel);
    if (e && e.dataset.h !== html) {
      e.dataset.h = html;
      e.innerHTML = html;
    }
    return e;
  }
  /* ---- three small repeats, each written once ---- */
  function isKaspaAddress(s) {
    return /^kaspa(test)?:[a-z0-9]{20,}$/.test(s);
  }
  /* the purse line a screen shows once the engine has answered: the error in its place, else what it holds, then the screen's own follow-up */
  function readPurse(el, onOk) {
    return D.funding().then(function (r) {
      if (!r.ok) {
        setText(el, '[data-purse]', r.error);
        return;
      }
      var lead = ui.duress ? 'Available: ' : 'Your purse holds ';
      setText(el, '[data-purse]', lead + fmt(r.kas) + ' KAS.');
      if (onOk) onOk(r);
    });
  }
  /* a .seg row of buttons: the tapped one lights, its siblings dim, then the screen's own follow-up */
  function wireSeg(el, sel, onPick) {
    [].forEach.call(el.querySelectorAll(sel + ' button'), function (b) {
      b.onclick = function () {
        [].forEach.call(b.parentNode.children, function (x) {
          x.classList.toggle('on', x === b);
        });
        if (onPick) onPick(b);
      };
    });
  }
  /* ---- time dials: presets as a quiet row of words, plus Custom (a number and minutes / hours / days); zero is allowed and gets one honest line ---- */
  function hrs(key, h) {
    return h === 0 ? TIME[key].none : dur(h * 3600);
  }
  function unitFor(h) {
    return h >= 24 && h % 24 === 0 ? 'days' : h >= 1 && h % 1 === 0 ? 'hours' : 'minutes';
  }
  function timeDial(key, hours) {
    var t = TIME[key],
      custom = t.presets.indexOf(hours) < 0,
      u = unitFor(hours);
    return (
      '<div class="dial time" data-dial="' +
      key +
      '"><div class="h"><span>' +
      t.label +
      '</span><b data-v>' +
      hrs(key, hours) +
      '</b></div>' +
      '<div class="chips">' +
      t.presets
        .map(function (h) {
          return '<button data-h="' + h + '"' + (h === hours ? ' class="on"' : '') + '>' + hrs(key, h) + '</button>';
        })
        .join('') +
      '<button data-h="custom"' +
      (custom ? ' class="on"' : '') +
      '>Custom</button></div>' +
      '<div class="custom"' +
      (custom ? '' : ' hidden') +
      '><input data-num inputmode="decimal" aria-label="' +
      t.label +
      ', number" placeholder="0" value="' +
      (custom ? plainNum(Math.round((hours / UNIT[u]) * 100) / 100) : '') +
      '">' +
      '<span class="seg" data-unit>' +
      ['minutes', 'hours', 'days']
        .map(function (n) {
          return '<button data-u="' + n + '"' + (n === u ? ' class="on"' : '') + '>' + n + '</button>';
        })
        .join('') +
      '</span></div>' +
      '<div class="sub zero" data-zero' +
      (hours === 0 ? '' : ' hidden') +
      '>' +
      t.zero +
      '</div></div>'
    );
  }
  function wireTimeDial(root, key, onChange) {
    var el = root.querySelector('[data-dial="' + key + '"]');
    if (!el) return;
    var num = el.querySelector('[data-num]'),
      unit = el.querySelector('[data-unit] .on').dataset.u;
    function set(h) {
      setText(el, '[data-v]', hrs(key, h));
      el.querySelector('[data-zero]').hidden = h !== 0;
      onChange(h);
    }
    function fromNum() {
      /* an empty field keeps the last value; whole minutes are the finest step */
      if (num.value.trim() === '') return;
      var v = parseFloat(num.value);
      if (isNaN(v) || v < 0) v = 0;
      set(Math.round(v * UNIT[unit] * 60) / 60);
    }
    [].forEach.call(el.querySelectorAll('.chips button'), function (b) {
      b.onclick = function () {
        [].forEach.call(el.querySelectorAll('.chips button'), function (x) {
          x.classList.toggle('on', x === b);
        });
        var c = b.dataset.h === 'custom';
        el.querySelector('.custom').hidden = !c;
        if (c) {
          num.focus();
          fromNum();
        } else set(+b.dataset.h);
      };
    });
    num.oninput = fromNum;
    [].forEach.call(el.querySelectorAll('[data-unit] button'), function (b) {
      b.onclick = function () {
        unit = b.dataset.u;
        [].forEach.call(b.parentNode.children, function (x) {
          x.classList.toggle('on', x === b);
        });
        fromNum();
      };
    });
  }
  function haptic() {
    if (bridge && bridge.haptic) bridge.haptic();
  }

  /* ---- confirm: fingerprint on the phone; an on-screen Confirm only where no money or words are at stake ---- */
  function onScreenConfirm(title, text, danger) {
    return new Promise(function (done) {
      sheet.innerHTML =
        '<div class="pane"><div class="label">Fingerprint is not available here</div><div class="line lite mt8">' +
        esc(title) +
        '</div><div class="sub">' +
        esc(text) +
        '</div>' +
        '<button class="pill' +
        (danger ? ' danger' : '') +
        '" data-yes>Confirm</button><button class="ghost" data-no>Cancel</button></div>';
      sheet.setAttribute('aria-hidden', 'false');
      void sheet.offsetWidth;
      sheet.classList.add('on');
      ui.closeSheet = function (v) {
        ui.closeSheet = null;
        sheet.classList.remove('on');
        sheet.setAttribute('aria-hidden', 'true');
        done(v);
      };
      sheet.querySelector('[data-yes]').onclick = function () {
        ui.closeSheet(true);
      };
      sheet.querySelector('[data-no]').onclick = function () {
        ui.closeSheet(false);
      };
    });
  }
  /* a plain choice before a slide: one line, the action or Not now */
  function askChoice(title, text, yes) {
    return new Promise(function (done) {
      sheet.innerHTML =
        '<div class="pane"><div class="label">' +
        esc(title) +
        '</div><div class="line lite mt8">' +
        esc(text) +
        '</div><button class="pill" data-yes>' +
        esc(yes) +
        '</button><button class="ghost" data-no>Not now</button></div>';
      sheet.setAttribute('aria-hidden', 'false');
      void sheet.offsetWidth;
      sheet.classList.add('on');
      ui.closeSheet = function (v) {
        ui.closeSheet = null;
        sheet.classList.remove('on');
        sheet.setAttribute('aria-hidden', 'true');
        done(v === true);
      };
      sheet.querySelector('[data-yes]').onclick = function () {
        ui.closeSheet(true);
      };
      sheet.querySelector('[data-no]').onclick = function () {
        ui.closeSheet(false);
      };
    });
  }
  /* A sheet that only says no: the one answer in Live mode when the fingerprint did not succeed. */
  function refuse(msg) {
    return new Promise(function (done) {
      sheet.innerHTML =
        '<div class="pane"><div class="label">Not done</div><div class="line lite mt8">' +
        esc(msg) +
        '</div><button class="ghost" data-no>OK</button></div>';
      sheet.setAttribute('aria-hidden', 'false');
      void sheet.offsetWidth;
      sheet.classList.add('on');
      ui.closeSheet = function () {
        ui.closeSheet = null;
        sheet.classList.remove('on');
        sheet.setAttribute('aria-hidden', 'true');
        done(false);
      };
      sheet.querySelector('[data-no]').onclick = function () {
        ui.closeSheet(false);
      };
    });
  }
  /* Live mode: a real fingerprint ("ok") is the only yes; lockout, nothing enrolled, sensor error, cancel are all no.
     Sample mode: with no sensor the plain Confirm sheet stands in, so the app can be browsed on any phone; the phone
     side (MainActivity.confirmWithBiometric) answers "unavailable" only in Sample mode, never in Live. */
  function confirmAction(title, text, danger) {
    if (!bridge) return onScreenConfirm(title, text, danger);
    ui.bio = true; /* the phone's prompt may pause the page: no lock while it is up */
    return new Promise(function (done) {
      window.__kvBio = function (st) {
        ui.bio = false;
        if (st === 'ok') {
          haptic();
          done(true);
        } else if (st === 'cancel') done(false);
        else if (st === 'away') {
          done(false);
          lockNow();
        } /* the phone took the prompt down because the app was put away */ else if (D.isLive() || st !== 'unavailable')
          refuse('Your fingerprint or PIN is needed. Try again.').then(done);
        else onScreenConfirm(title, text, danger).then(done);
      };
      bridge.confirmWithBiometric('__kvBio', title);
    });
  }

  /* ---- help: two sentences per screen (screen id -> text), shown by the "?" in the top corner ---- */
  var HELP = {
    welcome:
      'KVault keeps your money like a bank account, with a daily limit and an emergency stop. Start a new account, restore one with your recovery seed, ' +
      'or open an invitation from someone you know.',
    words:
      'Write these words on paper, never in a photo or a message. Your recovery seed brings this account back on a new phone; your vault seeds open your ' +
      'vault.',
    wordcheck:
      'A quick check that your paper copy is right. Tap the word that matches the number we ask for.',
    makekeys:
      'This sets up your account on this phone with one fingerprint. You then write down three vault seeds, and any two of them open your vault.',
    sheet:
      'One of your three vault seeds; any two open your vault. Write it on paper and keep the three in separate places, never in a photo.',
    sheetcheck:
      'A quick check that your paper copy is right. Tap the word that matches the number asked for.',
    showsheets:
      'Your three vault seeds, kept sealed on this phone behind your fingerprint. Use them to check or rewrite your paper copies.',
    burnsheets:
      'This deletes the sealed copy of your vault seeds from this phone, and it cannot be undone. From then on only your paper seeds open the vault.',
    pursepay:
      'The purse sits outside the account and its rules. A payment from it leaves at once, with no notice period and no daily limit.',
    resetkeys:
      'This starts a fresh account, purse and vault, with new vault seeds to write down. It is only possible once the purse and the account are empty.',
    keysmade:
      'The covenant code identifies the rules your money lives under. Money goes into the purse first, then into your account.',
    topup:
      'Money moves from the purse into your open account and comes under its rules. Leave about 1 KAS in the purse for the network fee.',
    fundacct:
      'Money moves from the purse into your account and comes under its rules. Leave about 1 KAS in the purse for the network fee.',
    restore:
      'Type the 24-word recovery seed from your paper to bring your account back onto this phone. Your vault still opens with two of your vault seeds.',
    home:
      'Your balance, what you can still spend immediately today, and five bars for your rules: daily limit, notice period, emergency stop, heir and vault. ' +
      'Tap a bar to change that rule; the emergency stop and your vault are at the bottom.',
    open:
      'An Everyday account is for spending and receiving. A Protected account adds a covenant: a daily limit, a notice period and an emergency stop.',
    glossary:
      'Every term this app uses, each with a plain line and a technical one. Type in the search line to filter.',
    movein:
      'Moving money from Everyday into your new protected account. You can also do this later.',
    send:
      'Payments under today’s limit go right away. Large ones become a pending withdrawal that waits for the notice period, so you can stop them.',
    sent:
      'Your payment is on its way, or waiting as a pending withdrawal. A pending one can still be stopped until it lands.',
    receive:
      'Share this address, or let someone scan the code, to get paid. Money that arrives goes into this account.',
    pending:
      'A large payment waiting out its notice period. Decline it if you did not ask for it; Approve only dismisses the alert, and the payment still waits.',
    stop:
      'The emergency stop moves all your money, pending withdrawals included, into your vault at once. Only two of your three vault seeds, typed from paper, ' +
      'take it out again.',
    stopdone:
      'Everything is in your vault now. It stays there until you take it out with two of your vault seeds.',
    safe:
      'Your vault holds the money an emergency stop moved out of reach. Taking it out needs two of your three vault seeds, typed from paper.',
    takeout:
      'Type two of your three vault seeds from paper to move money out of the vault. The words are used once and not kept, and a mistyped word moves nothing.',
    people:
      'Your guardians shows who can stop this account for you and who is your heir: scan their link code to add them, then share your watch code. You guard ' +
      'shows the accounts you watch for others and any funds received as heir; share your link code to be added.',
    persondetail:
      'Give the person a name and, where it applies, a daily limit. Adding or removing a guardian or an heir changes your rules, so it ends with Update now.',
    shareinvite:
      'Send this link to the person. It opens their own KVault with the account you set up for them.',
    invite:
      'Someone set up a protected account for you and holds its emergency stop for now. After your first week you can take the stop over.',
    alerts:
      'Everything that happened on your account: payments, pending withdrawals, stops and invitations. Tap one to see where it leads.',
    protection:
      'The rules on this account, one row each; tap a row to change that rule. Below them are who hears about a pending withdrawal and the extra protections ' +
      'on this phone.',
    settings: lazyText(function () {
      return (
        'Money holds your rules, accounts, coins and statement; Keys holds ' +
        keysHolds() +
        '. Help holds how it works, the glossary, preferences and about.'
      );
    }),
    prefs:
      'Your currency, alerts, app lock and fingerprint, and whether screenshots are allowed. These settings apply to this phone only and do not change your ' +
      'rules.',
    guardians:
      'Who holds your emergency stop buttons, and who is your heir. To change them, use ' +
      'Change guardians or heir: it opens People.',
    seconddevice:
      'A second phone or tablet can watch this account and hear about every withdrawal, so a lost phone does not mean a missed alert. Linking one is not ' +
      'part of this app.',
    about:
      'Which build this is, and whether it is Live on this phone or the Sample. Start over wipes the sample and begins again.',
    heirclock:
      'How long until your heir could claim this account. Any payment, top-up or stop restarts the clock, and the line turns amber, then red, as the time ' +
      'gets close.',
    howitworks:
      'Short steps, one idea each, on how your account protects your money. Swipe or tap Next; each step links to its full story.',
    linkcode:
      'Your link code lets an account holder make you their guardian, their heir, or both. It contains nothing secret, so it is safe to show.',
    papers:
      'Every paper you need, and whether each is done: your three vault seeds, your paper stop and its printed code, and any ' +
      'heir seed. Tap a row to write, check or print it.',
    heirsheet:
      'These 24 words are your heir seed. If this phone is lost, they are the only way to claim an account you are heir to.',
    heircheck:
      'A quick check that your paper copy of the heir seed is right. Tap the word that matches the number asked for.',
    watchcode:
      'Your guardians scan this code to see the account: its balance, any pending withdrawal and the heir clock. It cannot move money; show it again after ' +
      'you change who helps you.',
    watched:
      'An account you guard: its balance, any pending withdrawal and its heir clock. You can stop it or send a withdrawal back, and claim it as heir when ' +
      'its inactivity period has passed, but never spend from it.',
    heirpurse:
      'Funds you claimed as an heir. They sit outside any account and its rules, and go where you send them at once.',
    paperstop:
      'An emergency stop button written on paper as 24 words. With it, a lost phone is recovered from paper alone.',
    paperuse:
      'For a lost phone: the code printed with your stop words, then the 24 words. Everything moves to your vault.',
    paperout:
      'Any two of your three vault seeds take the funds out of your vault, with no fingerprint. ' +
      'First the account code, then the two vault seeds, then where the funds go.',
  };
  /* which walk-through step a screen's ? leads to */
  var HELP_STEP = {
    heirclock: 14,
    home: 6,
    open: 5,
    send: 9,
    sent: 8,
    pending: 8,
    stop: 11,
    stopdone: 11,
    safe: 11,
    takeout: 11,
    words: 2,
    wordcheck: 2,
    restore: 2,
    people: 13,
    persondetail: 13,
    shareinvite: 13,
    invite: 13,
    protection: 7,
    seconddevice: 12,
    guardians: 13,
    about: 16,
    receive: 10,
    settings: 15,
    price: 17,
    linkcode: 13,
    heirsheet: 13,
    heircheck: 13,
    watchcode: 13,
    watched: 13,
    heirpurse: 14,
  };
  /* the How-it-works screen: [title, story] */
  var STORIES = [
    [
      'Your account has a daily limit',
      'Every day you can spend up to your daily limit immediately, with a tap and your fingerprint. The limit refreshes once a day. Anything a thief could grab in a hurry is capped at that amount.',
    ],
    [
      'Large payments wait',
      'A payment above your daily limit does not leave at once. It becomes a pending withdrawal, waits out your notice period, and you get a message. If it is yours, let it go through; if it is not, stop it.',
    ],
    [
      'The emergency stop',
      'Slide the stop and all your money moves into your vault in one go, pending withdrawals included. Nobody can spend from the vault: not you, not a ' +
      'thief, not us. Two of your three vault seeds, kept on paper, open it again.',
    ],
    [
      'Your vault seeds',
      'Your three vault seeds open your vault, and any two are enough. They live on paper; the copy on this phone ' +
        'is sealed behind your fingerprint, so a thief with your phone cannot open the vault.',
    ],
    [
      'Guardians and sponsoring',
      'A guardian is someone you trust who holds one of your emergency stop buttons and watches your account from their own phone; if you lose yours, they ' +
      'can stop everything for you. Sponsoring works the other way round: you set up a protected account for a newcomer and hold their stop until they are ' +
      'ready to take it over.',
    ],
    [
      'What a thief can and cannot do with your unlocked phone',
      'They can see your balance and spend up to today’s limit. They cannot take more without waiting out the notice period, during which you or a guardian ' +
      'stop everything. They cannot reach your vault, because it needs two vault seeds and the copy on the phone is sealed behind your fingerprint.',
    ],
  ];
  /* the walk-through (page by page, the basics first, Kaspa last): [heading, what happens, why, stories to read in full] */
  var STEPS = [
    [
      'Digital money, in one minute',
      'KAS is money that lives on a public network of computers instead of in a bank. Nobody runs it: thousands of computers keep the same record and agree on every payment.',
      'That is why nobody can freeze it or take it from you, and why nobody can undo a mistake for you either.',
      [],
    ],
    [
      'Your address',
      'An address is like an account number: a long string that starts with kaspa:. Anyone can pay into it and anyone can look at it, but only your keys can pay out of it.',
      'Share your address freely. Keep your seed to yourself.',
      [],
    ],
    [
      'Your vault seeds',
      'Your three vault seeds are words written on paper, and any two of them open your vault.',
      'Whoever holds the words holds the money, so they never go into a photo, a message or a website.',
      [3],
    ],
    [
      'A payment',
      'A payment is final the moment the network accepts it, in about a second. There is no bank to call and no chargeback.',
      'That is why this app gives you a limit, a notice period and a stop: ways to catch a mistake before it is final.',
      [],
    ],
    [
      'Fees',
      'Every payment carries a tiny fee for the network, a fraction of a cent. The app shows it before you slide.',
      'The fee is the same whether you send 1 KAS or 100,000.',
      [],
    ],
    [
      'What a covenant is',
      'A covenant is the set of rules your money lives under: how much can leave each day, how long large payments wait, and what happens in an emergency. You pick the rules, and the Kaspa network itself enforces them.',
      'Why it matters: nobody can break them, not a thief, not us, not even you in a hurry.',
      [6],
    ],
    [
      'Home',
      'Your balance, what is still available immediately today, and five bars for the five rules on this account: limit, notice, stops, heir and vault. Tap a bar to change that rule. The log underneath shows what happened.',
      'Everything that guards your money is on this one screen. The account name at the top switches between your accounts.',
      [],
    ],
    [
      'The daily limit',
      'A protected account has a daily limit, the way a bank account does. Payments up to your daily limit go through at once, with a tap and your fingerprint. The limit refreshes once a day.',
      'Why a limit: a stolen phone costs you at most one day’s limit, never the whole account.',
      [0],
    ],
    [
      'The notice period',
      'Anything above the limit does not leave straight away. It becomes a pending withdrawal and waits out your notice period. You get a message the moment it is asked for, and if it was you, it lands by itself when the wait is over.',
      'Why the wait: time to notice a payment you did not make, and to stop it.',
      [1],
    ],
    [
      'Send',
      'Type or scan an address, or pick a trusted payee. The screen says whether this payment goes now or waits as a large payment. Your fingerprint, then a slide, and it is on its way.',
      'A slide is harder to do by accident than a tap. A payment that waits can still be pulled back.',
      [1],
    ],
    [
      'Receive',
      'Your code and your address. Copy it or share it. Send only Kaspa to it.',
      'Money arriving needs nothing from you: the network delivers it to your address, and it is yours the moment it lands.',
      [],
    ],
    [
      'The emergency stop and the vault',
      'One red button moves everything in the account, pending withdrawals included, into your vault. No phone can pay out of the vault. Only two of your ' +
      'three vault seeds, typed from paper, open it again.',
      'Why the vault is separate: a thief with your phone cannot reach it, and neither can we.',
      [2, 3],
    ],
    [
      'Three stops and guardians',
      'Your emergency stop has up to three buttons: yours, and one each for up to two guardians you trust. Any one of them can stop the account, so a ' +
      'guardian can press theirs when your phone is lost. Adding or removing a guardian changes your rules and ends with Update now.',
      'Why three: stopping must always be possible, and a stop can only ever move money into your vault, never out of it.',
      [4],
    ],
    [
      'People',
      'Two tabs. Your guardians shows who can stop this account and who is your heir; you add them by scanning their link code and share your watch code so ' +
      'they can see the account. You guard shows the accounts you watch for others, and funds you received as heir.',
      'Each of them is a rule the network enforces, not a promise.',
      [4],
    ],
    [
      'The heir after inactivity',
      'You can name an heir. If the account hears nothing from you for the inactivity period you set, your heir can claim the money. Every payment you make, ' +
      'even a small one, starts the inactivity period again. The emergency stop is stronger than the heir. Once the money is in your vault only the vault ' +
      'seed opens it, so a stop pressed in time keeps a false heir out too. What an heir claims lands on their phone under Received as heir. Naming or ' +
      'removing an heir changes your rules and ends with Update now.',
      'Why inactivity, and why it resets: your money should not be lost with you, and while you are here it stays yours.',
      [],
    ],
    [
      'Settings',
      lazyText(function () {
        return (
          'Money holds your rules, your accounts, your coins and your statement. Keys holds ' +
          keysHolds() +
          '. Help holds this walk-through and what the words mean.'
        );
      }),
      'Every rule can be adjusted from a screen in the app.',
      [],
    ],
    [
      'What we can and cannot promise',
      'The rules were tested against the attacks we could think of: stolen phones, rushed payments, a lost stop. Everything is open source, so anyone can ' +
      'read the rules and test them. Nobody can return money that has left, and whoever holds your recovery seed and two of your vault seeds holds everything.',
      'Why we say it plainly: with money, a promise you cannot check is worth nothing.',
      [5],
    ],
    [
      'Why Kaspa',
      'Kaspa is the network this money runs on, and the reason this account can exist. A payment is final in about a second and costs a fraction of a cent, day and night, with no bank hours and no middleman. It has run since 2021 on proof of work, the same kind of security that guards Bitcoin, with no company behind it and no coins set aside for insiders: every coin was mined in the open. It is the fastest proof-of-work network there is, and it does not slow down when the world uses it.',
      'And Kaspa can hold rules on money. Your daily limit, your notice period, your stop and your heir are enforced by the network itself, not by this app and not by us. No other network gives ordinary money this protection at this speed. That is why we built on Kaspa.',
      [],
    ],
  ];
  /* the rule shape each step shows in the scene behind it (scene.js modes); steps without one
     keep the calm ribbon. No step draws a copy of a real screen: on the phone that copy held
     the owner's real people and numbers (a guardian's name once showed in it) */
  /* a step's shape: a mode, or a whole scene (the K seal tracing in = the logo locking in) */
  var KSEAL = { mode: 'safe', mark: 'k', lift: 0.25 },
    REDSTOP = { mode: 'stopped', k: 1, lift: 0.25 };
  var STEP_SCENE = { 2: 'safe', 5: 'coin', 7: 'limit', 8: 'notice', 9: 'sending', 10: 'receiving',
    11: REDSTOP, 12: 'stops', 13: 'people', 14: 'beat', 16: KSEAL, 17: KSEAL };
  /* the screen each step talks about: the menu's explanation opens that step */
  var STEP_PEEK = {
    5: 'protection',
    6: 'home',
    7: 'protection',
    8: 'pending',
    9: 'send',
    10: 'receive',
    11: 'stop',
    12: 'safe',
    13: 'people',
    14: 'people',
    15: 'settings',
  };
  function openStory(i) {
    var s = STORIES[i];
    if (!s) return;
    sheet.innerHTML =
      '<div class="pane"><div class="label">The full story</div><div class="story mt8"><h2>' +
      esc(s[0]) +
      '</h2><p>' +
      esc(s[1]) +
      '</p></div><button class="ghost" data-no>Close</button></div>';
    sheet.setAttribute('aria-hidden', 'false');
    void sheet.offsetWidth;
    sheet.classList.add('on');
    ui.closeSheet = function () {
      ui.closeSheet = null;
      sheet.classList.remove('on');
      sheet.setAttribute('aria-hidden', 'true');
    };
    sheet.querySelector('[data-no]').onclick = function () {
      ui.closeSheet();
    };
  }
  /* what each pip on Home means: what it is, how it works, why it is there, and the person's own setting */
  var EXPLAIN = {
    limit: {
      title: 'Daily limit',
      what: 'The most that can leave this account in a day without waiting.',
      how: 'Send up to this with a tap and your fingerprint. It refreshes once a day.',
      why: 'If someone steals your phone, this is the most they can take in a hurry.',
      mine: function (g) {
        return 'Yours is ' + fmt(g.limit) + ' KAS a day.';
      },
    },
    notice: {
      title: 'Notice period',
      what: 'How long a payment above your daily limit has to wait before it leaves.',
      how: 'You get a message when one starts. If it is yours, let it go through. If it is not, press the emergency stop.',
      why: 'A thief would have to wait this long for anything bigger, and you would see it coming.',
      mine: function (g) {
        return 'Yours is ' + dur(g.notice) + '.';
      },
    },
    stops: {
      title: 'Emergency stop',
      what: 'A button that moves all your money into your vault at once.',
      how:
        'You hold button 1. Up to two guardians can hold buttons 2 and 3, so someone can press it if you lose your phone.',
      why: 'Once it is pressed, nobody can spend that money from a phone: not you, not a thief, not us.',
      mine: function (g) {
        return 'You have ' + g.stops + (g.stops === 1 ? ' button.' : ' buttons.');
      },
    },
    heir: {
      title: 'Heir',
      what: 'Someone who can claim this account if you stop using it for a long time.',
      how:
        'Every payment you make restarts the clock. Your heir can only claim after the full inactivity period has passed. An emergency stop puts the money ' +
        'in your vault, and nothing leaves the vault without two vault seeds: not for a thief, not for your heir.',
      why: 'Your money is not lost if something happens to you.',
      mine: function (g) {
        return g.heir ? 'You have named an heir.' : 'You have not named one. Tap the Heir bar on Home to name someone.';
      },
    },
    safe: {
      title: 'Vault',
      what: 'Where your money goes when an emergency stop is pressed.',
      how:
        'It opens only with two of your three vault seeds, typed from paper, so losing one is not a disaster.',
      why: 'A thief with your phone cannot reach it.',
      mine: function (g) {
        return g.sheets === 1 ? 'Yours opens with one seed.' : 'Yours has ' + g.sheets + ' seeds; any two open it.';
      },
    },
  };
  function openSheet(html) {
    sheet.innerHTML = '<div class="pane">' + html + '</div>';
    sheet.setAttribute('aria-hidden', 'false');
    void sheet.offsetWidth;
    sheet.classList.add('on');
    ui.closeSheet = function () {
      ui.closeSheet = null;
      sheet.classList.remove('on');
      sheet.setAttribute('aria-hidden', 'true');
    };
    var no = sheet.querySelector('[data-no]');
    if (no)
      no.onclick = function () {
        ui.closeSheet();
      };
  }
  function accountsList(s, local) {
    var list = [['main', s.account || 'Your account', s.balance]];
    if (local) {
      /* one row per account on this phone's keys; only the current one's balance is known */
      var cur = s.accountId || 1;
      list = (s.accounts || [1]).map(function (n) {
        return [
          n === cur ? 'main' : 'acct:' + n,
          n > 1 ? 'Account ' + n : (n === cur && s.account) || 'My account',
          n === cur ? s.balance : null,
        ];
      });
    } else {
      if (s.everyday) list.push(['everyday', s.everyday.name, s.everyday.balance]);
      (s.extra || []).forEach(function (a) {
        list.push([a.name, a.name, a.balance]);
      });
    }
    if (local && s.purse > 0)
      list.push(['purse', 'Purse', s.purse]); /* the purse is in the picker, and picking it adds it to the account */
    return list;
  }
  function wireAccountRow(b) {
    var c = acctColor(b.dataset.a),
      d = b.querySelector('.dot');
    if (c && d) {
      d.style.background = 'rgb(' + c.join(',') + ')';
      d.style.boxShadow = '0 0 8px rgba(' + c.join(',') + ',.6)';
    } /* CSSOM: the page's CSP forbids style attributes */
    b.onclick = function () {
      if (b.dataset.a === 'purse') {
        ui.closeSheet();
        go(D.state.unopened ? 'fundacct' : 'topup', {});
        return;
      }
      if (b.dataset.a.indexOf('acct:') === 0) {
        ui.acct = 'main';
        ui.closeSheet();
        D.setAccount(parseInt(b.dataset.a.slice(5), 10)).then(function () {
          refresh();
          applyScene();
        });
        refresh();
        return;
      } /* switch the engine's account, Home follows */
      ui.acct = b.dataset.a;
      ui.closeSheet();
      refresh();
      applyScene();
    };
  }
  /* the account button top left: pick which account Home shows */
  function openAccounts() {
    var s = D.state,
      local = D.isLocal(),
      list = accountsList(s, local);
    var canOpen = local && typeof s.nextAccount === 'number'; /* a free account number is left (8 at most) */
    openSheet(
      '<div class="label">Accounts</div><div class="list mt8">' +
        list
          .map(function (a) {
            var c = acctColor(a[0]);
            return (
              '<div class="item"><button class="rowmain' +
              (a[0] === ui.acct ? ' on' : '') +
              '" data-a="' +
              esc(a[0]) +
              '"><i class="dot"></i><span><span class="t">' +
              esc(a[1]) +
              '</span></span><span class="r">' +
              (a[2] == null ? '' : fmt(a[2]) + ' KAS') +
              '</span></button><button class="r go" data-edit-a="' +
              esc(a[0]) +
              '" aria-label="Name and colour">Edit</button></div>'
            );
          })
          .join('') +
        '</div>' +
        (D.isLive()
          ? canOpen
            ? '<button class="ghost" data-open-next>Open another account</button>'
            : ''
          : '<button class="ghost" data-open>Open an account</button>') +
        '<button class="ghost" data-no>Close</button>'
    );
    [].forEach.call(sheet.querySelectorAll('[data-a]'), wireAccountRow);
    [].forEach.call(sheet.querySelectorAll('[data-edit-a]'), function (b) {
      b.onclick = function () {
        ui.closeSheet();
        setTimeout(function () {
          accountSheet(b.dataset.editA);
        }, 350);
      };
    });
    var o = sheet.querySelector('[data-open]');
    if (o)
      o.onclick = function () {
        ui.closeSheet();
        go('open', {});
      };
    var on = sheet.querySelector('[data-open-next]');
    if (on)
      on.onclick = function () {
        ui.closeSheet();
        go('fundacct', { account: D.state.nextAccount });
      };
  }
  /* names and colours: a sheet with a field; the account sheet adds the colour swatches */
  var PALETTE = [
    ['Teal', null],
    ['Violet', [168, 140, 255]],
    ['Amber', [255, 190, 90]],
    ['Rose', [255, 120, 150]],
    ['Sky', [110, 190, 255]],
    ['Lime', [170, 255, 120]],
  ];
  function colorMap() {
    try {
      return JSON.parse(D.getSetting('colors') || '{}') || {};
    } catch (e) {
      return {};
    }
  }
  function acctColor(id) {
    var c = colorMap()[id];
    return c && c.length === 3 ? c : null;
  }
  function accountSheet(id) {
    var s = D.state,
      cur =
        id === 'main'
          ? s.account
          : id === 'everyday' && s.everyday
            ? s.everyday.name
            : (s.extra || [])
                .filter(function (a) {
                  return a.name === id;
                })
                .map(function (a) {
                  return a.name;
                })[0] || id,
      chosen = acctColor(id);
    openSheet(
      '<div class="label">Account</div><label class="field mt8"><span>Name</span><input data-nm value="' +
        esc(cur) +
        '" maxlength="24"></label><div class="label mt14">Colour</div><div class="swatches">' +
        PALETTE.map(function (p) {
          var c = p[1];
          return (
            '<button class="swatch' +
            ((!c && !chosen) || (c && chosen && c.join() === chosen.join()) ? ' on' : '') +
            '" data-c="' +
            (c ? c.join(',') : '') +
            '" aria-label="' +
            p[0] +
            '"></button>'
          );
        }).join('') +
        '</div><div class="sub">The scene takes the colour of the account you are looking at.</div><button class="pill" data-save>Save</button><button class="ghost" data-no>Close</button>'
    );
    var pick = chosen ? chosen.join(',') : '';
    [].forEach.call(sheet.querySelectorAll('[data-c]'), function (b) {
      if (b.dataset.c) b.style.background = 'rgb(' + b.dataset.c + ')';
      b.onclick = function () {
        pick = b.dataset.c;
        [].forEach.call(sheet.querySelectorAll('[data-c]'), function (x) {
          x.classList.toggle('on', x === b);
        });
      };
    });
    sheet.querySelector('[data-save]').onclick = function () {
      var v = sheet.querySelector('[data-nm]').value.trim() || cur,
        m = colorMap();
      if (pick) m[id] = pick.split(',').map(Number);
      else delete m[id];
      D.setSetting('colors', JSON.stringify(m));
      if (id === 'main') s.account = v;
      else if (id === 'everyday' && s.everyday) s.everyday.name = v;
      else
        (s.extra || []).forEach(function (a) {
          if (a.name === id) {
            a.name = v;
            if (ui.acct === id) ui.acct = v;
            if (m[id]) {
              m[v] = m[id];
              delete m[id];
              D.setSetting('colors', JSON.stringify(m));
            }
          }
        });
      ui.closeSheet();
      refresh();
      applyScene();
    };
  }
  /* the menu top right: the explanations and the settings live here, not on Home */
  function openMenu() {
    if (ui.duress) return; /* the menu lists every other screen */
    var items = [
      ['explain', 'Explain this screen'],
      ['alerts', 'Alerts'],
      ['protection', 'Your rules'],
      ['howitworks', 'How it works'],
      ['glossary', 'Glossary'],
      ['settings', 'Settings'],
    ];
    openSheet(
      '<div class="label">' +
        (D.isLive() ? 'Your account' : 'Sample') +
        '</div><div class="list mt8">' +
        items
          .map(function (m) {
            return '<button class="item" data-m="' + m[0] + '"><span><span class="t">' + m[1] + '</span></span></button>';
          })
          .join('') +
        '</div><button class="ghost" data-no>Close</button>'
    );
    [].forEach.call(sheet.querySelectorAll('[data-m]'), function (b) {
      b.onclick = function () {
        ui.closeSheet();
        if (b.dataset.m === 'explain') go('howitworks', { step: stepFor(ui.cur && ui.cur.name) });
        else go(b.dataset.m, {}); /* the explainer is the walk, opened at this screen's page */
      };
    });
  }
  /* the heir dial: the named heir and the inactivity period, or a way to name one */
  function heirDialBody(s) {
    var body;
    /* the named heir is the option here; nobody named yet means naming one first */
    var hp = ui.people.filter(function (q) {
      return q.role === 'Heir';
    });
    body = hp.length
      ? '<div class="dial"><div class="h"><span>Your heir</span></div><span class="seg" data-seg>' +
        hp
          .map(function (q, i) {
            return '<button data-v="' + i + '"' + (i === 0 ? ' class="on"' : '') + '>' +
              esc(q.name) + '</button>';
          })
          .join('') +
        '</span></div>' +
        '<div class="dial"><div class="h"><span>Inactivity period before ' +
        esc(hp[0].name) +
        ' can claim it</span><b data-l>' +
        (s.heir ? (s.heirMonths || 6) + ' months' : 'No heir') +
        '</b></div><input type="range" aria-label="Months of inactivity" data-hr min="0" max="24" step="1" value="' +
        (s.heir ? s.heirMonths || 6 : 0) +
        '"><div class="custom"><input data-hnum inputmode="numeric" aria-label="Months of inactivity, any number" placeholder="Or type the ' +
        'months"></div></div>' +
        '<div class="sub">0 means no heir. Every payment you make restarts the inactivity period. After an emergency stop nothing can be taken from your ' +
        'vault, not by your heir either: only your vault seed opens it.</div><button class="ghost" data-heir-add>Name someone else</button>'
      : '<div class="line">You have not named an heir yet. Name one, and this bar sets the ' +
        'inactivity period before they can claim.</div><div class="sub">An heir can claim what ' +
        'is in this account if it hears nothing from you for the inactivity period you set. ' +
        'Never from your vault: after an emergency stop only your vault seed opens it.</div>' +
        '<button class="pill" data-heir-add>Name an heir</button>';
    return body;
  }
  function dialBody(key, s, g) {
    var body = '';
    if (key === 'limit')
      body =
        '<div class="dial"><div class="h"><span>On the spot each day</span><b data-l>' +
        fmt(s.limit) +
        ' KAS</b></div><input type="range" aria-label="Daily limit" data-lr min="0" max="500" step="5" value="' +
        s.limit +
        '"><div class="custom"><input data-lnum inputmode="decimal" aria-label="Daily limit, any ' +
          'amount" placeholder="Or type any amount"></div></div><div class="sub">Anything above ' +
          'it is a large payment: it waits the notice period and can be stopped. At 0, every ' +
          'payment is a large payment.</div>';
    else if (key === 'notice')
      body =
        timeDial('notice', Math.round(noticeOf(s) / 60) / 60) +
        '<div class="sub">How long a large payment waits before it leaves. You, or a guardian, can stop it in that time.</div>';
    else if (key === 'stops')
      body =
        '<div class="dial"><div class="h"><span>Who can press the emergency stop</span></div><span class="seg" data-seg>' +
        [
          [1, 'You'],
          [2, 'You and a guardian'],
          [3, 'Plus a spare'],
        ]
          .map(function (o) {
            return '<button data-v="' + o[0] + '"' + (o[0] === (g.stops || 1) ? ' class="on"' : '') + '>' + o[1] + '</button>';
          })
          .join('') +
        '</span></div><div class="sub">A stop can only ever move money into your vault, never out of it. Guardians are named under People.</div>';
    else if (key === 'heir') body = heirDialBody(s);
    else if (key === 'safe')
      body =
        '<div class="dial"><div class="h"><span>How your vault opens</span></div><span class="seg" data-seg>' +
        [
          [1, 'One seed'],
          [3, 'Any 2 of 3 seeds'],
        ]
          .map(function (o) {
            return '<button data-v="' + o[0] + '"' + (o[0] === (g.sheets || 1) ? ' class="on"' : '') + '>' + o[1] + '</button>';
          })
          .join('') +
        '</span></div><div class="sub">Three seeds on paper; any two open the vault, so one can ' +
          'be lost safely.</div>';
    return body;
  }
  function wireDialInputs(lr, ln, hr, hn, limitV, heirV, root) {
    var r = root || sheet; /* the dial sheet, or one dial's box on the Change the rules screen */
    if (lr)
      lr.oninput = function () {
        if (ln) ln.value = '';
        setText(r, '[data-l]', fmt(+lr.value) + ' KAS');
      };
    if (ln)
      ln.oninput = function () {
        setText(r, '[data-l]', fmt(limitV()) + ' KAS');
      };
    if (hr)
      hr.oninput = function () {
        if (hn) hn.value = '';
        setText(r, '[data-l]', +hr.value === 0 ? 'No heir' : hr.value + (hr.value === '1' ? ' month' : ' months'));
      };
    if (hn)
      hn.oninput = function () {
        var v = heirV();
        setText(r, '[data-l]', v === 0 ? 'No heir' : v + (v === 1 ? ' month' : ' months'));
      };
  }
  function dialSave(key, s, value, hours, preOpen, canRules, heirFn, limitV) {
    /* the rule is in the coin: no save in place, the Change the rules sheet takes over */
    if ((key === 'stops' || key === 'safe') && !value) {
      ui.closeSheet();
      return;
    } /* nothing picked, nothing to change */
    var hv = key === 'heir' && heirFn ? heirFn() : 0,
      heirName =
        (
          ui.people.filter(function (q) {
            return q.role === 'Heir';
          })[0] || {}
        ).name || 'your heir';
    var what =
      key === 'limit'
        ? 'Change the daily limit to ' + fmt(limitV()) + ' KAS'
        : key === 'notice'
          ? 'Change the notice period to ' + dur(Math.round(hours * 3600))
          : key === 'stops'
            ? 'Change the emergency stop to ' +
              ['', 'your button only', 'your button and a guardian\u2019s', 'three buttons: yours, a guardian\u2019s and a spare'][value]
            : key === 'heir'
              ? hv === 0
                ? 'Remove ' + heirName + ' as your heir'
                : 'Set the inactivity period before ' + heirName + ' can claim it to ' + hv +
                  (hv === 1 ? ' month' : ' months')
              : 'Open your vault with ' + (value === 1 ? 'one seed' : 'any two of three seeds');
    if (preOpen) {
      if (key === 'limit') ui.liveRules.limit = limitV();
      else ui.liveRules.notice = hours;
      ui.closeSheet();
      haptic();
      refresh();
      return;
    }
    if (canRules) {
      ui.closeSheet();
      go('changerules', key === 'limit' ? { focus: 'limit', limit: limitV() } : { focus: 'notice', notice: hours });
      return;
    } /* on this phone a Save only sets the dial on the Change the rules screen; nothing is asked of the engine until Review */
    openChangeRules(what, '', function () {
      if (key === 'limit') applyRulesTo(ui.acct, { limit: limitV() });
      else if (key === 'notice') applyRulesTo(ui.acct, { notice: hours });
      else if (key === 'stops') s.stopKeys = value;
      else if (key === 'heir') {
        s.heir = hv > 0;
        s.heirMonths = hv;
        if (hv === 0 && ui.acct === 'main')
          ui.people = ui.people.filter(function (p) {
            return p.role !== 'Heir';
          });
      } /* removed = gone from People too */ else if (key === 'safe') s.sheets = value;
    });
  }
  /* one dial per bar, in a sheet: limit, notice, stop buttons, heir, vault keys. Save leads to the Change the rules sheet. */
  function openDial(key) {
    if (D.isLocal() && phoneRuleSheet(key)) return;
    var s = acct(),
      g = ui.guards,
      live = D.isLive(),
      body = '',
      value = null,
      title = { limit: 'Daily limit', notice: 'Notice period', stops: 'Emergency stop', heir: 'Heir', safe: 'Vault seeds' }[key];
    var preOpen =
      D.isLocal() &&
      D.state.unopened &&
      (key === 'limit' ||
        key === 'notice'); /* before the account opens, these are simply the dials Open the account will use (a Confirm, not just Close) */
    if (preOpen) s = Object.assign({}, s, { limit: ui.liveRules.limit, noticeSeconds: ui.liveRules.notice * 3600 });
    body = dialBody(key, s, g);
    var canRules =
      D.isLocal() && !preOpen && (key === 'limit' || key === 'notice'); /* on this phone these two dials go to the engine (rules.change) */
    openSheet(
      '<div class="label">' +
        title +
        '</div><div class="mt8">' +
        body +
        '</div>' +
        (live && !preOpen && !canRules
          ? '<div class="sub">' + 'This rule cannot be changed from here.' + '</div>'
          : '<button class="pill" data-save>' + (preOpen ? 'Confirm' : 'Save') + '</button>') +
        '<button class="ghost" data-no>Close</button>'
    );
    var ha = sheet.querySelector('[data-heir-add]');
    if (ha)
      ha.onclick = function () {
        ui.closeSheet();
        go('persondetail', { kind: 'Heir' });
      };
    if (key === 'heir' && !sheet.querySelector('[data-hr]')) {
      var sv0 = sheet.querySelector('[data-save]');
      if (sv0) sv0.parentNode.removeChild(sv0);
    }
    var lr = sheet.querySelector('[data-lr]'),
      hr = sheet.querySelector('[data-hr]'),
      ln = sheet.querySelector('[data-lnum]'),
      hn = sheet.querySelector('[data-hnum]'),
      hours = Math.round(noticeOf(s) / 60) / 60;
    function limitV() {
      var v = ln && ln.value.trim() !== '' ? parseFloat(ln.value) : +lr.value;
      return isNaN(v) ? +lr.value : Math.max(0, v);
    }
    function heirV() {
      var v = hn && hn.value.trim() !== '' ? parseInt(hn.value, 10) : hr ? +hr.value : 0;
      return isNaN(v) ? (hr ? +hr.value : 0) : Math.max(0, v);
    }
    wireDialInputs(lr, ln, hr, hn, limitV, heirV);
    if (key === 'notice')
      wireTimeDial(sheet, 'notice', function (h) {
        hours = h;
      });
    wireSeg(sheet, '[data-seg]', function (b) {
      value = +b.dataset.v;
    });
    var sv = sheet.querySelector('[data-save]');
    if (sv)
      sv.onclick = function () {
        dialSave(key, s, value, hours, preOpen, canRules, hr ? heirV : null, limitV);
      };
  }
  /* the heir, the guardians (stop keys) and every rule live inside the coin on the network and cannot be edited in place.
     The only change is updating the rules: the money moves under the new ones. Up to the daily limit moves immediately,
     the rest waits the notice period (a withdrawal above the limit), or the emergency stop and then out of the vault. Every path that used to
     edit a rule or a person in place ends here. Sample fakes the move. */
  /* on this phone a rules change is edited first on one screen (changerules), then confirmed once on a short sheet: the changes in plain
     words, when they take effect and the fee; the mechanics sit under Details. One slide pays the money to the purse (at once, or after
     the notice period like any large payment); "Finish updating my rules" then runs the rest in one go (D.finishRules). */
  var MONTH_S = 30 * 86400; /* one heir month in seconds; the engine counts DAA units at 10 a second */
  function fold(inner) {
    return '<button class="ghost small" data-details>Details</button><div class="more" data-more hidden>' + inner + '</div>';
  }
  function wireFold(root) {
    [].forEach.call(root.querySelectorAll('[data-details]'), function (b) {
      b.onclick = function () {
        var m = b.nextElementSibling;
        m.hidden = !m.hidden;
        b.textContent = m.hidden ? 'Details' : 'Hide details';
      };
    });
  }
  function monthsOf(n) {
    return n + (n === 1 ? ' month' : ' months');
  }
  function rulesToday() {
    var st = D.state,
      hs = st.heirDelaySeconds;
    return {
      limit: +st.limit || 0,
      notice: Math.round(noticeOf(st) / 60) / 60,
      heir: st.heir ? (hs ? Math.max(1, Math.round(hs / MONTH_S)) : acct().heirMonths || 6) : null,
    };
  }
  function rulesStart(now, p) {
    p = p || {};
    return {
      limit: typeof p.limit === 'number' ? p.limit : now.limit,
      notice: typeof p.notice === 'number' ? p.notice : now.notice,
      heir: now.heir != null && typeof p.heir === 'number' ? p.heir : now.heir,
    };
  }
  /* the changed rules only: plain lines for the sheet, engine dials for rules.change */
  function rulesDiff(now, v) {
    var lines = [],
      dials = {};
    if (Math.abs(v.limit - now.limit) > 1e-9) {
      lines.push('Daily limit ' + fmt(now.limit) + ' \u2192 ' + fmt(v.limit) + ' KAS');
      dials.cap_kas = v.limit;
    }
    if (v.notice !== now.notice) {
      lines.push('Large payments wait ' + hrs('notice', now.notice) + ' \u2192 ' + hrs('notice', v.notice));
      dials.delay = Math.round(v.notice * 36000);
    }
    if (now.heir != null && v.heir !== now.heir) {
      lines.push('Heir waits ' + monthsOf(now.heir) + ' \u2192 ' + monthsOf(v.heir));
      dials.heir_delay = Math.round(v.heir * MONTH_S * 10);
    }
    return { lines: lines, dials: dials };
  }
  function heirWaitDial(m) {
    return (
      '<div class="dial"><div class="h"><span>Inactivity period before your heir can claim it</span><b data-l>' +
      monthsOf(m) +
      '</b></div><input type="range" aria-label="Months of inactivity" data-hr min="1" max="24" step="1" value="' +
      m +
      '"><div class="custom"><input data-hnum inputmode="numeric" aria-label="Months of inactivity, any number" placeholder="Or type the months"></div></div>' +
      '<div class="sub">Every payment you make restarts the inactivity period.</div>'
    );
  }
  function changeRulesHtml(p) {
    var now = rulesToday(),
      v = rulesStart(now, p);
    return (
      topbar() +
      '<h1 class="head">Change the rules</h1><p class="sub">Set each rule the way you want it. Nothing changes until you review it.</p>' +
      '<div data-d="limit">' +
      dialBody('limit', { limit: v.limit }) +
      '</div><div data-d="notice">' +
      timeDial('notice', v.notice) +
      '<div class="sub">How long a large payment waits before it leaves. You, or a guardian, can stop it in that time.</div></div>' +
      (now.heir != null ? '<div data-d="heir">' + heirWaitDial(v.heir) + '</div>' : '') +
      '<button class="ghost" data-heir-who>' +
      (D.state.heir ? 'Change who your heir is' : 'Name an heir') +
      '</button><div class="err" data-e></div><div class="push"></div><button class="pill" data-review disabled>Review changes</button>'
    );
  }
  function changeRulesAfter(el, p) {
    var now = rulesToday(),
      v = rulesStart(now, p),
      lb = el.querySelector('[data-d="limit"]'),
      hb = el.querySelector('[data-d="heir"]'),
      rv = el.querySelector('[data-review]');
    var lr = lb.querySelector('[data-lr]'),
      ln = lb.querySelector('[data-lnum]'),
      hr = hb && hb.querySelector('[data-hr]'),
      hn = hb && hb.querySelector('[data-hnum]');
    if (+lr.value !== v.limit) ln.value = plainNum(v.limit); /* beyond the slider: the typed field holds it */
    if (hr && +hr.value !== v.heir) hn.value = String(v.heir);
    function limitV() {
      var x = ln.value.trim() !== '' ? parseFloat(ln.value) : +lr.value;
      return isNaN(x) ? +lr.value : Math.max(0, x);
    }
    function heirV() {
      var x = hn.value.trim() !== '' ? parseInt(hn.value, 10) : +hr.value;
      return isNaN(x) ? +hr.value : Math.max(1, x);
    }
    function read() {
      return { limit: limitV(), notice: v.notice, heir: hr ? heirV() : null };
    }
    function check() {
      rv.disabled = !rulesDiff(now, read()).lines.length;
    }
    wireDialInputs(lr, ln, null, null, limitV, null, lb);
    if (hr) wireDialInputs(null, null, hr, hn, null, heirV, hb);
    [lr, ln, hr, hn].forEach(function (i) {
      if (i) i.addEventListener('input', check);
    });
    wireTimeDial(el, 'notice', function (h) {
      v.notice = h;
      check();
    });
    check();
    el.querySelector('[data-heir-who]').onclick = function () {
      openHeirSheet();
    };
    rv.onclick = function () {
      reviewRules(el, rv, rulesDiff(now, read()));
    };
    var f = p && p.focus && el.querySelector('[data-d="' + p.focus + '"]');
    if (f && f.scrollIntoView) f.scrollIntoView({ block: 'center' });
  }
  /* one call to rules.change with only the changed dials; it moves nothing */
  function reviewRules(el, rv, ch) {
    if (!ch.lines.length) return;
    rv.disabled = true;
    setText(el, '[data-e]', '');
    D.changeRules(ch.dials).then(function (r) {
      rv.disabled = false;
      rulesReviewSheet(ch.lines, r || {});
    });
  }
  /* the review sheet after rules.change (dials, guardians, heir): the slide that moves the money, or Update now */
  function rulesReviewSheet(lines, r, paper) {
    openSheet(reviewSheetHtml(lines, r));
    wireFold(sheet);
    var st = sheet.querySelector('[data-start]');
    if (st)
      st.onclick = function () {
        rulesSlide(st, r);
      };
    var nw = sheet.querySelector('[data-now]');
    if (nw)
      nw.onclick = function () {
        nowSlide(nw, paper);
      };
  }
  /* the instant route: one slide, then "Updating your rules..." runs stop, take-out and reopen (D.updateNow) */
  function nowSlide(btn, paper) {
    armSlide(btn, 'Update now', 'Your funds stay under your control throughout.', false,
      'Slide to update', 'send', function () {
      ui.closeSheet();
      go('waiting', { job: 'rules', now: true, paper: !!paper }, { replace: true });
    });
  }
  /* the slide on the review sheet: 'move' pays the purse, then 'now' finishes at once and 'notice' waits on Home; 'open' finishes straight away */
  function rulesSlide(btn, r) {
    armSlide(btn, 'Update my rules', 'Your funds stay under your control throughout.', false,
      'Slide to update', 'send', function () {
      if (r.route === 'open') {
        ui.closeSheet();
        return go('waiting', { job: 'rules' }, { replace: true });
      }
      D.moveRules(r).then(function (x) {
        if (!x.ok) {
          setText(sheet, '[data-serr]', x.error || 'The action failed. No funds were moved.');
          resetSlide(sheet);
          return;
        }
        ui.closeSheet();
        if (r.kind === 'now') go('waiting', { job: 'rules' }, { replace: true });
        else go('home', {}, { root: true });
      });
    });
  }
  /* the step line under "Updating your rules...": plain words per step, or the engine's own words while it waits */
  var RULE_STEPS = {
    finish: 'Finishing the payment to your purse.',
    stop: 'Moving your funds to your vault.',
    takeout: 'Bringing it back to your purse.',
    fund: 'Opening your account under your new rules.',
  };
  function rulesStep(el) {
    return function (step, words) {
      setText(el, '[data-rstep]', step === 'wait' ? words : RULE_STEPS[step] || '');
    };
  }
  /* Home's one button while a rule change is ready to finish */
  function rulesFinishSlide(btn) {
    armSlide(btn, 'Finish updating my rules', 'Your funds stay under your control throughout.',
      false, 'Slide to finish', 'send', function () {
      go('waiting', { job: 'rules' }, {});
    });
  }
  /* Home's line while new rules are saved */
  function rulesLine() {
    var p = D.rulesPending();
    if (p && p.secondsLeft > 0) return 'Your new rules start in ' + dur(p.secondsLeft) + '.';
    if (D.rulesReady()) return 'Your new rules are ready.';
    return 'Your new rules are saved. Update my rules to apply them.';
  }
  function reviewSheetHtml(lines, r) {
    function rows(a, cls) {
      return (a || [])
        .map(function (t) {
          return '<div class="' + cls + '">' + esc(String(t)) + '</div>';
        })
        .join('');
    }
    var head = '<div class="label">Change the rules</div><div class="mt8">' + rows(lines, 'line') + '</div>';
    if (!r.ok)
      return (
        head +
        '<div class="err">' +
        esc(r.error || 'The rules could not be changed. Nothing has moved.') +
        '</div><button class="ghost" data-no>Close</button>'
      );
    var now = r.now && r.now.ok && r.route !== 'open' && r.kind !== 'now' ? r.now : null;
    function fee(x) {
      return '<div class="sub">' + (x == null ? 'No fee.' : 'About ' + (+x).toFixed(4) + ' KAS in fees.') + '</div>';
    }
    var later = 'Takes effect in ' + dur(D.state.noticeSeconds) + '. You can stop it until then.';
    var when = r.route === 'open' || r.kind === 'now' ? 'Takes effect now.' : later;
    var face = now
      ? '<div class="line">Update now: takes effect now.</div>' +
        fee(now.fee_kas_about) +
        '<div class="line">Update with notice: ' +
        later.charAt(0).toLowerCase() +
        later.slice(1) +
        '</div>' +
        fee(r.fee_kas_about) +
        '<div class="err" data-serr></div><button class="pill" data-now>Update now</button><button class="pill" data-start>Update with notice</button>'
      : '<div class="line">' +
        when +
        '</div>' +
        fee(r.fee_kas_about) +
        '<div class="err" data-serr></div><button class="pill" data-start>Update my rules</button>';
    return (
      head +
      face +
      '<button class="ghost" data-no>Not now</button>' +
      fold(
        (now
          ? '<div class="sub">Updating now briefly stops your account: funds move to your vault, ' +
            'return to your purse, and reopen under the new rules.</div>'
          : '') +
        '<div class="sub">Your rules are stored with your funds, so an update moves them through your purse and back under the new rules.</div>' +
          '<div class="sub">1 KAS remains in your vault; you can withdraw it at any time.</div>' +
          (r.fee_kas == null ? '' : '<div class="sub">Fees for this update: ' +
            (+r.fee_kas).toFixed(4) + ' KAS.</div>')
      )
    );
  }
  /* on this phone no rule says "later". The heir has its own call (heir.set); the stop buttons and vault seeds are part of the keys. */
  function phoneRuleSheet(key) {
    if (key === 'heir') {
      openHeirSheet();
      return true;
    }
    if (key !== 'stops' && key !== 'safe') return false;
    openKeysFact(key === 'stops' ? 'Emergency stop' : 'Vault seeds');
    return true;
  }
  function openKeysFact(title) {
    openSheet(
      '<div class="label">' +
        title +
        '</div><div class="line mt8">These are part of your keys and cannot be changed on their own.</div>' +
        '<button class="pill" data-no>Close</button>'
    ); /* never a way to replace keys from here (the owner's call: not on the main pages at all) */
  }
  /* the heir on this phone's keys (engine heir.set, keyless): a kaspa: address; refused in the engine's words while any account is open or holds money */
  /* ---- guardians and heirs on this phone (docs/PEOPLE_SCREENS.md). The owner's helpers come from the rule block
     (keys.public): stop buttons 2 and 3 when they are not button 1, and the heir. Names live in setting helper_names. ---- */
  /* Who holds stop slot k ('cancel2_x' / 'cancel3_x'): 'you' | 'unheld' | 'guardian'. keys.public
     answers it per slot
     (stops: [{which, holder}]); a key set made before that list answers 'unknown', and an older
     engine nothing: then a
     slot repeating stop 1 is yours, and any other is a guardian only when a link the owner applied
     named it in
     helper_names (phone setup fills the slots with keys nobody holds, since the rule block needs
     five different keys). */
  function slotHolder(k) {
    var pb = ui.pub || {},
      x = pb[k],
      which = k === 'cancel3_x' ? 3 : 2,
      s = (ui.stops || []).filter(function (t) {
        return t && t.which === which;
      })[0];
    if (s && /^(you|unheld|guardian|paper)$/.test(s.holder)) return s.holder;
    if (!x || x === pb.cancel_x) return 'you';
    return Object.prototype.hasOwnProperty.call(D.helperNames(), x) ? 'guardian' : 'unheld';
  }
  function helperRows() {
    if (!D.isLocal()) return D.state.helpers || []; /* Sample: demo.js rows in this same shape */
    var pb = ui.pub || {},
      names = D.helperNames(),
      out = [];
    [
      ['cancel2_x', 2],
      ['cancel3_x', 3],
    ].forEach(function (k) {
      var x = pb[k[0]];
      if (slotHolder(k[0]) === 'unheld')
        out.push({
          role: 'unused',
          which: k[1],
          name: 'Emergency Stop ' + k[1],
          what: 'Not in use. Add a guardian to give it to someone you trust.',
        });
      else if (slotHolder(k[0]) === 'guardian')
        out.push({
          role: 'guardian',
          which: k[1],
          name: names[x] || 'Someone',
          what: 'Guardian. Holds Emergency Stop ' + k[1] +
            '. A stop only ever sends your money to your own vault.',
        });
      else if (slotHolder(k[0]) === 'paper') out.push(paperRow(k[1]));
    });
    var hk = pb.heir_x || pb.heir_address;
    if (hk)
      out.push({
        role: 'heir',
        name: names[pb.heir_x] || names[pb.heir_address] || 'Someone',
        what: 'Heir. After ' + heirSilence() +
          ' with no payment from you, they can claim what is in your account.',
      });
    return out;
  }
  /* Your rules > Who hears: each real guardian and heir, one plain row (their alerts are set on
     their own phone, so no switch here); Sample keeps its made-up guardian Sam */
  function hearRows() {
    if (!D.isLive()) return toggleRow('alert_sam', 'Sam', 'Guardian');
    return helperRows()
      .filter(function (h) {
        return h.role === 'guardian' || h.role === 'heir';
      })
      .map(function (h) {
        var role = h.role === 'heir' ? 'Heir' : 'Guardian';
        return '<div class="item"><span><span class="t">' + esc(h.name) +
          '</span><span class="d">' + role + '. Hears on their own phone.</span></span></div>';
      })
      .join('');
  }
  /* Your rules: who holds each stop button, and who the heir is, in one line each */
  function rulesWho() {
    var stops = ['Button 1: you.'],
      heir = '';
    helperRows().forEach(function (h) {
      if (h.role === 'guardian') stops.push('Button ' + h.which + ': ' + h.name + '.');
      else if (h.role === 'paper') stops.push('Button ' + h.which + ': your paper stop.');
      else if (h.role === 'heir') heir = h.name;
    });
    return { stops: stops.join(' '), heir: heir };
  }
  function paintRulesWho(el) {
    var w = rulesWho();
    setText(el, '[data-whostops]', w.stops);
    setText(el, '[data-whoheir]', w.heir ? w.heir + ' can claim after the inactivity period.' : '');
  }
  function wireRulesWho(el) {
    paintRulesWho(el);
    if (D.isLocal())
      D.keysPublic().then(function (r) {
        if (!r || !r.ok) return;
        ui.pub = r.public || {};
        ui.stops = r.stops || null;
        paintRulesWho(el);
      });
  }
  /* Settings > Guardians: the real list (keys.public stops + names), read-only */
  var GUARD_WARN =
    "Changing a guardian or your heir changes your account's rules. It takes a rules update, " +
    'which moves your funds into a new account, at once or after your notice period. Anyone ' +
    'you remove can no longer stop it or claim it.';
  function guardiansPaint(el) {
    var rows = helperRows()
      .map(function (h) {
        return '<div class="item"><span><span class="t">' + esc(h.name) +
          '</span><span class="d">' + esc(h.what) + '</span></span></div>';
      })
      .join('');
    setHtml(el, '[data-gl]', rows || '<div class="sub">' + (D.isLocal() && !ui.pub ?
      'Loading\u2026' : 'No guardians yet.') + '</div>');
  }
  function guardiansWarn() {
    openSheet(
      '<div class="label">Change guardians or heir</div><div class="line mt8">' + esc(GUARD_WARN) +
        '</div><button class="pill mt8" data-yes>Go to People</button>' +
        '<button class="ghost" data-no>Cancel</button>'
    );
    sheet.querySelector('[data-yes]').onclick = function () {
      ui.closeSheet();
      go('people', {}, { root: true });
    };
  }
  function helperWord(h) {
    if (h.role === 'guardian') return 'Guardian';
    if (h.role === 'paper') return 'Emergency Stop ' + h.which;
    return h.role === 'unused' ? 'Not in use' : 'Heir';
  }
  /* the paper stop's row: never "Not in use", never a guardian with Remove */
  function paperRow(which) {
    return {
      role: 'paper',
      which: which,
      name: 'Paper stop',
      what:
        'Emergency Stop ' + which + ', written on paper as 24 words. ' +
        'A stop only ever sends your money to your own vault.',
    };
  }
  function heirSilence() {
    var hs = D.state.heirDelaySeconds;
    return hs ? dur(hs) : 'the inactivity period set on your account';
  }
  /* People, left column: one short row per helper (name, role); a tap opens the details and Remove */
  function helperRowsHtml() {
    var rows = helperRows()
      .map(function (h, i) {
        return (
          '<button class="item" data-hi="' +
          i +
          '"><span><span class="t">' +
          esc(h.name) +
          '</span><span class="d' +
          (h.role === 'guardian' || h.role === 'heir' ? ' role' : '') +
          '">' +
          helperWord(h) +
          '</span></span></button>'
        );
      })
      .join('');
    return (rows || '<div class="sub">None yet.</div>') + (D.state.peopleNext ? '<div class="sub">Change pending: update your rules to apply it.</div>' : '');
  }
  function openHelperSheet(h) {
    if (!h) return;
    var head = '<div class="label">' + esc(h.name) + '</div><div class="line mt8">' + esc(h.what) +
      '</div>';
    if (h.role === 'unused' || h.role === 'paper')
      return openSheet(head + '<button class="ghost" data-no>Close</button>');
    openSheet(
      '<div class="label">' + esc(h.name) + '</div><div class="line mt8">' + esc(h.what) + '</div><button class="ghost" data-hrm>Remove</button><button class="ghost" data-no>Close</button>'
    );
    sheet.querySelector('[data-hrm]').onclick = function () {
      removeHelper(h);
    };
  }
  /* People, right column: one short row per guarded account */
  function watchedShortHtml(onlyHot) {
    return watchedList()
      .filter(function (w) {
        var st = w.st || {};
        return !onlyHot || st.stage === 'waiting' || st.can_claim;
      })
      .map(function (w) {
        var st = w.st || {},
          hot = st.stage === 'waiting',
          word = hot ? 'Withdrawal waiting' : st.can_claim ? 'Ready to claim' : '';
        if (!word && st.stage === 'stopped') word = 'Stopped. Ask the owner for a new watch code.';
        else if (!word && st.stage === 'closed') word = 'Stopped';
        if (!word && st.kas != null) word = fmt(st.kas) + ' KAS';
        return (
          '<button class="item' +
          (hot ? ' hot' : '') +
          '" data-go="watched" data-params="' +
          esc(JSON.stringify({ id: w.id })) +
          '"><span><span class="t">' +
          esc(w.name || 'Someone') +
          '</span><span class="d role">' +
          esc(rolesShort(w.roles)) +
          '</span><span class="d">' +
          esc(word) +
          '</span></span></button>'
        );
      })
      .join('');
  }
  function watchedLine(w) {
    var st = w.st;
    if (!st) return 'Reading the account…';
    if (st.stage === 'waiting' && st.pending)
      return 'A withdrawal of ' + fmt(st.pending.kas) + ' KAS is waiting. It goes through in ' + dur(st.pending.ends_in_seconds) + '.';
    if (st.stage === 'stopped')
      return 'This account was closed. If ' + (w.name || 'the owner') +
        ' changed their rules, the money is in their new account: ask them for a new watch code.';
    if (st.stage === 'closed')
      return 'This account was closed. Ask the owner for a new watch code.';
    var l = fmt(st.kas) + ' KAS\nNothing waiting.';
    if (st.can_claim) return l + '\nThe inactivity period has run out: you can claim it.';
    if (!(w.roles && w.roles.heir && st.heir_seconds_left != null)) return l;
    /* the phone's count ran out but the network has not caught up yet: never "0 seconds left" */
    var almost =
      '\nAlmost ready: the inactivity period ends in a few minutes, then you can claim it.';
    if (st.heir_seconds_left <= 0) return l + almost;
    return l + '\nHeir Unlock: ' + dur(st.heir_seconds_left) + ' left.';
  }
  /* the short role line on a watched row, in the role colour */
  function rolesShort(r) {
    var g = r && r.guardian,
      h = r && r.heir;
    return [g ? 'Guardian' : '', h ? 'Heir' : ''].filter(Boolean).join(', ');
  }
  function rolesWord(r) {
    var g = r && r.guardian,
      h = r && r.heir;
    if (g && h) return 'You are their guardian (Emergency Stop ' + g + ') and their heir.';
    if (g) return 'You are their guardian (Emergency Stop ' + g + ').';
    return h ? 'You are their heir.' : 'View only.';
  }
  function watchedList() {
    return D.state.watched || []; /* Sample: demo.js rows */
  }
  function watchedName(w) {
    return (w && w.name ? w.name : 'Someone') + '’s account';
  }
  /* Home's rows for the accounts this phone helps: folded to the name, the role and one line
     (read more only if you want); a tap opens the whole status and its button. A row with
     something waiting (a withdrawal, a claim) is always open */
  function watchedRowsHtml(onlyHot) {
    ui.wOpen = ui.wOpen || {};
    return watchedList()
      .map(function (w) {
        var st = w.st || {},
          hot = st.stage === 'waiting',
          open = hot || st.can_claim || ui.wOpen[w.id],
          act = hot && st.can_stop ? 'Stop' : st.can_claim ? 'Claim' : 'Open',
          lines = watchedLine(w);
        if (onlyHot && !(hot || st.can_claim)) return '';
        return (
          '<div class="item' + (hot ? ' hot' : '') + '"><button class="rowmain" data-wx="' +
          esc(w.id) + '" aria-expanded="' + !!open + '"><span><span class="t">' +
          esc(watchedName(w)) + '</span><span class="d role">' + esc(rolesShort(w.roles)) +
          '</span><span class="d lines">' +
          esc(open ? lines : lines.split('\n')[0]) + '</span></span></button>' +
          (open ? '<button class="r go" data-go="watched" data-params="' +
            esc(JSON.stringify({ id: w.id })) + '">' + act + '</button>' : '') + '</div>'
        );
      })
      .join('');
  }
  /* You guard: what a claim paid to this phone's heir address (heir.purse), shown only when there
     is something */
  function heirPurseRowHtml() {
    var h = ui.heirPurse;
    if (!h || !(h.kas > 0)) return '';
    return (
      '<div class="label mt20">Received as heir</div><div class="list"><button class="item" data-go="heirpurse"><span><span class="t">' +
      fmt(h.kas) +
      ' KAS</span><span class="d">Funds you claimed. Tap to send them on.</span></span></button></div>'
    );
  }
  function peopleLiveHtml() {
    return (
      '<div class="top"><span class="label">People</span>' +
      tag() +
      menuBtn() +
      '</div><div class="treegap short"></div>' +
      '<div class="label mt20">Your guardians and heir</div><div data-helpfold></div><div class="list" data-helpers></div>' +
      '<button class="door" data-addhelper>+ Add a guardian or heir</button>' +
      '<button class="ghost" data-go="watchcode">Share watch code</button>' +
      '<div class="label mt20">People you protect</div><div data-protfold></div><div class="list" data-helped></div>' +
      '<button class="door" data-addwatch>+ Protect someone</button><div data-heirpurse></div>'
    );
  }
  function wirePeopleLive(el) {
    ui.pub = ui.pub || null;
    if (D.isLocal()) {
      /* Sample has no engine to ask; its rows come from demo.js */
      D.keysPublic().then(function (r) {
        if (r && r.ok) {
          ui.pub = r.public || {};
          ui.stops = r.stops || null;
        }
        refresh();
      });
      D.heirPurse().then(function (r) {
        ui.heirPurse = r && r.ok ? r : null;
        refresh();
      });
    }
    D.loadWatched();
    el.querySelector('[data-addhelper]').onclick = scanLink;
    el.querySelector('[data-addwatch]').onclick = function () {
      go('linkcode', { guide: true });
    };
    el.querySelector('[data-helpers]').addEventListener('click', function (e) {
      var b = e.target.closest('[data-hi]');
      if (b) openHelperSheet(helperRows()[+b.dataset.hi]);
    });
  }
  /* People folds each list to one line (names and roles; the count); with nobody yet the list
     and its door show as before. Share watch code always shows */
  function helperSummary(rows) {
    var by = {},
      order = [];
    rows.forEach(function (h) {
      var w = h.role === 'guardian' ? 'Guardian' : h.role === 'heir' ? 'Heir' : '';
      if (!by[h.name]) {
        by[h.name] = [];
        order.push(h.name);
      }
      if (w && by[h.name].indexOf(w) < 0) by[h.name].push(w);
    });
    return order.map(function (nm) {
      return by[nm].length ? nm + ': ' + by[nm].join(', ') : nm;
    }).join(' \u00b7 ') + (D.state.peopleNext ? ' \u00b7 Change pending' : '');
  }
  function livePeopleLive(el) {
    var ready = ui.pub || !D.isLocal(),
      rows = ready ? helperRows().filter(function (h) {
        return h.role !== 'unused';
      }) : [],
      oh = !rows.length || foldOpen('helpers'),
      w = watchedList(),
      ow = !w.length || foldOpen('protect');
    setHtml(el, '[data-helpfold]', rows.length ? foldHead('helpers', esc(helperSummary(rows))) : '');
    el.querySelector('[data-helpers]').hidden = !oh;
    el.querySelector('[data-addhelper]').hidden = !oh;
    setHtml(el, '[data-helpers]', ready ? helperRowsHtml() : '<div class="sub">Loading…</div>');
    setHtml(el, '[data-protfold]', w.length ? foldHead('protect', esc(w.length + (w.length === 1 ? ' account: ' : ' accounts: ') +
      w.map(function (x) {
        return x.name || 'Someone';
      }).join(', '))) : '');
    setHtml(el, '[data-helped]', w.length ? watchedShortHtml(!ow) : '<div class="sub">None yet.</div>');
    el.querySelector('[data-addwatch]').hidden = !ow;
    setHtml(el, '[data-heirpurse]', heirPurseRowHtml());
  }
  function scanSays(title, err) {
    openSheet('<div class="label">' + esc(title) + '</div><div class="err mt8">' + esc(err) + '</div><button class="ghost" data-no>Close</button>');
  }
  /* the owner scans a helper's link code (link.read), then picks each role */
  /* the guided add (People's first door): 1 you scan their link code, 2 you choose their
     role and Update now, 3 you show your watch code (ui.guide holds their name until then) */
  function scanLink() {
    ui.guide = null;
    ui.onScan = function (text) {
      back();
      D.readLink(text).then(function (r) {
        if (!r.ok) return scanSays('Link code', r.error || 'That is not a link code.');
        openPickRoles(r);
      });
    };
    go('scan', { what: 'answer', step: 'Step 1 of 3 \u00b7 You scan', head: 'Scan their code',
      say: 'Ask them to tap Protect someone. Point your camera at the code on their phone.' });
  }
  /* the button a new guardian takes: one nobody holds first, then the paper stop's,
     then a guardian's (2 on a tie) */
  function guardianSlot() {
    var rank = { paper: 1, guardian: 2 },
      r2 = rank[slotHolder('cancel2_x')] || 0,
      r3 = rank[slotHolder('cancel3_x')] || 0;
    return r3 < r2 ? 3 : 2;
  }
  function slotNote(which) {
    var h = slotHolder(which === 3 ? 'cancel3_x' : 'cancel2_x');
    if (h === 'paper') return ', in place of your paper stop';
    return h === 'guardian' ? ', in place of who holds it now' : '';
  }
  function openPickRoles(r) {
    var pb = ui.pub || {},
      al = r.already || {},
      which = al.guardian || guardianSlot(),
      on = { g: !al.guardian, h: !al.heir };
    if (al.guardian && al.heir) return scanSays(r.name, r.name + ' is already your guardian and your heir.');
    function item(k, t, d) {
      if (k === 'g' ? al.guardian : al.heir) return '<div class="item"><span><span class="t">' + t + '</span><span class="d">Already.</span></span></div>';
      return (
        '<button class="item on" data-role="' + k + '"><span><span class="t">' + t + '</span><span class="d">' + d + '</span></span><span class="r go" data-yes>Yes</span></button>'
      );
    }
    openSheet(
      '<div class="label">Step 2 of 3 \u00b7 You choose</div><div class="line mt8">Tap to choose what ' +
        esc(r.name) +
        ' can be.</div><div class="list">' +
        item(
          'g',
          'Your guardian',
          'Holds Emergency Stop ' + which + slotNote(which) +
            '. A stop only ever sends your money to your own vault.'
        ) +
        item(
          'h',
          'Your heir',
          'After ' + esc(heirSilence()) +
            ' with no payment from you, they can claim what is in your account.' +
            (pb.heir_x || pb.heir_address ? ' In place of your heir now.' : '')
        ) +
        '</div><div class="err" data-pe></div><button class="pill" data-pok>Review the change</button><button class="ghost" data-no>Not now</button>'
    );
    [].forEach.call(sheet.querySelectorAll('[data-role]'), function (b) {
      b.onclick = function () {
        on[b.dataset.role] = !on[b.dataset.role];
        b.classList.toggle('on', on[b.dataset.role]);
        b.querySelector('[data-yes]').textContent = on[b.dataset.role] ? 'Yes' : 'No';
      };
    });
    sheet.querySelector('[data-pok]').onclick = function () {
      if (!on.g && !on.h) return setText(sheet, '[data-pe]', 'Choose at least one.');
      var a = {},
        lines = [];
      if (on.g) {
        a.guardian_x = r.stop_x;
        a.guardian_which = which;
        D.nameHelper(r.stop_x, r.name || 'Guardian');
        lines.push(r.name + ' becomes your guardian (Emergency Stop ' + which + ').');
      }
      if (on.h) {
        a.heir_x = r.heir_x;
        D.nameHelper(r.heir_x, r.name);
        lines.push(r.name + ' becomes your heir.');
      }
      ui.guide = r.name || 'them';
      peopleChange(lines, a);
    };
  }
  function removeHelper(h) {
    if (!h) return;
    var g = h.role === 'guardian';
    confirmAction(
      'Remove ' + h.name,
      g ? 'Their copy of Emergency Stop ' + h.which + ' stops working.' :
        'They can no longer claim your account after the inactivity period.'
    ).then(function (yes) {
      if (!yes) return;
      peopleChange(
        [h.name + (g ? ' is no longer your guardian.' : ' is no longer your heir.')],
        g ? { guardian_x: '', guardian_which: h.which } : { heir_x: '' }
      );
    });
  }
  /* a guardian or heir lives in the rules: rules.change stores it, then the usual review sheet and Update now */
  function peopleChange(lines, args) {
    D.changeRules(args).then(function (r) {
      rulesReviewSheet(lines, r || {});
    });
  }
  /* the helper scans an owner's watch code (watch.import) */
  function scanWatch() {
    ui.onScan = function (text) {
      back();
      D.watchImport(text).then(function (r) {
        if (!r.ok) return scanSays('Watch code', r.error || 'That is not a watch code.');
        haptic();
        D.addHistory('Now watching ' + watchedName(r) + '. ' + rolesWord(r.roles), 'people');
        go('watched', { id: r.id }, { root: true });
        protectDone(r);
      });
    };
    go('scan', { what: 'answer', step: 'Step 2 of 3 \u00b7 You scan',
      head: 'They are choosing your role',
      say: 'When they have saved it, their phone shows a code. Point your camera at it.' });
  }
  /* the helper's last step: what they can do now, in plain words */
  function protectDone(r) {
    var who = r.name || 'them',
      g = r.roles && r.roles.guardian,
      h = r.roles && r.roles.heir;
    openSheet(
      '<div class="label">Done</div><div class="line mt8">You now protect ' + esc(who) + '. ' +
        esc(rolesWord(r.roles)) + '</div>' +
        (g ? '<div class="sub mt8">Guardian: if their phone is lost or taken, you can press ' +
          'emergency stop button ' + esc(String(g)) + '.</div>' : '') +
        (h ? '<div class="sub mt8">Heir: if they stop using their account for the inactivity ' +
          'period, you can claim it.</div>' : '') +
        '<div class="sub mt8">You can see their account here. You cannot move their funds.</div>' +
        '<button class="pill" data-no>See ' + esc(watchedName(r)) + '</button>'
    );
  }
  function wordsHtml(w) {
    return (
      '<div class="words">' +
      w
        .map(function (x, i) {
          return '<div><i>' + (i + 1) + '</i>' + esc(x) + '</div>';
        })
        .join('') +
      '</div>'
    );
  }
  /* the helper's first link code: a name, one finger, then the heir sheet */
  function openMakeLink(el) {
    openSheet(
      '<div class="label">Your link code</div><div class="line mt8">It lets someone make you their guardian, their heir, or both. Nothing in it is secret, so it is safe to show.</div>' +
        '<div class="sub">This phone makes two new keys for it and keeps them in its safe store. The heir key also goes on paper as your heir seed: 24 ' +
        'words. If this phone is lost, that seed is the only way to claim.</div>' +
        '<label class="field"><span>Your name, as they will see it</span><input data-ln maxlength="40"></label><div class="err" data-le></div>' +
        '<button class="pill" data-lok>Make my link code</button><button class="ghost" data-no>Not now</button>'
    );
    var b = sheet.querySelector('[data-lok]');
    b.onclick = function () {
      var v = sheet.querySelector('[data-ln]').value.trim();
      if (!v) return setText(sheet, '[data-le]', 'Type your name.');
      b.disabled = true;
      D.makeLink(v).then(function (r) {
        b.disabled = false;
        if (!r.ok) return setText(sheet, '[data-le]', r.error || 'The link code could not be made.');
        ui.closeSheet();
        ui.heirWords = r.heir_words || null;
        go('heirsheet', { first: true }, { replace: true });
      });
    };
    if (el) setText(el, '[data-e]', '');
  }
  /* the watched account's buttons: Stop and Send it back (guardian), Claim (heir) */
  function watchedButtons(w) {
    var st = (w && w.st) || {},
      b = '';
    if (st.can_stop && st.stage === 'waiting') b += '<button class="pill" data-wd>Send the withdrawal back</button>';
    if (st.can_stop) b += '<button class="pill danger" data-ws>Stop their account</button>';
    if (st.can_claim) b += '<button class="pill" data-wc>Claim it</button>';
    return b;
  }
  function watchedMove(el, btn, w, kind) {
    var M = {
      ws: ['Stop ' + watchedName(w),
        'Their money goes to their own vault. Only they can open it again.', true, 'Slide to stop',
        'stop', 'watchStop'],
      wd: ['Send the withdrawal back', 'The waiting withdrawal goes back into their account.', false, 'Slide to send it back', 'send', 'watchDecline'],
      wc: ['Claim ' + watchedName(w), 'Everything in it comes to you.', false, 'Slide to claim', 'send', 'watchClaim'],
    }[kind];
    armSlide(btn, M[0], M[1], M[2], M[3], M[4], function () {
      D[M[5]](w.id).then(function (r) {
        if (!r.ok) {
          setText(el, '[data-e]', r.error || 'The action failed. No funds were moved.');
          resetSlide(el);
          return;
        }
        haptic();
        D.addHistory(M[0] + ': done.', 'people');
        openSheet('<div class="label">' + esc(M[0]) +
          '</div><div class="line mt8">Submitted. It will show on the account shortly.</div>' +
            '<button class="ghost" data-no>Close</button>');
      });
    });
  }
  function openHeirSheet() {
    if (D.isLocal()) {
      if (ui.closeSheet) ui.closeSheet();
      return go('people', {});
    } /* on the phone the heir comes from a link code (People) */
    var has = !!D.state.heir;
    openSheet(
      '<div class="label">' +
        (has ? 'Change the heir' : 'Name an heir') +
        '</div><div class="line mt8">Your heir can claim what is in an account after it hears nothing from you for the inactivity period set on it. Never ' +
        'what is in your vault.</div>' +
        '<div class="sub">Your heir can be changed while no account holds money.</div>' +
        '<label class="field"><span>Your heir</span><input data-heir-addr autocapitalize="none" spellcheck="false" placeholder="kaspa: address of your heir"></label><div class="err" data-herr></div>' +
        '<button class="pill" data-heir-ok>' +
        (has ? 'Change the heir' : 'Name the heir') +
        '</button>' +
        (has ? '<button class="ghost" data-heir-rm>Remove the heir</button>' : '') +
        '<button class="ghost" data-no>Not now</button>'
    );
    var inp = sheet.querySelector('[data-heir-addr]'),
      okb = sheet.querySelector('[data-heir-ok]'),
      rm = sheet.querySelector('[data-heir-rm]');
    function send(v) {
      okb.disabled = true;
      if (rm) rm.disabled = true;
      setText(sheet, '[data-herr]', '');
      D.setHeir(v).then(function (r) {
        okb.disabled = false;
        if (rm) rm.disabled = false;
        if (!r.ok) {
          setText(sheet, '[data-herr]', r.error);
          return;
        }
        haptic();
        openSheet(
          '<div class="label">Your heir</div><div class="line mt8">' +
            (v === '' ? 'The heir is removed.' : 'Your heir is set.') +
            '</div><button class="ghost" data-no>Close</button>'
        );
        refresh();
        applyScene();
      });
    }
    okb.onclick = function () {
      var v = inp.value.trim();
      if (!v) {
        setText(sheet, '[data-herr]', 'Type the address of your heir.');
        return;
      }
      send(v);
    };
    if (rm)
      rm.onclick = function () {
        send('');
      };
  }
  /* fundacct opens the current account again after a rules change: asked for (reopen) or new rules waiting on it */
  function reopening(p) {
    return D.isLocal() && !(p && typeof p.account === 'number') && !!(p && p.reopen);
  }
  /* fundacct: what the money comes under; folded under Details in step 3 of a rules change */
  function fundIntro(an, re, rw) {
    return (
      '<p class="sub">Money moves from the purse into ' +
      (an ? 'account ' + an : 'your account') +
      ' and comes under its rules: the daily limit, the notice period, the emergency stop and your heir.</p>' +
      '<div class="sub" data-rules>' +
      (re
        ? rw
          ? 'The new rules take effect now:' + rw
          : 'The new rules you set take effect now.'
        : D.isMainnet()
          ? 'Rules on this account: ' +
            fmt(ui.liveRules.limit) +
            ' KAS a day without waiting, payments larger than that wait ' +
            dur(ui.liveRules.notice * 3600) +
            ', the limit refreshes every ' +
            dur(ui.liveRules.window * 3600) +
            ', 1 KAS always stays in it. Tap a bar on Home to change them before opening.'
          : 'Test dials: minutes instead of days.') +
      '</div>'
    );
  }
  function openChangeRules(what, detail, onDo) {
    var s = acct(),
      live = D.isLive(),
      keys = (s.sheets || 1) === 1 ? 'your vault seed' : 'two of your three keys';
    openSheet(
      '<div class="label">Change the rules</div><div class="line mt8">' +
        esc(what) +
        '.</div>' +
        (detail ? '<div class="sub">' + esc(detail) + '</div>' : '') +
        '<div class="sub">The rules of this account live in the account itself, on the Kaspa network. They cannot be edited in place.</div>' +
        '<div class="line mt8">Your money moves under the new rules. Up to the daily limit (' +
        fmt(s.limit) +
        ' KAS) moves immediately; the rest waits the notice period (' +
        dur(noticeOf(s)) +
        ').</div>' +
        '<div class="sub">Or press the emergency stop and open the new account from your vault with ' +
        keys +
        '.</div>' +
        (live
          ? '<div class="sub">' + 'This change cannot be made from here.' + '</div>'
          : '<button class="pill" data-move>Move my money</button>') +
        '<button class="ghost" data-no>Keep the rules</button>'
    );
    var mv = sheet.querySelector('[data-move]');
    if (mv)
      mv.onclick = function () {
        confirmAction('Change the rules', what + '.').then(function (yes) {
          if (!yes) return;
          ui.closeSheet();
          D.addHistory('Rules changed: ' + what + '. Money moved to the new account.', 'protection');
          if (onDo) onDo();
          if (KVScene.pulse) KVScene.pulse('send');
          refresh();
          applyScene();
        });
      };
  }
  /* an account with no covenant (Everyday): the bars offer to put one on, with the standard rules; every rule is then a dial on its bar (the old all-in-one editor is gone) */
  var FRESH_RULES = { limit: 100, notice: 24, window: 24, heir: 8760 };
  function openProtect() {
    var s = acct(),
      name = s && s.name ? s.name : 'This account';
    openSheet(
      '<div class="label">No covenant</div><div class="line mt8">' +
        esc(name) +
        ' has no covenant: payments leave at once, with no limit, no notice period and no emergency stop.</div><div class="sub">Add a covenant to turn on ' +
        'all five rules: 100 KAS a day available immediately; larger payments clear after a 24 h notice period. Change any rule afterwards from its ' +
        'bar.</div>' +
        (D.isLive()
          ? '<div class="sub">' + 'Your account is already under a covenant.</div>'
          : '<button class="pill" data-save>Protect this account</button>') +
        '<button class="ghost" data-no>Close</button>'
    );
    var sv = sheet.querySelector('[data-save]');
    if (sv)
      sv.onclick = function () {
        confirmAction('Protect this account', 'The rules start now.').then(function (yes) {
          if (!yes) return;
          applyRulesTo(ui.acct, FRESH_RULES);
          ui.closeSheet();
          if (KVScene.pulse) KVScene.pulse('send');
          go(
            'covenantset',
            {
              text:
                fmt(FRESH_RULES.limit) +
                ' KAS a day available immediately; larger payments clear after a ' +
                dur(FRESH_RULES.notice * 3600) +
                ' notice period.',
            },
            { root: true }
          );
        });
      };
  }
  /* Library: every word has a plain line and a technical one. First section is this app's own words; the rest is crypto and Kaspa. */
  var LIBRARY = [
    {
      title: 'This app',
      items: [
        {
          term: 'Everyday account',
          plain: 'An account with no covenant, for spending and receiving. It has no limit and no stop.',
          tech:
            "A plain pay-to-public-key address. Any coin at it is spent by one signature from this phone's key, with no script rules.",
        },
        {
          term: 'Protected account',
          plain: 'An account with a covenant on it: a daily limit, a notice period and an emergency stop.',
          tech: 'One covenant coin whose script carries the rules and the state. Every spend must renew it under the same covenant id; see Lineage.',
        },
        {
          term: 'KAS',
          plain: 'Kaspa, the digital money this app holds. Amounts show in KAS; the network counts in sompi.',
          tech: '1 KAS = 100,000,000 sompi. Balances and prices are display conversions; every script rule compares whole numbers of sompi.',
        },
        {
          term: 'Large payment',
          plain:
            'Any payment above your daily limit. It waits the notice period before it leaves, and you can stop it in that time. The limit bar on Home sets where bigger starts.',
          tech: 'A spend whose amount plus fee would push the window total past the limit fails the spend entry. It must go through request, then finish once the notice period has elapsed.',
        },
        {
          term: 'Pending withdrawal',
          plain: 'A large payment waiting out its notice period. You can still stop it or decline it.',
          tech: "The coin's state holds the request's destination hash, amount and DAA score. Finish, cancel and decline are the only entries that clear it.",
        },
        {
          term: 'Emergency stop',
          plain: 'A button that moves all your money into your vault at once, even while a withdrawal is pending.',
          tech: "The app's name for the cancel entry: a stop key signature plus one output paying the whole coin, less at most maxFee, to the vault's script hash.",
        },
        {
          term: 'Vault',
          plain:
            'Where your money goes after a stop. Nobody can spend from it with a phone alone; it takes two of your three vault seeds, typed from paper.',
          tech: "The app's name for the vault address: a 2-of-3 multisig script whose hash the covenant commits to at creation.",
        },
        {
          term: 'Vault seed',
          plain:
            'The 24 words that open your vault. Three vault seeds are made and any two open it. They live on paper; this phone keeps a sealed copy until you ' +
            'delete it.',
          tech: 'Three BIP-39 mnemonics, one per vault key. The covenant stores only the hash of the 2-of-3 script, so the words are needed only to spend from the vault.',
        },
        {
          term: 'Recovery seed',
          plain:
            'In Sample, the words that bring the account back on a new phone. An account made on this phone ' +
            'has none: if the phone is lost, a guardian\u2019s stop moves it to your vault.',
          tech: "A BIP-39 mnemonic for the hot keys: the Everyday address, the card key and this phone's stop key. The covenant coin is found again on the network by its covenant id.",
        },
        {
          term: 'Guardian',
          plain:
            'Someone you trust who holds emergency stop button 2 or 3. With your watch code they see the account and can stop it or send a withdrawal back, ' +
            'never spend.',
          tech:
            'The holder of stop key 2 or 3, set from their link code by a rules change. That key can sign only the cancel and decline entries.',
        },
        {
          term: 'Sponsored',
          plain: 'A covenant for a newcomer. They spend, and you hold their stop until they are ready to take over.',
          tech: 'The same script with a different key set: the card key belongs to the newcomer, the stop key to the sponsor.',
        },
        {
          term: 'Allowance',
          plain: 'A covenant for a family member. They spend up to a limit you choose, and you keep the stop.',
          tech: "A covenant with a low daily limit, the family member's card key and your stop key. The network enforces the limit, not the app.",
        },
        {
          term: 'Panic PIN',
          plain:
            'A PIN you type on the unlock screen when someone makes you open the app. The app opens showing only your purse.',
          tech:
            'Six digits kept on this phone as a salted SHA-256, never as the digits. Matching it ' +
            'opens the duress view; the page then refuses every action except a payment from the purse.',
        },
        {
          term: 'Duress view',
          plain:
            'The app as the panic PIN shows it: the purse only. Your account, vault, stops, people and rules are out of sight.',
          tech:
            'It lasts until the app is closed and started again. No rule on the network changes, so the daily limit, ' +
            'the notice period and the emergency stops still hold.',
        },
      ],
    },
    {
      title: 'Money on a network',
      items: [
        {
          term: 'Coin',
          plain:
            'A coin is one lump of Kaspa sitting at an address. Your balance is the sum of your coins, and a payment spends whole coins and gives change back.',
          tech: 'A coin is an unspent transaction output (UTXO): an amount in sompi plus a script that says who may spend it. It is consumed entirely when spent.',
        },
        {
          term: 'Address',
          plain:
            'An address is the string you give someone so they can pay you. It is derived from your key, so money sent there can only be spent by that key.',
          tech: 'A Kaspa address is a bech32-style string with a network prefix, one version byte and a 32-byte public key (or a 32-byte script hash), with a checksum that catches typing errors.',
        },
        {
          term: 'Private key',
          plain:
            'The private key is the secret that lets you spend. Whoever holds it holds the money. It never leaves the device that made it.',
          tech: 'A private key is a random 256-bit number on the secp256k1 curve. Signing with it proves ownership without revealing it; there is no recovery if it is lost.',
        },
        {
          term: 'Public key',
          plain:
            'The public key is the part you can show. It is made from the private key, but nobody can work backwards from it to the secret.',
          tech: 'The public key is the curve point private key times the generator G. Kaspa uses the 32-byte x-only form, which is what a kaspa:q address encodes.',
        },
        {
          term: 'Signature',
          plain:
            'A signature is the proof attached to a payment that the key holder approved it. The network checks it; nobody can forge one without the private key.',
          tech: 'Kaspa uses 64-byte Schnorr signatures (BIP-340 style) over a hash of the transaction. Any change to the transaction after signing makes the signature fail.',
        },
        {
          term: 'Seed words',
          plain:
            "Seed words are 12 or 24 ordinary words that encode an account's master secret. Anyone with the words can rebuild every key, so they are kept on " +
            "paper only.",
          tech: 'BIP-39 mnemonic: 128 or 256 bits of randomness plus a checksum mapped to a 2048-word list. The words are stretched with PBKDF2 into a 512-bit seed.',
        },
        {
          term: 'Derivation',
          plain:
            'Derivation is how one seed grows into many keys and addresses in a fixed order, so the same words on a new device find the same money.',
          tech: "BIP-32 hierarchical derivation along the BIP-44 path m/44'/111111'/account'/change/index, where 111111 is Kaspa's registered coin type. Each index yields one key pair.",
        },
        {
          term: 'Wallet',
          plain:
            'A wallet is software that keeps your keys, watches the network for your coins, builds payments and signs them. It does not hold money; the network does.',
          tech: 'A wallet tracks the UTXOs of its derived addresses through a node, assembles transactions from them, signs inputs with the matching private keys and submits the result.',
        },
        {
          term: 'Node',
          plain:
            'A node is a computer running the Kaspa program. It receives every block, checks every rule and keeps the current list of who owns what. Thousands of them agree.',
          tech: 'A node (kaspad, now the Rust implementation) validates blocks and transactions, maintains the blockDAG and UTXO set, relays data to peers and serves wallets over RPC.',
        },
        {
          term: 'Transaction',
          plain:
            'A transaction is a signed instruction: take these coins, create those new coins for these addresses. Once it is in a block, it cannot be undone.',
          tech: 'A transaction lists inputs (references to UTXOs plus signatures) and outputs (amount plus script). Inputs must cover outputs; the difference is the fee. Its id is a hash.',
        },
        {
          term: 'Fee',
          plain:
            'The fee is a tiny amount paid to miners for including your payment. On Kaspa a normal payment costs a fraction of a cent.',
          tech: 'Fee equals inputs minus outputs, paid in sompi. Priority is fee per unit of mass, a size and compute measure; a plain payment weighs a few thousand grams.',
        },
        {
          term: 'Confirmation',
          plain:
            'A confirmation is a block that includes or builds on your payment. Each one makes reversing it harder. On Kaspa this happens in about a second.',
          tech: 'Confirmations count blocks in the selected chain at or past the block that accepted the transaction. At ten blocks per second, ten confirmations arrive in about one second.',
        },
      ],
    },
    {
      title: 'How Kaspa works',
      items: [
        {
          term: 'BlockDAG',
          plain:
            'Most networks keep a single chain of blocks and throw away ones made at the same time. Kaspa keeps them all, linked in a web, so nothing is wasted.',
          tech: 'The blockDAG is a directed acyclic graph where each block references all tips it saw, not one parent. Parallel blocks are merged, not orphaned, allowing high block rates.',
        },
        {
          term: 'GHOSTDAG',
          plain:
            'GHOSTDAG is the rule that puts every block in the web into one agreed order, so all nodes read the same history even when blocks arrive together.',
          tech: 'GHOSTDAG partitions blocks into a well-connected blue set and a red set, bounded by parameter k, and derives a total order along the selected chain from blue blocks.',
        },
        {
          term: 'Blue score',
          plain:
            "Blue score is a block's position in the agreed order: how many trusted blocks came before it. A higher score means a later, deeper-buried block.",
          tech: "Blue score is the number of blue blocks in a block's past under GHOSTDAG. The selected parent is the one with the highest blue score; it drives ordering.",
        },
        {
          term: 'DAA score',
          plain:
            "DAA score is Kaspa's clock. It counts blocks, about ten a second, so a wait of one day is written as a number of blocks rather than hours.",
          tech: "The DAA (difficulty adjustment) score counts blocks in a block's past within the difficulty window. It rises roughly 10 per second and is used for time locks.",
        },
        {
          term: 'Block rate: 10 per second',
          plain:
            'Since the Crescendo upgrade in May 2025, Kaspa makes ten blocks every second. Payments show up almost instantly and the network stays uncrowded.',
          tech: 'Crescendo (activated 5 May 2025) raised the block rate from 1 to 10 blocks per second, so target block time is 100 ms. Per-block reward fell tenfold to keep emission steady.',
        },
        {
          term: 'Proof of work: kHeavyHash',
          plain:
            'Miners race to solve a maths puzzle to make each block. The work is costly, which is what makes rewriting history impractical.',
          tech: 'kHeavyHash hashes the block header, multiplies the result by a 64x64 matrix derived from the header, then hashes again. A valid block hash must fall below the current target.',
        },
        {
          term: 'Pruning',
          plain:
            'A node does not have to keep every block forever. Old block bodies are dropped after a few days; the list of current coins is kept, so nothing owned is lost.',
          tech: 'Blocks older than the pruning depth are discarded and replaced by a pruning point with a committed UTXO set, so a node needs only recent history plus the current state.',
        },
        {
          term: 'UTXO model',
          plain:
            'Kaspa does not keep account balances. It keeps a list of unspent coins. Your balance is just the coins at your addresses added up.',
          tech: 'The UTXO set maps each outpoint (transaction id, index) to an amount and script. A transaction consumes inputs and creates outputs atomically; double spends fail validation.',
        },
        {
          term: 'Sompi',
          plain: 'A sompi is the smallest piece of Kaspa, like a cent to a dollar but much smaller. One KAS is one hundred million sompi.',
          tech: '1 KAS = 100,000,000 sompi (10^8). All on-chain amounts, fees and script rules are integers in sompi; the wallet converts to KAS only for display.',
        },
        {
          term: 'Supply and emission',
          plain:
            'New Kaspa is created only as block rewards, and the amount shrinks every month on a fixed schedule. There is a hard ceiling near 28.7 billion KAS.',
          tech: 'Emission falls by the twelfth root of two each month, halving yearly, toward a cap of about 28.7 billion KAS. Rewards are paid per block to the miner; no premine.',
        },
        {
          term: 'kaspa: and kaspatest: addresses',
          plain:
            'Real-money addresses start with kaspa:. Test-network addresses start with kaspatest: and hold coins with no value. This app refuses to mix the ' +
            'two.',
          tech: 'The prefix is part of the bech32 encoding and checksum, so a kaspatest: address is invalid on mainnet and vice versa. Both carry the same version byte and key or script payload.',
        },
        {
          term: 'Mempool',
          plain:
            "The mempool is a node's waiting room: payments that have been sent but not yet put in a block. On Kaspa the wait is usually under a second.",
          tech: 'Each node keeps its own mempool of valid unconfirmed transactions, ordered by fee rate, and relays them. Miners draw block contents from it; conflicting spends are rejected.',
        },
        {
          term: 'Reorg',
          plain:
            'A reorg is when the agreed order of the newest blocks shifts slightly as late blocks arrive. It is normal and brief, and old history is not affected.',
          tech: 'In the blockDAG a reorg changes the selected chain over the most recent blocks when a heavier branch appears. Blocks are reordered, not discarded; depth bounds its reach.',
        },
        {
          term: 'Finality in about ten seconds',
          plain:
            'After roughly ten seconds a payment is buried under about a hundred blocks and is treated as final. Rewriting it would need more mining power than exists.',
          tech: 'At ten blocks per second, ten seconds adds about 100 blocks of confirmation depth. Nodes also enforce a finality depth past which reorgs are refused outright.',
        },
      ],
    },
    {
      title: 'Rules on money',
      items: [
        {
          term: 'Covenant',
          plain:
            'A covenant is a rule attached to the coin itself, not to the app. Every node enforces it, so even a stolen key cannot break it.',
          tech: 'Since the Toccata upgrade (KIP-17) a script can inspect its own spending transaction and require outputs carrying the same script and state, so the rules follow the coin.',
        },
        {
          term: 'SilverScript',
          plain:
            "SilverScript is the language the vault's rules are written in. It is compiled once into the network's own script, which every node runs on each spend.",
          tech: 'SilverScript compiles to Kaspa script with introspection opcodes. Each entry (spend, request, finish, cancel, decline, topup, inherit) is a branch with its own require checks.',
        },
        {
          term: 'The daily limit',
          plain:
            'Payments up to the daily limit leave at once. The limit counts fees and refreshes a full day after your last payment, not at midnight.',
          tech: "The coin's state holds a rolling window start and spent total in DAA score and sompi. A spend passes only if total plus amount plus fee stays within the limit.",
        },
        {
          term: 'The notice period',
          plain:
            'A payment above the limit waits a set time, visible to you and your guardians, before it can leave. During the wait it can be stopped or declined.',
          tech: "A request writes destination hash and amount into the coin's state; finish is allowed only after the notice period, measured in DAA score, has elapsed since the request.",
        },
        {
          term: 'Stop buttons',
          plain:
            'An emergency stop button moves everything in the account into your vault, even while a withdrawal is pending. It can only send money there, ' +
            'nowhere else, so it is safe to give one to a guardian.',
          tech:
            'Cancel requires a stop key signature and an output paying the whole coin, less at most maxFee, to the fixed cold script hash. There are up to ' +
            'three stop keys; keys 2 and 3 go to guardians through a rules change.',
        },
        {
          term: 'Decline',
          plain:
            'Decline cancels a waiting withdrawal without moving any money. Use it when the request is wrong but there is no need to empty the account.',
          tech: 'Decline needs a stop key signature; it clears the pending destination and amount, leaves window counters unchanged and renews the coin under the same covenant.',
        },
        {
          term: 'Heir and the inactivity clock',
          plain:
            'An heir can claim the money in the account, never the vault, if it sits untouched for the inactivity period, six months by default. Any payment ' +
            'or action restarts the clock, and what the heir claims lands under Received as heir.',
          tech:
            'Inherit allows the heir key after inheritDelay in DAA score since the coin was last renewed. With autoHeir set, anyone may send the coin to the ' +
            'heir script once the inactivity period has passed.',
        },
        {
          term: '2 of 3',
          plain: 'The vault has three seeds on paper, and any two open it. Lose one and nothing is lost; a thief with one has nothing.',
          tech: 'The vault address is a 2-of-3 multisig script; the covenant commits to its hash. Spending from it needs two Schnorr signatures from the three vault keys.',
        },
        {
          term: 'Max fee and the minimum left behind',
          plain:
            'Two guard rails: no action may spend more than a small fixed fee, and every coin the rules create must be big enough to be worth spending later.',
          tech: 'maxFee caps inputs minus outputs on every entry. minRemainder requires each continuation or exit output to be at least that many sompi, so no unspendable dust is created.',
        },
        {
          term: 'Lineage',
          plain:
            "Every time the account's coin is spent, the leftover must go back into a coin carrying the same rules. The money cannot slip out of its own protection.",
          tech: "Each entry requires an output with this covenant's id and updated state. Only a single-output finish, cancel or inherit ends the lineage; outside coins can leave only by those exits.",
        },
      ],
    },
    {
      title: 'Keys and devices',
      items: [
        {
          term: 'Hot vs cold',
          plain:
            "Hot money can be spent from a device that talks to the internet; cold money cannot. Your account is hot, the vault is cold: it needs vault " +
            "seeds from paper. Cold is slower and much harder to steal.",
          tech:
            'A hot key signs on a networked device and is exposed to its software. The vault is a 2-of-3 script whose keys are rebuilt from paper seeds only ' +
            'for the one signing that takes money out.',
        },
        {
          term: 'Secure chip / Keystore',
          plain:
            "The phone's secure chip seals what this phone signs with, and the copy of your vault seeds, behind your fingerprint. Apps, backups and even a " +
            "copy of the phone's storage cannot read them.",
          tech: 'Android Keystore (StrongBox where present) stores keys non-exportable. The chip does ECDSA only, not Schnorr, so it seals the key and the app signs in memory while unlocked.',
        },
        {
          term: 'Fingerprint vs PIN',
          plain:
            'Fingerprint is fast and cannot be read over your shoulder; a PIN works when the sensor fails. Both only unlock the chip; neither is the key itself.',
          tech: 'Biometric or PIN authentication gates key use for a short validity window. A failed biometric falls back to the device credential; both are checked by the secure hardware.',
        },
        {
          term: 'Seedless signer',
          plain:
            'A seedless signer is a key with no written words behind it. It is born inside the chip or split across people; nothing exists to photograph or steal.',
          tech: "The key is generated in secure hardware and is non-exportable. Recovery comes from the covenant's other exits (stop, heir, vault) rather than from a mnemonic backup.",
        },
        {
          term: 'Hardware key',
          plain:
            'A hardware key is a small purpose-built device that holds a key and signs on request. It shows what it signs on its own screen.',
          tech: 'A dedicated signing device with its own secure element and display. It verifies and signs transactions internally and never exposes the private key to the host computer.',
        },
        {
          term: 'Paper backup',
          plain:
            'Paper holds your vault seeds. It cannot be hacked, but it can be lost in a fire or read, so keep the seeds in separate places and never ' +
            'photograph them.',
          tech:
            'A paper backup records BIP-39 words that regenerate a key. For the 2-of-3 vault, three seeds are stored apart; any two suffice, so one lost or ' +
            'stolen seed is harmless.',
        },
      ],
    },
    {
      title: 'Safety habits',
      items: [
        {
          term: 'Where your vault seeds live',
          plain:
            'On paper in three places, and sealed on this phone behind your fingerprint. Never in a photo, a note or a message: those are where thieves look ' +
            'first.',
          tech:
            'The phone seals the seeds with the Android Keystore and opens them for one signing after a fingerprint. Words typed from paper stay in memory ' +
            'for that signing only.',
        },
        {
          term: 'Check the address start and end',
          plain:
            'Before paying, compare the first and last several characters of the address against the one you were given. Malware swaps addresses; it rarely matches both ends.',
          tech: 'Address-swapping malware replaces clipboard contents with an attacker address. Verifying the first and last 6 to 8 characters, plus the kaspa: prefix, detects almost every swap.',
        },
        {
          term: 'Small test payment first',
          plain:
            "When paying a new address for a large amount, send a small amount first and confirm it arrived. Kaspa's fees and speed make this nearly free.",
          tech: 'A test transaction confirms the destination script accepts funds and the recipient controls it. Confirmation takes about a second and costs a few thousand sompi.',
        },
        {
          term: 'Two phones watching',
          plain:
            'This phone watches your money while it is on. A second phone or tablet can ' +
            'watch too, so the alert still comes if this phone is off, lost or lied to.',
          tech: "Two independent guards poll separate nodes for the covenant coin's state. A pending request seen by either triggers alerts, so one compromised node cannot hide a withdrawal.",
        },
        {
          term: 'What a stop costs you',
          plain:
            "A stop moves everything to your vault. Getting it back out means typing two vault seeds from paper, which takes a few minutes. If in doubt, use " +
            "the emergency stop.",
          tech:
            'Cancel pays the full coin, minus at most maxFee, to the cold script. Taking it out needs two of the three vault signatures, then a new topup ' +
            'into an account; nothing is lost but time.',
        },
        {
          term: 'What a thief can and cannot do',
          plain:
            'With your unlocked phone a thief can spend up to the daily limit, once. They cannot empty the account, change the rules or touch your vault.',
          tech: 'The covenant enforces the limit and notice period on every spend regardless of who signs. Bigger withdrawals are visible for the notice period and can be stopped or declined.',
        },
        {
          term: 'If your phone is lost',
          plain:
            'A guardian presses their stop button and your account moves to your vault; two of your three vault ' +
            'seeds open it. The purse has no backup, so keep it small. An heir can claim only after the ' +
            'inactivity period, and the funds go to the heir.',
          tech:
            'Any stop key holder can cancel the coin to the cold script. The lost phone holds only hot keys, sealed behind its fingerprint and bounded by ' +
            'the limit.',
        },
        {
          term: 'If a vault seed is lost',
          plain:
            'One lost vault seed is fine: any two of three still open the vault. Move the money to a new vault soon, so a second loss cannot lock you out.',
          tech:
            "A 2-of-3 script tolerates one missing key. Spend the cold coins to a new 2-of-3 script with three fresh keys, then point a fresh account's " +
            "covenant at it (Replace your keys).",
        },
      ],
    },
  ];
  STORIES.push([
    'Choosing a covenant',
    'A covenant is the same idea at the strength you choose. A big daily limit and a short notice period suit money you use all the time; a small limit and a long notice period suit savings you rarely touch. Every rule is a bar on Home: tap it to change that one rule. Accounts for other people, such as a newcomer you sponsor or a family member on an allowance, follow the same rules with their own numbers.',
  ]);
  function addHelp(el, name) {
    if (!HELP[name]) return;
    var first = el.firstElementChild,
      top = first && first.classList.contains('top') ? first : null;
    if (!top) {
      top = document.createElement('div');
      top.className = 'top';
      if (first && first.classList.contains('label')) top.appendChild(first);
      else top.innerHTML = '<span></span>';
      el.insertBefore(top, el.firstChild);
    }
    var right = top.querySelector('.topright');
    if (!right) {
      right = document.createElement('span');
      right.className = 'topright';
      while (top.children.length > 1) right.appendChild(top.children[1]);
      top.appendChild(right);
    }
    /* Home has its own; inner screens and the duress view have no menu */
    if (!SCR[name].tab || name === 'home' || ui.duress) return;
    /* the screen drew its own */
    var own = right.querySelector('[data-menu]');
    if (own) return void (own.onclick = openMenu);
    right.insertAdjacentHTML('beforeend', '<button class="menu" data-menu aria-label="Menu"><i></i><i></i><i></i></button>');
    right.querySelector('[data-menu]').onclick = openMenu;
  }
  function openHelp(name) {
    var t = HELP[name];
    if (!t) return;
    sheet.innerHTML =
      '<div class="pane"><div class="label">This screen</div><div class="line mt8">' +
      esc(t) +
      '</div>' +
      (name === 'howitworks' ? '' : '<button class="pill" data-story>Read the full story</button>') +
      '<button class="ghost" data-no>Close</button></div>';
    sheet.setAttribute('aria-hidden', 'false');
    void sheet.offsetWidth;
    sheet.classList.add('on');
    ui.closeSheet = function () {
      ui.closeSheet = null;
      sheet.classList.remove('on');
      sheet.setAttribute('aria-hidden', 'true');
    };
    sheet.querySelector('[data-no]').onclick = function () {
      ui.closeSheet();
    };
    var s = sheet.querySelector('[data-story]');
    if (s)
      s.onclick = function () {
        ui.closeSheet();
        go('howitworks', { step: (HELP_STEP[name] || 0) + 1 });
      };
  }

  /* ---- the explainer: an overlay with lines pointing at the things on this screen; tap a label to read ---- */
  var EXPLAINERS = {
    home: [
      ['[data-pick]', 'Account', 'Which account you are looking at. Tap it to switch, or to open another.'],
      ['[data-bal]', 'Balance', 'What this account holds. Tap it, or the arrows under it, to see it in dollars.'],
      ['[data-limit]', 'Today', 'What is still available immediately today, and when the limit refreshes.'],
      ['.pips', 'Your rules', 'The five rules on this money. Tap a bar to change that one rule.'],
      ['[data-log]', 'Activity',
        'Recent activity. Any payment still pending can be stopped from here.'],
      ['[data-tab="safe"]', 'Vault',
        'Where funds go after an emergency stop, and the emergency stop itself. Two of your ' +
        'vault seeds open it.'],
      ['[data-menu]', 'Menu', 'Alerts, your covenant, the explanations and the settings.'],
    ],
    send: [
      ['[data-to]', 'To', 'An address, a name from your payees, or scan a code.'],
      ['[data-amt]', 'Amount', 'In KAS. Up to your daily limit it goes at once; above it, it waits the notice period and can be stopped.'],
      [
        '[data-ok]',
        'Send',
        'Your fingerprint or PIN, then a slide. Up to your limit it is final within a second, unless the one-minute cancellation window is on.',
      ],
    ],
    receive: [
      ['.qrbox', 'Your code', 'Anyone can scan this to pay you.'],
      ['[data-addr]', 'Your address', 'Tap to see all of it. Send only Kaspa to it.'],
      ['[data-copy]', 'Copy', 'Puts the address on the clipboard.'],
      ['[data-share]', 'Share', 'Sends the address to someone.'],
    ],
    people: [
      ['[data-addhelper]', 'Add a guardian or heir',
        'Three steps: scan their code, choose their role and Update now, then show your code.'],
      ['[data-go="watchcode"]', 'Watch code',
        'Lets your guardians see this account. It cannot move money.'],
      ['[data-addwatch]', 'Protect someone',
        'Show them your code, then scan theirs once they have chosen your role.'],
    ],
    price: [['.chart', 'KAS price', 'One KAS in dollars over a day, a week or a month.']],
    settings: [
      ['.list', 'Money', 'Your rules, accounts, coins and statement.'],
      [
        '.list:nth-of-type(2)',
        'Keys',
        lazyText(function () {
          var t = keysHolds();
          return t.charAt(0).toUpperCase() + t.slice(1) + '.';
        }),
      ],
      ['.list:nth-of-type(3)', 'Help', 'How it works, the glossary, preferences and about.'],
    ],
  };
  function stepFor(name) {
    for (var k in STEP_PEEK) if (STEP_PEEK[k] === name) return +k;
    return 0;
  }
  function explainScreen() {
    var name = ui.cur && ui.cur.name,
      spec = EXPLAINERS[name];
    if (!spec || !ui.el) return;
    if (ui.stopExplain) ui.stopExplain();
    var els = [],
      host = ui.el;
    spec.forEach(function (x) {
      var el = host.querySelector(x[0]);
      if (el && el.getBoundingClientRect().width) {
        el.classList.add('xp');
        els.push([el, x[1], x[2]]);
      }
    });
    if (!els.length) return;
    var bar = document.getElementById('xbar');
    if (!bar) {
      bar = document.createElement('div');
      bar.id = 'xbar';
      document.body.appendChild(bar);
    }
    /* a stepped walk, not touches: Back / Next go line by line, the line it talks about lights up and the rest dims; a tap on a lit line still jumps to it */
    var at = 0;
    bar.innerHTML =
      '<div class="xtext" data-xt></div><div class="xnav"><span class="xcount" data-xc></span><button class="xbtn" data-xprev>Back</button><button class="xbtn" data-xnext>Next</button><button class="xdone" data-xdone>Done</button></div>';
    document.body.classList.add('explaining');
    function showStep(i) {
      at = Math.max(0, Math.min(els.length - 1, i));
      var hit = els[at];
      els.forEach(function (x) {
        x[0].classList.toggle('xp-on', x === hit);
      });
      setHtml(bar, '[data-xt]', '<b>' + esc(hit[1]) + '</b>' + esc(hit[2]));
      setText(bar, '[data-xc]', at + 1 + ' of ' + els.length);
      bar.querySelector('[data-xprev]').disabled = at === 0;
      bar.querySelector('[data-xnext]').textContent = at === els.length - 1 ? 'Done' : 'Next';
      try {
        hit[0].scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      } catch (e) {}
    }
    function onTap(e) {
      var hit = -1;
      els.forEach(function (x, i) {
        if (x[0] === e.target || x[0].contains(e.target)) hit = i;
      });
      if (hit < 0) return;
      e.stopPropagation();
      e.preventDefault();
      showStep(hit);
    }
    host.addEventListener('click', onTap, true);
    ui.stopExplain = function () {
      ui.stopExplain = null;
      document.body.classList.remove('explaining');
      els.forEach(function (x) {
        x[0].classList.remove('xp', 'xp-on');
      });
      host.removeEventListener('click', onTap, true);
      bar.innerHTML = '';
    };
    bar.querySelector('[data-xdone]').onclick = function () {
      if (ui.stopExplain) ui.stopExplain();
    };
    bar.querySelector('[data-xprev]').onclick = function () {
      haptic();
      showStep(at - 1);
    };
    bar.querySelector('[data-xnext]').onclick = function () {
      haptic();
      if (at === els.length - 1) {
        if (ui.stopExplain) ui.stopExplain();
      } else showStep(at + 1);
    };
    showStep(0);
  }

  /* ---- the shown account (each account uses its own covenant): one accessor, used by the bars, the dials and the covenant screen ---- */
  function acctById(id) {
    var s = D.state;
    if (id === 'main') return s;
    if (id === 'everyday') return s.everyday || null;
    return (
      (s.extra || []).filter(function (a) {
        return a.name === id;
      })[0] || null
    );
  }
  function acct() {
    return acctById(ui.acct) || D.state;
  }
  function noticeOf(a) {
    return a.noticeSeconds || (a.notice ? a.notice * 3600 : 86400);
  }
  /* the heir clock: seconds before the heir may claim, and the share of the silence already run (0..1); null when not known
     (no heir, an older engine, or Live without the whole delay). At 0 left the share is 1 whatever the delay. */
  function heirLeft() {
    var v = D.state.heirSecondsLeft;
    return typeof v === 'number' && isFinite(v) ? Math.max(0, v) : null;
  }
  function heirShare() {
    var l = heirLeft(),
      t = D.state.heirDelaySeconds;
    if (l == null) return null;
    if (l <= 0) return 1;
    if (typeof t !== 'number' || t <= 0) return null;
    return Math.max(0, Math.min(1, 1 - l / t));
  }
  function heirAmber() {
    var sh = heirShare();
    return sh != null && sh >= HEIR_WARN.amber;
  }
  function heirRed() {
    var sh = heirShare(),
      r = D.isLive() && typeof D.state.heirWarnShare === 'number' ? D.state.heirWarnShare : HEIR_WARN.red;
    return sh != null && sh >= r;
  }
  function heirBig(s) {
    if (s < 86400) return clock(s);
    var d = Math.floor(s / 86400);
    return fmt(d) + '<small>' + (d === 1 ? 'day' : 'days') + '</small>';
  } /* the largest sensible unit; under a day the clock */
  function applyRulesTo(id, r) {
    if (D.isLive() || !r) return;
    var a = acctById(id);
    if (!a) return;
    if (r.limit != null) {
      a.limit = r.limit;
      if (id === 'main' && a.left > r.limit) a.left = r.limit;
    }
    if (r.notice != null) a.noticeSeconds = Math.round(r.notice * 3600);
    refresh();
    applyScene();
  }

  /* ---- the background follows the numbers ---- */
  function baseScene() {
    var s = D.state,
      pct = s.limit ? ((s.left || 0) / s.limit) * 100 : 100;
    if (s.stopped && !s.unopened)
      return {
        left: 0,
        mode: 'stopped',
        k: 1,
      }; /* unopened: the lines stay; a collapsed point read as 'the tendrils are gone' */
    if (ui.acct !== 'main') pct = 100;
    return { left: pct, mode: 'home', k: 0 };
  }
  function applyScene() {
    var def = ui.cur && SCR[ui.cur.name],
      o = baseScene();
    var over = def && def.scene ? def.scene(ui.cur.params || {}) : null;
    if (ui.duress) over = { left: 100, mode: 'home', k: 0 }; /* no limit, no stop */
    if (over) for (var k in over) o[k] = over[k];
    if (over && over.mode && over.mode !== 'stopped' && over.k === undefined)
      o.k = 0; /* a stopped account collapses the ribbon to a point; a shape needs it whole */
    /* reading screens dim the scene; a walk step that shows a rule shape keeps it bright */
    o.dim = over && over.dim === false ? false : !(def && def.money);
    if (o.charge === undefined) o.charge = ui.drag; /* a held slide charges the background */
    if (o.due === undefined) o.due = null; /* only a Notice screen pins the fill; elsewhere the scene's own clock runs */
    if (o.mark === undefined) o.mark = null; /* only the lock screen draws the K */
    if (o.lift === undefined) o.lift = 0; /* only walk steps lift their shape clear of the text */
    if (ui.cur) o.screen = ui.cur.name;
    o.accent = ui.duress ? null : acctColor(ui.acct); /* the account's colour tints the scene */
    KVScene.setState(o);
    document.getElementById('scene').style.opacity = def && def.noScene ? '0' : ''; /* a screen may switch the art off (Price) */
  }

  /* ---- navigation: one screen fades up as the last fades out ---- */
  function show(name, params) {
    if (KVScene.sleep) KVScene.sleep(false); /* whatever happened in the scanner, a new screen wakes the scene */
    var def = SCR[name],
      el = document.createElement('section');
    el.className = 'ui enter' + (def.tab ? ' tabbed' : '') +
      (def.tab === 'settings' && name !== 'settings' && (def.dark || !def.money) ? ' dark' : '');
    /* Settings pages wear dark glass; money screens keep the bright ribbon unless they opt in */
    ui.cur = { name: name, params: params || {} };
    if (bridge && bridge.screen) bridge.screen(name); /* the phone decides FLAG_SECURE from the screen name */
    el.innerHTML = def.html(ui.cur.params);
    addHelp(el, name);
    var mb = el.querySelector('[data-menu]');
    if (mb && name !== 'home') mb.onclick = openMenu; /* Home wires its own */
    if (!def.bare) glassify(el); /* bottom-half glass sheet behind everything below the top bar */
    stage.appendChild(el);
    var old = ui.el;
    ui.el = el;
    if (def.after) def.after(el, ui.cur.params);
    if (def.live) def.live(el, ui.cur.params);
    [].forEach.call(el.querySelectorAll('.big,.clock'), fit);
    document.body.classList.toggle('notabs', !def.tab || !!ui.duress);
    document.body.classList.toggle('scanning', name === 'scan'); /* the camera fills the screen */
    document.body.classList.toggle('qrscreen', name === 'receive'); /* the ribbon dims further behind the square code */
    [].forEach.call(tabs.children, function (b) {
      b.classList.toggle('on', b.dataset.tab === def.tab);
    });
    ui.drag = 0;
    applyScene();
    requestAnimationFrame(function () {
      requestAnimationFrame(function () {
        el.classList.remove('enter');
      });
    });
    if (old) {
      old.classList.add('leave');
      setTimeout(function () {
        old.remove();
      }, 560);
    }
  }
  /* the duress view (panic PIN): these screens only; any other name lands on the purse */
  var DURESS_OK = ['lock', 'duress', 'pursepay'];
  function go(name, params, opts) {
    if (ui.duress && DURESS_OK.indexOf(name) < 0) {
      name = 'duress';
      params = {};
    }
    /* Sample only */
    if (SCR[name] && SCR[name].sample && D.isLive()) return go('home', {}, { root: true });
    if (ui.stopExplain) ui.stopExplain(); /* the explainer belongs to the screen it was opened on */
    opts = opts || {};
    if (opts.root) ui.stack = [];
    else if (opts.replace) ui.stack.pop();
    ui.stack.push({ name: name, params: params || {} });
    show(name, params);
    /* put away during a locked screen's job: the lock is the next screen */
    if (ui.lockDue && name !== 'lock') lockNow();
  }
  function back() {
    if (ui.closeSheet) {
      ui.closeSheet(false);
      return true;
    }
    if (ui.cur && SCR[ui.cur.name].locked) return true;
    if (ui.stack.length < 2) return false;
    ui.stack.pop();
    var t = ui.stack[ui.stack.length - 1];
    show(t.name, t.params);
    return true;
  }
  window.KV_back = back;
  /* the phone's one-time notices (e.g. "fingerprints changed") sit on Home until tapped */
  window.KV_notice = function (text) {
    text = String(text || '').slice(0, 200);
    if (!text) return;
    ui.notice = ui.notice ? ui.notice + ' ' + text : text;
    refresh();
  };
  /* home-screen shortcuts (long-press the app icon): the phone passes one of three screen names, nothing else */
  window.KV_open = function (name) {
    if (name !== 'send' && name !== 'receive' && name !== 'stop') return true;
    if (ui.cur && ui.cur.name === 'lock') ui.afterLock = name;
    else go(name, {}); /* behind the lock, it opens after the fingerprint */
    return true;
  };
  document.addEventListener('focusin', function (e) {
    /* the keyboard covered the fields; the focused one scrolls into view once the keyboard is up */
    var t = e.target;
    if (!t || !/^(INPUT|TEXTAREA)$/.test(t.tagName)) return;
    setTimeout(function () {
      if (document.activeElement === t) t.scrollIntoView({ block: 'center', behavior: 'smooth' });
    }, 380);
  });
  window.KV_insets = function (t, r, b, l, ime) {
    var st = document.documentElement.style;
    st.setProperty('--n-t', t + 'px');
    st.setProperty('--n-r', r + 'px');
    st.setProperty('--n-b', b + 'px');
    st.setProperty('--n-l', l + 'px');
    st.setProperty('--ime', ime + 'px');
    var f = document.activeElement;
    if (ime && f && f.scrollIntoView) f.scrollIntoView({ block: 'center', behavior: 'smooth' });
  };

  /* ---- the lock: an account that exists opens behind a fingerprint or PIN, at start and the moment the app is put away (no
     grace). The page learns it is away from visibilitychange; the phone says so too from onPause, since a paused
     WebView may not fire it. The phone's own prompt may pause the page: that never locks (ui.bio); if the app is put away while
     the prompt is up, the prompt answers 'away' and the page locks then. Coming back shows the lock that is already there. ---- */
  function hasAccount() {
    return D.getSetting('welcomed') === '1' || D.isLive();
  }
  function lockNow() {
    if (ui.bio || D.prompting() || !hasAccount() || !ui.cur) return;
    var held = ui.cur.name !== 'lock' && !!SCR[ui.cur.name].locked;
    ui.lockDue = held; /* a money job holds the screen: the lock comes when it moves on */
    if (held || ui.cur.name === 'lock') return;
    if (ui.closeSheet) ui.closeSheet(false);
    go('lock', {}, { root: true });
  }
  D.onAway = function () {
    scrubAway();
    lockNow();
  }; /* the phone took a money prompt down because the app was put away */
  /* put away: typed vault seeds and stop words leave the screen at once, whatever lock_after */
  function scrubAway() {
    var el = ui.el,
      name = ui.cur && ui.cur.name;
    if (!el || (name !== 'paperout' && name !== 'paperuse')) return;
    if (el.querySelector('[data-armed]')) resetSlide(el);
    if (name === 'paperuse') {
      [].forEach.call(el.querySelectorAll('[data-pw]'), function (i) {
        i.value = '';
      });
      return;
    }
    paperOutClear(el);
    if (!el.kvBusy && el.kvCode) paperOutStep(el, 'seeds');
  }
  function lockAfterSecs() {
    return parseInt(D.getSetting('lock_after'), 10) || 0;
  } /* fewer prompts; 0 = at once */
  function shown() {
    return !ui.away && !document.hidden;
  } /* the app is in front: the only time the phone's fingerprint prompt may open by itself */
  function wentAway() {
    var secs = lockAfterSecs();
    ui.away = true;
    clearTimeout(ui.awayT);
    scrubAway();
    if (!secs) return lockNow();
    ui.awayAt = Date.now();
    ui.awayT = setTimeout(lockNow, secs * 1000);
  }
  function cameBack() {
    /* back on the lock screen: the phone's prompt opens by itself */
    clearTimeout(ui.awayT);
    ui.away = false;
    if (ui.lockDue || (ui.awayAt && Date.now() - ui.awayAt >= lockAfterSecs() * 1000)) lockNow();
    ui.awayAt = 0;
    if (ui.cur && ui.cur.name === 'lock' && !ui.bio && ui.el) {
      var b = ui.el.querySelector('[data-ok]');
      if (b)
        setTimeout(function () {
          if (!ui.bio && shown() && ui.cur && ui.cur.name === 'lock') b.click();
        }, 250);
    }
  }
  document.addEventListener('visibilitychange', function () {
    if (document.hidden) wentAway();
    else cameBack();
  });
  window.KV_pause = function () {
    wentAway();
    return true;
  };
  window.KV_resume = function () {
    cameBack();
    return true;
  };

  /* clicks on anything marked data-go / data-back, in one place */
  stage.addEventListener('click', function (e) {
    var t = e.target.closest('[data-back],[data-go],[data-toggle],[data-acct],[data-help],[data-explain]');
    if (!t) return;
    if (t.hasAttribute('data-back')) back();
    else if (t.hasAttribute('data-help')) openHelp(ui.cur.name);
    else if (t.dataset.explain) {
      if (!ui.guards) openProtect();
      else if (t.dataset.explain === 'safe' && t.classList.contains('pip'))
        go('safe'); /* the Vault bar opens the vault */
      else openDial(t.dataset.explain);
    } /* a bar opens that one dial; no covenant yet: the offer to put one on */ else if (t.dataset.toggle) {
      var on = t.getAttribute('aria-checked') !== 'true';
      t.setAttribute('aria-checked', on);
      if (!t.closest('[data-staged]')) D.setSetting(t.dataset.toggle, on ? 'on' : 'off'); /* a staged screen saves from its bar */
    } else if (t.dataset.acct) {
      ui.acct = t.dataset.acct;
      refresh();
      applyScene();
    } else {
      var gp = {};
      try {
        gp = t.dataset.params ? JSON.parse(t.dataset.params) : {};
      } catch (err) {
        gp = {}; /* a broken params attribute opens the screen plain */
      }
      go(t.dataset.go, gp);
    }
  });
  tabs.addEventListener('click', function (e) {
    var b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.tab === 'home') return go('home', {}, { root: true });
    ui.stack = [{ name: 'home', params: {} }];
    go(b.dataset.tab, {});
  });
  function refresh() {
    var d = ui.cur && SCR[ui.cur.name];
    if (d && d.live) d.live(ui.el, ui.cur.params);
  }
  /* ---- the activity log: the last few things, and a Stop on anything still leaving ---- */
  /* the guard's own alerts (engine status.alerts: newest first, bank words, `at` in seconds):
     rows in the Alerts list (Home keeps its short log and alarm row); a waiting withdrawal
     stands behind the waiting row above, as in history */
  function alertRows(s) {
    return (s.alerts || [])
      .filter(function (a) {
        return !(s.pending && a.kind === 'withdrawal_pending');
      })
      .map(function (a) {
        return { text: a.text, sub: alertWhen(a.at), go: '', ref: '' };
      });
  }
  function alertWhen(at) {
    var d = new Date(at * 1000),
      t = d.getHours() + ':' + ('0' + d.getMinutes()).slice(-2);
    if (d.toDateString() === new Date().toDateString()) return 'Today, ' + t;
    return d.toLocaleDateString() + ', ' + t;
  }
  /* the bell's amber dot: an alert newer than the newest one seen in Alerts. Kept in the
     alerts_seen setting; a phone that does not keep that key yet keeps it for this run */
  function alertsTop() {
    var a = (D.state.alerts || [])[0];
    return a ? a.at : 0;
  }
  /* alerts_seen: "<newest own alert seen>|<watched alert marks seen>", or the older bare number */
  function seenStore() {
    var v = String(D.getSetting('alerts_seen') || ''),
      bar = v.indexOf('|'),
      t = Number(bar < 0 ? v : v.slice(0, bar)) || 0,
      marks = bar < 0 ? [] : v.slice(bar + 1).split(',');
    return { t: Math.max(t, ui.alertsSeen || 0), marks: marks.concat(ui.watchSeen || []) };
  }
  function alertsNew() {
    return alertsTop() > seenStore().t;
  }
  /* a short mark for one watched alert (FNV-1a, 8 hex) so the seen list stays small */
  function alertMark(k) {
    var h = 0x811c9dc5;
    for (var i = 0; i < k.length && i < 512; i++)
      h = Math.imul(h ^ k.charCodeAt(i), 0x01000193) >>> 0;
    return ('0000000' + h.toString(16)).slice(-8);
  }
  /* alerts about the accounts this phone watches for others: what each one's status says
     now (a withdrawal waiting, ready to claim, stopped) and the engine's alerts (st.alerts) */
  function watchedAlerts() {
    if (!D.isLocal()) return [];
    var out = [];
    watchedList().forEach(function (w) {
      var st = w.st || {},
        nm = watchedName(w);
      function add(key, text, sub, loud) {
        var mark = alertMark(w.id + '|' + key);
        out.push({ mark: mark, text: text, sub: sub, loud: !!loud, id: w.id });
      }
      if (st.stage === 'waiting' && st.pending)
        add('w' + st.pending.kas + st.pending.to,
          'A withdrawal of ' + fmt(st.pending.kas) + ' KAS is waiting on ' + nm,
          'Goes through in ' + dur(st.pending.ends_in_seconds), st.can_stop);
      if (st.can_claim) add('c', nm + ' can be claimed', 'The inactivity period has run out');
      if (st.stage === 'stopped') add('s', nm + ' was stopped', 'Emergency stop');
      (st.alerts || []).slice(0, 10).forEach(function (a) {
        var hot = a.kind === 'withdrawal_pending' && st.can_stop;
        add('a' + a.at + a.text, nm + ': ' + a.text, alertWhen(a.at), hot);
      });
    });
    return out;
  }
  function watchedUnseen() {
    var marks = seenStore().marks;
    return watchedAlerts().filter(function (a) {
      return marks.indexOf(a.mark) < 0;
    });
  }
  /* something still waits on a watched account (a withdrawal, or a claim): the bell stays lit */
  function watchedWaiting() {
    return watchedList().some(function (w) {
      var st = w.st || {};
      return (st.stage === 'waiting' && st.pending) || st.can_claim;
    });
  }
  /* the bell: lit while anything waits (here or on a watched account), acknowledged or not;
     red for a withdrawal not yet owned up to here, or one you can stop on an account you guard,
     not yet seen; amber for news since Alerts */
  function bellLight(bell, waits, banner, main) {
    var wNew = main ? watchedUnseen() : [],
      wLoud = wNew.some(function (a) {
        return a.loud;
      });
    bell.classList.toggle('has', waits || (main && watchedWaiting()));
    bell.classList.toggle('loud', banner || wLoud);
    bell.classList.toggle('new', !banner && !wLoud && (alertsNew() || wNew.length > 0));
  }
  function alertsMark() {
    var t = Math.max(alertsTop(), seenStore().t),
      marks = watchedAlerts().map(function (a) {
        return a.mark;
      }).slice(0, 24);
    ui.alertsSeen = t;
    ui.watchSeen = marks;
    D.setSetting('alerts_seen', String(t) + (marks.length ? '|' + marks.join(',') : ''));
  }
  function logRows(n, all) {
    var s = D.state,
      rows = [];
    if (s.outgoing)
      rows.push({
        text: 'Sending ' + fmt(s.outgoing.amount) + ' KAS to ' + s.outgoing.to,
        sub: 'Leaves in ' + s.outgoing.secondsLeft + ' s',
        stop: 'outgoing',
        go: '',
      });
    var pq = s.pending; /* a withdrawal only the guard saw has no amount, address or time */
    if (pq)
      rows.push({
        text:
          pq.amount == null
            ? 'A withdrawal your alerts caught'
            : 'Withdrawal of ' + fmt(pq.amount) + ' KAS to ' +
              (pq.to || 'an address your alerts saw'),
        sub:
          pq.secondsLeft > 0 ? 'Leaves in ' + clock(pq.secondsLeft)
            : pq.secondsLeft == null ? 'Caught by your alerts' : 'Ready to leave',
        stop: 'pending',
        go: 'pending',
      });
    if (all)
      watchedAlerts().forEach(function (a) {
        rows.push({ text: a.text, sub: a.sub, go: 'watched', ref: '', watch: a.id });
      });
    if (all)
      alertRows(s).forEach(function (r) {
        if (rows.length < n) rows.push(r);
      });
    s.history.forEach(function (h) {
      if (rows.length >= n) return;
      if (h.kind === 'outgoing' || (h.kind === 'pending' && s.pending)) return; /* the waiting row above stands for it, quieted or not */
      rows.push({
        text: h.text,
        sub: h.when || (h.tx ? 'Reference ' + String(h.tx).slice(0, 10) + '\u2026' : ''),
        go: h.go || '',
        ref: h.tx && h.tx !== 'sample' ? String(h.tx) : '',
      });
    });
    return rows.slice(
      0,
      Math.max(
        n,
        rows.filter(function (r) {
          return r.stop;
        }).length
      )
    );
  }
  function logHtml(n, all, keep) {
    var rows = (ui.logRows = logRows(n, all));
    return rows
      .map(function (r, i) {
        if (keep && !keep(r)) return '';
        return (
          '<div class="item"><button class="rowmain" data-logrow="' +
          i +
          '"><span><span class="t">' +
          esc(r.text) +
          '</span><span class="d">' +
          esc(r.sub) +
          '</span></span></button>' +
          (r.stop ? '<button class="stopbtn" data-stopwhat="' + r.stop + '">Stop</button>' : '') +
          '</div>'
        );
      })
      .join('');
  }
  /* a tap on a row: the detail in a sheet (amount, when, reference, and Stop while it can still be stopped) */
  function openRow(r) {
    if (r.stop === 'pending') return go('pending', {});
    if (r.watch) return go('watched', { id: r.watch });
    var m = /([\d.,]+) KAS/.exec(r.text),
      kas = m ? parseFloat(m[1].replace(/,/g, '')) : null;
    openSheet(
      '<div class="label">' +
        esc(r.sub || 'Activity') +
        '</div><div class="line mt8">' +
        esc(r.text) +
        '</div>' +
        (kas != null ? '<div class="big mt8">' + bigKas(kas) + '</div><div class="sub">' + esc(under(kas)) + '</div>' : '') +
        (r.ref ? '<div class="sub">Reference ' + esc(r.ref) + '</div>' : '') +
        (r.stop === 'outgoing' ? '<button class="pill danger" data-stopit>Stop it</button>' : '') +
        '<button class="ghost" data-no>Close</button>'
    );
    var sb = sheet.querySelector('[data-stopit]');
    if (sb)
      sb.onclick = function () {
        ui.closeSheet();
        var x = D.stopOutgoing();
        haptic();
        if (x && x.ok === false) refuse(x.error);
      };
  }
  stage.addEventListener('click', function (e) {
    var b = e.target.closest('[data-stopwhat]');
    if (!b) return;
    if (b.dataset.stopwhat === 'outgoing') {
      var r = D.stopOutgoing();
      haptic();
      if (r && r.ok === false) refuse(r.error);
    } else go('pending', {});
  });
  stage.addEventListener('click', function (e) {
    var b = e.target.closest('[data-logrow]');
    if (!b) return;
    var r = ui.logRows && ui.logRows[+b.dataset.logrow];
    if (r) openRow(r);
  });
  /* Home and People fold their long blocks to one tappable line, so the scene behind stays in
     view; anything waiting (a withdrawal, a claim, a Stop) stays shown. Open or
     closed is kept in memory only: the phone's settings are allow-listed (Bridge.java) */
  function foldOpen(k) {
    return !!(ui.folds && ui.folds[k]);
  }
  function foldHead(k, html, cls) {
    var o = foldOpen(k);
    return '<button class="foldhd' + (cls ? ' ' + cls : '') + '" data-fold="' + k + '" aria-expanded="' + o +
      '"><span class="t">' + html + '</span><svg class="chev' + (o ? ' on' : '') +
      '" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 4l4 4 4-4"/></svg></button>';
  }
  stage.addEventListener('click', function (e) {
    var b = e.target.closest('[data-fold]');
    if (!b) return;
    ui.folds = ui.folds || {};
    ui.folds[b.dataset.fold] = !ui.folds[b.dataset.fold];
    refresh();
  });
  /* Home's activity: one line with the count; a row that can still be stopped always shows */
  function homeLogHtml() {
    var body = logHtml(3, false, foldOpen('log') ? null : function (r) {
        return r.stop;
      }),
      n = ui.logRows.length;
    return n ? foldHead('log', 'Account activity (' + n + ')') + body : '';
  }
  /* Home's helped accounts: one line; a row with a withdrawal or claim waiting always shows */
  function homeHelpedHtml() {
    var n = watchedList().length;
    return n ? foldHead('helped', 'People you help (' + n + ')') + watchedRowsHtml(!foldOpen('helped')) : '';
  }

  /* ---- "protected by": five pips that show what guards this money ---- */
  /* Home Heir pip: the heir's name, capital first; 'Set' while this phone does not know it */
  function heirPip() {
    var h = helperRows().filter(function (r) {
        return r.role === 'heir';
      })[0],
      n = h && h.name && h.name !== 'Someone' ? String(h.name).trim() : '';
    return n ? n.charAt(0).toUpperCase() + n.slice(1) : 'Set';
  }
  function guardsHtml(g) {
    var pips = g
      ? [
          [true, 'Limit', fmt(g.limit) + ' a day'],
          [true, 'Notice', dur(g.notice)],
          [true, 'Stops', g.stops + (g.stops === 1 ? ' button' : ' buttons')],
          [!!g.heir, 'Heir', g.heir ? heirPip() : 'none', g.heir && !g.stopped ? heirLeft() : null],
          [true, 'Vault', g.stopped ? 'Holding' : 'Ready'],
        ]
      : [
          [false, 'Limit', 'none'],
          [false, 'Notice', 'none'],
          [false, 'Stops', 'none'],
          [false, 'Heir', 'none'],
          [false, 'Vault', 'none'],
        ];
    ui.guards = g;
    return (
      '<div class="pips' +
      (g && g.stopped ? ' stopped' : '') +
      '" aria-label="The covenant on this money">' +
      pips
        .map(function (p) {
          return (
            '<button class="pip' +
            (p[0] ? ' on' : '') +
            '" data-explain="' +
            (p[1] === 'Vault' ? 'safe' : p[1].toLowerCase()) +
            '" aria-label="Change ' +
            p[1] +
            '"><i></i><span class="k">' +
            p[1] +
            '</span><span class="v">' +
            esc(p[2]) +
            '</span>' +
            (p[3] != null ? '<span class="hl ' + heirTone() + '">' + esc(dur(p[3])) +
              ' left</span>' : '') +
            '</button>'
          );
        })
        .join('') +
      '</div>' +
      (g ? '<button class="ghost small" data-go="protection">See all your rules</button>' : '')
    );
  }

  /* trusted payees: with the rule on, a payment to anyone else waits the notice period whatever the amount */
  function onlyTrusted() {
    return !D.isLive() && D.getSetting('only_trusted') === 'on';
  }
  function trustedTo(d) {
    return ui.people.some(function (q) {
      return q.role === 'Trusted payee' && (q.name === d.label || q.addr === d.to);
    });
  }

  /* ---- KAS price: the chart (line coloured by direction, grid, high/low marks, MA tools) over the scene's calm glowing line ---- */
  var chartRange = 'week';
  function chartMoney(v) {
    return '$' + v.toFixed(v < 1 ? 3 : 2);
  } /* 3 decimals under a dollar, 2 above */
  function chartWhen(t, range) {
    var d = new Date(t),
      two = function (v) {
        return ('0' + v).slice(-2);
      };
    var DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'],
      MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
    if (range === 'day') return two(d.getHours()) + ':' + two(d.getMinutes());
    if (range === 'week') return DAYS[d.getDay()] + ' ' + two(d.getHours()) + ':' + two(d.getMinutes());
    if (range === 'year' || range === 'all') return MONTHS[d.getMonth()] + ' ' + d.getFullYear();
    return d.getDate() + ' ' + MONTHS[d.getMonth()];
  }
  function chartTick(t, range) {
    /* the short form along the bottom */
    var d = new Date(t),
      MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
    if (range === 'week') return ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][d.getDay()];
    if (range === 'year') return MONTHS[d.getMonth()];
    if (range === 'all') return String(d.getFullYear());
    return chartWhen(t, range);
  }
  function movingAvg(vals, n) {
    /* simple average of the last n points; the first points average what exists so far */
    var out = [],
      sum = 0;
    for (var i = 0; i < vals.length; i++) {
      sum += vals[i];
      if (i >= n) sum -= vals[i - n];
      out.push(sum / Math.min(i + 1, n));
    }
    return out;
  }
  function chartGrid(C, lo, hi, x0, x1, Y) {
    var s = '';
    for (var g = 0; g < C.grid; g++) {
      /* price lines, labels at the right edge */
      var gv = hi - ((hi - lo) * g) / (C.grid - 1),
        gy = Y(gv);
      s +=
        '<line class="grid" x1="' +
        x0 +
        '" x2="' +
        x1 +
        '" y1="' +
        gy +
        '" y2="' +
        gy +
        '"/><text class="lbl" x="' +
        (x1 + 6) +
        '" y="' +
        gy +
        '" dy="3.5">' +
        chartMoney(gv) +
        '</text>';
    }
    return s;
  }
  function chartTimes(C, points, range, n, H, X) {
    var s = '';
    for (var t = 0; t < C.times; t++) {
      /* time labels along the bottom; the ends hug the edges */
      var ti = Math.round((t / (C.times - 1)) * (n - 1)),
        anchor = t === 0 ? 'start' : t === C.times - 1 ? 'end' : 'middle';
      s +=
        '<text class="lbl" x="' +
        X(ti) +
        '" y="' +
        (H - 5) +
        '" text-anchor="' +
        anchor +
        '">' +
        esc(chartTick(points[ti][0], range)) +
        '</text>';
    }
    return s;
  }
  function chartHiLo(vals, X, Y, x0, x1) {
    /* the period's high and low, marked where they happen on the line */
    var hiI = 0,
      loI = 0;
    vals.forEach(function (v, i) {
      if (v > vals[hiI]) hiI = i;
      if (v < vals[loI]) loI = i;
    });
    function mark(i, word, dy) {
      var x = +X(i),
        anchor = x < x0 + 40 ? 'start' : x > x1 - 40 ? 'end' : 'middle';
      return (
        '<circle class="hl" cx="' + X(i) + '" cy="' + Y(vals[i]) + '" r="2"/><text class="hll" x="' + X(i) + '" y="' + Y(vals[i]) +
        '" dy="' + dy + '" text-anchor="' + anchor + '">' + word + ' ' + chartMoney(vals[i]) + '</text>'
      );
    }
    return mark(hiI, 'High', -7) + mark(loI, 'Low', 14);
  }
  function chartDelta(d) {
    var a = Math.abs(d);
    return (d < 0 ? '\u2212' : '+') + '$' + a.toFixed(a < 0.01 ? 4 : a < 1 ? 3 : 2);
  } /* the period's change in dollars, one more decimal than prices under a cent */
  function chartSvg(points, range, tools, W) {
    /* -> SVG markup; W = drawing width in CSS pixels */
    var C = CHART,
      H = C.h,
      n = points.length,
      vals = points.map(function (p) {
        return p[1];
      });
    var lo = Math.min.apply(null, vals),
      hi = Math.max.apply(null, vals),
      span = hi - lo || 1;
    var x0 = C.left,
      x1 = W - C.right,
      pw = x1 - x0,
      y0 = C.top,
      ph = H - C.top - C.bottom;
    function X(i) {
      return (x0 + (n > 1 ? (i / (n - 1)) * pw : pw / 2)).toFixed(1);
    }
    function Y(v) {
      return (y0 + (1 - (v - lo) / span) * ph).toFixed(1);
    }
    var up = '',
      dn = '',
      area = 'M' + X(0) + ' ' + (y0 + ph).toFixed(1);
    for (var i = 0; i < n; i++) {
      area += ' L' + X(i) + ' ' + Y(vals[i]);
      if (i) {
        var seg = 'M' + X(i - 1) + ' ' + Y(vals[i - 1]) + 'L' + X(i) + ' ' + Y(vals[i]);
        if (vals[i] >= vals[i - 1]) up += seg;
        else dn += seg;
      } /* each step goes to the green or the red path */
    }
    area += ' L' + X(n - 1) + ' ' + (y0 + ph).toFixed(1) + 'Z';
    var rising = vals[n - 1] >= vals[0],
      s =
        '<svg viewBox="0 0 ' +
        W +
        ' ' +
        H +
        '" data-n="' +
        n +
        '">' +
        '<defs><linearGradient id="kvAreaUp" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#7fcf9a" stop-opacity=".22"/><stop offset="1" stop-color="#7fcf9a" stop-opacity="0"/></linearGradient>' +
        '<linearGradient id="kvAreaDown" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#e07070" stop-opacity=".22"/><stop offset="1" stop-color="#e07070" stop-opacity="0"/></linearGradient></defs>';
    s += chartGrid(C, lo, hi, x0, x1, Y);
    s += chartTimes(C, points, range, n, H, X);
    s += '<path class="' + (rising ? 'area-up' : 'area-down') + '" d="' + area + '"/>';
    if (up) s += '<path class="ln-up" d="' + up + '"/>';
    if (dn) s += '<path class="ln-down" d="' + dn + '"/>';
    C.ma.forEach(function (m) {
      if (!tools[m.key]) return;
      var av = movingAvg(vals, m.n),
        d = '';
      for (var i = 0; i < n; i++) d += (i ? 'L' : 'M') + X(i) + ' ' + Y(av[i]);
      s += '<path class="ma ' + m.cls + '" d="' + d + '"/>';
    });
    s += chartHiLo(vals, X, Y, x0, x1);
    s +=
      '<circle class="dot" cx="' +
      X(n - 1) +
      '" cy="' +
      Y(vals[n - 1]) +
      '" r="' +
      C.dot +
      '"/></svg>';
    return s;
  }
  function chartBox() {
    return ui.el && ui.cur && ui.cur.name === 'price' ? ui.el.querySelector('[data-chart]') : null;
  } /* the chart's box while the price screen is up */
  function chartAskPrices() {
    ui.priceAsked = ui.priceAsked || {};
    if (D.fetchPrices && !(ui.priceAsked[chartRange] > Date.now() - 600000)) {
      /* real prices arrive and redraw; a miss is asked again next time */
      ui.priceAsked[chartRange] = Date.now();
      D.fetchPrices(chartRange).then(function (v) {
        if (!v) {
          delete ui.priceAsked[chartRange];
          var b3 = !D.priceAt(chartRange) && chartBox();
          if (b3) {
            b3.innerHTML = '<div class="sub">CoinGecko is busy right now. Trying again in a moment.</div>';
            ui.priceRetries = (ui.priceRetries || 0) + 1;
            if (ui.priceRetries <= CHART.retries) setTimeout(function () {
              chartInto(chartBox());
            }, CHART.retryMs); /* the feed rests 2 min after a refusal */
          }
          return;
        }
        ui.priceRetries = 0;
        var b2 = ui.el && ui.cur && ui.cur.name === 'price' ? ui.el.querySelector('[data-chart]') : null;
        if (b2) chartInto(b2);
      });
    }
  }
  function chartControls(tools) {
    return (
      '<div class="ranges">' +
      CHART.ranges
        .map(function (r) {
          return '<button data-range="' + r[0] + '"' + (r[0] === chartRange ? ' class="on"' : '') + '>' + r[1] + '</button>';
        })
        .join('') +
      '</div>' /* the periods and tools as visible controls under the chart */ +
      '<div class="chips tools"><span class="lbl">Tools</span>' +
      CHART.ma
        .map(function (m) {
          return '<button data-tool="' + m.key + '"' + (tools[m.key] ? ' class="on"' : '') + '>' + m.label + '</button>';
        })
        .join('') +
      '</div>'
    );
  }
  function wireChartButtons(box) {
    [].forEach.call(box.querySelectorAll('[data-range]'), function (b) {
      b.onclick = function () {
        chartRange = b.dataset.range;
        chartInto(box);
      };
    });
    [].forEach.call(box.querySelectorAll('[data-tool]'), function (b) {
      b.onclick = function () {
        D.setSetting(b.dataset.tool, D.getSetting(b.dataset.tool) === 'on' ? 'off' : 'on');
        haptic();
        chartInto(box);
      };
    });
  }
  function chartSource(range) {
    var at = D.priceAt ? D.priceAt(range) : 0;
    if (!at) return 'Sample prices until the real ones arrive';
    return 'Prices from CoinGecko' + (Date.now() - at > 600000 ? ', as of ' + chartWhen(at, 'week') : '');
  } /* older than 10 minutes: the saved prices say when they are from */
  var RANGE_WORDS = {
    day: 'past day',
    week: 'past week',
    month: 'past month',
    quarter: 'past 3 months',
    year: 'past year',
    all: 'since the start',
  };
  function usd(v) {
    return '$' + v.toFixed(v < 1 ? 4 : 2);
  } /* the summary's prices: four decimals under a dollar */
  function kasCount(v) {
    var big = v >= 1000;
    return v.toLocaleString('en-US', { maximumFractionDigits: big ? 0 : 4 });
  } /* grouped thousands, never scientific notation */
  function sumRow(k, v, sub) {
    return (
      '<div class="row prow"><span class="d">' +
      esc(k) +
      '</span><span class="v">' +
      esc(v) +
      (sub ? '<small>' + esc(sub) + '</small>' : '') +
      '</span></div>'
    );
  }
  function avgLabel(s) {
    if (s.avgHours >= 156) return 'Average, last 7 days';
    var h = s.avgHours;
    return 'Average, last ' + (h >= 48 ? Math.round(h / 24) + ' days' : h + ' hours');
  }
  function priceSumInto() {
    /* the big price, the period's change, average/high/low rows; follows the range buttons */
    var box = ui.el && ui.el.querySelector('[data-psum]');
    if (!box) return;
    var s = D.priceSummary ? D.priceSummary(chartRange) : null;
    if (!s) {
      box.innerHTML = '';
      return;
    }
    var up = s.change >= 0,
      pct = (up ? '+' : '\u2212') + Math.abs(s.pct).toFixed(1) + '%';
    box.innerHTML =
      '<div class="pbig">' +
      esc(usd(s.now)) +
      '</div><div class="pch ' +
      (up ? 'up' : 'down') +
      '">' +
      esc(chartDelta(s.change) + ' (' + pct + ')') +
      '<span class="d">' +
      esc(RANGE_WORDS[chartRange] || '') +
      '</span></div>' +
      sumRow(avgLabel(s), usd(s.avg)) +
      sumRow('High', usd(s.high)) +
      sumRow('Low', usd(s.low));
  }
  function supplyRows(s) {
    var pct = ((s.mined / s.max) * 100).toFixed(1) + '% of the maximum';
    return (
      sumRow('Mined so far', kasCount(s.mined) + ' KAS', pct) +
      sumRow('Maximum supply', kasCount(s.max) + ' KAS') +
      (s.reward !== null ? sumRow('New coins per block', kasCount(s.reward) + ' KAS') : '') +
      (s.cutAt && s.next !== null ?
        sumRow('Next reduction', s.cutAt, 'to ' + kasCount(s.next) + ' KAS') : '')
    );
  }
  function supplyInto(box) {
    /* market.supply: hidden quietly on a refusal or an engine without it */
    if (!box || !D.marketSupply) return;
    D.marketSupply()
      .then(function (s) {
        if (!s || !(ui.cur && ui.cur.name === 'price')) return;
        box.innerHTML = '<div class="label mt20">KAS supply</div>' + supplyRows(s);
      })
      .catch(function () {});
  }
  function chartHead(tools) {
    var legend = CHART.ma
      .filter(function (m) {
        return tools[m.key];
      })
      .map(function (m) {
        return '<span class="lg ' + m.cls + '">' + m.label + '</span>';
      })
      .join('');
    return legend ? '<div class="ch">' + legend + '</div>' : '';
  } /* the price and change sit in the summary above; the chart keeps its tool legend */
  function chartInto(box) {
    if (!box) return;
    var series = D.priceSeries(chartRange);
    chartAskPrices();
    if (!series) {
      box.innerHTML = '<div class="sub">Fetching the KAS price\u2026</div>';
      priceSumInto();
      ui.priceSeries = null;
      applyScene();
      return;
    }
    var lo = Math.min.apply(null, series),
      hi = Math.max.apply(null, series),
      span = hi - lo || 1,
      n = series.length;
    ui.priceSeries = series.map(function (v) {
      return (v - lo) / span;
    }); /* the scene draws it: one glowing line */
    var points =
      D.pricePoints(chartRange) ||
      series.map(function (v, i) {
        return [Date.now() - (n - 1 - i) * 3600000, v];
      }); /* the chart draws every point; the scene keeps its calm 72 */
    var tools = {};
    CHART.ma.forEach(function (m) {
      tools[m.key] = D.getSetting(m.key) === 'on';
    });
    var W = Math.max(240, Math.round(box.clientWidth || 360)); /* one SVG unit per pixel keeps the hairlines crisp */
    box.innerHTML =
      chartHead(tools) +
      chartSvg(points, chartRange, tools, W) +
      chartControls(tools) +
      '<div class="sub">' +
      chartSource(chartRange) +
      '</div>';
    priceSumInto();
    wireChartButtons(box);
    applyScene();
  }

  function toggleRow(key, title, sub, defaultOff) {
    var v = D.getSetting(key);
    var on = defaultOff ? v === 'on' : v !== 'off';
    return (
      '<div class="item"><span><span class="t">' +
      esc(title) +
      '</span>' +
      (sub ? '<span class="d">' + esc(sub) + '</span>' : '') +
      '</span><button class="switch" role="switch" aria-label="' +
      esc(title) +
      '" aria-checked="' +
      on +
      '" data-toggle="' +
      key +
      '"></button></div>'
    );
  }
  function row(go, title, sub, right) {
    return (
      '<button class="item" data-go="' +
      go +
      '"><span><span class="t">' +
      esc(title) +
      '</span>' +
      (sub ? '<span class="d">' + esc(sub) + '</span>' : '') +
      '</span><span class="r">' +
      esc(right || '') +
      '</span></button>'
    );
  }

  /* ---- the slide-to-stop gesture ---- */
  function slider(el, onChange, onDone) {
    var thumb = el.querySelector('i'),
      start = 0,
      p = 0,
      dragging = false;
    function max() {
      return el.clientWidth - thumb.offsetWidth - 10;
    }
    function leaving() {
      return !el.isConnected || !!el.closest('.leave');
    } /* a screen on its way out takes no more input */
    function setP(v) {
      p = Math.max(0, Math.min(1, v));
      el.style.setProperty('--p', p);
      el.style.setProperty('--x', p * max() + 'px');
      onChange(p);
    }
    function release() {
      if (p >= 0.92) {
        setP(1);
        haptic();
        onDone(setBack);
      } else setBack();
    }
    function setBack() {
      el.classList.add('spring');
      setP(0);
      setTimeout(function () {
        el.classList.remove('spring');
      }, 520);
    }
    thumb.addEventListener('pointerdown', function (e) {
      if (leaving()) return;
      dragging = true;
      start = e.clientX - p * max();
      el.classList.remove('spring');
      thumb.setPointerCapture(e.pointerId);
      e.preventDefault();
    });
    thumb.addEventListener('pointermove', function (e) {
      if (dragging) setP((e.clientX - start) / max());
    });
    thumb.addEventListener('pointerup', function () {
      if (dragging) {
        dragging = false;
        release();
      }
    });
    thumb.addEventListener('pointercancel', function () {
      dragging = false;
      setBack();
    });
    thumb.addEventListener('keydown', function (e) {
      if (leaving()) return;
      if (e.key === 'ArrowRight') {
        setP(p + 0.1);
        e.preventDefault();
      } else if (e.key === 'ArrowLeft') {
        setP(p - 0.1);
        e.preventDefault();
      } else if (e.key === 'Enter') release();
    });
  }

  /* ---- slide, then fingerprint: every money action. Live: the button becomes the slide at once, and the action's own
     signer prompt (D's moneyAction) is the one fingerprint; a plain prompt here opened no key and only doubled it
     (finger, slide, finger again). It stays after the slide: the safe key lives 15 s from that prompt.
     Sample: the plain confirm first. The slide charges the background; when it completes, one pulse runs. ---- */
  /* a slide that pays an address: the whole address shows in groups of 4 (Sample: in the confirm
     step too) */
  function armToSlide(to, btn, title, text, danger, label, kind, action) {
    armSlide(btn, title, text, danger, label, kind, action, to);
  }
  function slideNote(s, to) {
    var n = document.createElement('div');
    n.className = 'sub mt8';
    n.textContent = 'To ' + addrGroups(to);
    s.parentNode.insertBefore(n, s);
    s.kvNote = n;
  }
  function armSlide(btn, title, text, danger, label, kind, action, to) {
    (D.isLive() ? Promise.resolve(true) : confirmAction(title, text, danger)).then(function (yes) {
      if (!yes || !btn.parentNode) return;
      var s = document.createElement('div');
      s.className = 'slide' + (danger ? '' : ' go');
      s.setAttribute('data-armed', '');
      s.setAttribute('role', 'slider');
      s.setAttribute('aria-label', label);
      s.setAttribute('aria-valuemin', '0');
      s.setAttribute('aria-valuemax', '100');
      s.setAttribute('aria-valuenow', '0');
      s.innerHTML = '<i tabindex="0"></i><span>' + esc(label) + '</span>';
      s.kvBtn = btn;
      btn.parentNode.replaceChild(s, btn);
      /* Live has no confirm step: the address stands above the slide */
      if (to && D.isLive()) slideNote(s, to);
      ui.drag = 0;
      applyScene();
      slider(
        s,
        function (p) {
          ui.drag = p;
          s.setAttribute('aria-valuenow', Math.round(p * 100));
          applyScene();
        },
        function () {
          var run = action;
          action = null;
          if (!run) return; /* one shot; a held Enter repeated it within the leaving window */
          s.removeAttribute('data-armed');
          var th = s.querySelector('i');
          if (th) {
            th.blur();
            th.removeAttribute('tabindex');
          }
          ui.drag = 0;
          if (KVScene.pulse) KVScene.pulse(kind);
          run();
        }
      );
    });
  }
  /* a refusal after the slide: the slide springs back to its button and can be slid again (it must never be left dead) */
  function glow(b) {
    if (!b) return;
    b.classList.remove('glow');
    void b.offsetWidth;
    b.classList.add('glow');
  } /* a white pulse: the button did its job */
  function resetSlide(root) {
    var s = root && root.querySelector ? root.querySelector('.slide') : null;
    if (!s || !s.kvBtn || !s.parentNode) return;
    if (s.kvNote) s.kvNote.remove();
    s.parentNode.replaceChild(s.kvBtn, s);
    ui.drag = 0;
    applyScene();
  }
  /* the slide goes back to its button when the user edits what was confirmed */
  /* the slide pays what was armed: if To or the amount changed since (a payee chip, a scan),
     it refuses and springs back */
  function draftMoved(el, to, n) {
    var d = ui.draft;
    var same = String(d.to).toLowerCase() === String(to).toLowerCase();
    if (same && amountOf(d.amt) === n) return false;
    resetSlide(el);
    setText(el, '[data-e]', 'The payment changed. Check it and slide again.');
    return true;
  }
  function disarm(root) {
    var s = root.querySelector('.slide[data-armed]');
    if (!s || !s.kvBtn) return;
    if (s.kvNote) s.kvNote.remove();
    s.parentNode.replaceChild(s.kvBtn, s);
    ui.drag = 0;
    applyScene();
  }

  /* ---- staged settings: the controls change a draft; a bar with Save changes / Cancel rises once something differs from
     what is saved. Save applies through save(values) (true when done); Cancel redraws the screen from the saved values. ---- */
  function toggleState(root, key) {
    var t = root.querySelector('[data-toggle="' + key + '"]');
    return t && t.getAttribute('aria-checked') === 'true' ? 'on' : 'off';
  }
  function stageChanges(el, read, save) {
    var saved = read(),
      bar = document.createElement('div');
    el.setAttribute('data-staged', ''); /* the toggle handler leaves saving to the bar */
    bar.className = 'savebar';
    bar.innerHTML = '<button class="pill" data-save>Save changes</button><button class="ghost" data-cancel>Cancel</button>';
    el.appendChild(bar);
    var pad = document.createElement('div');
    pad.className = 'savepad';
    (el.querySelector('.glass') || el).appendChild(pad); /* room under the last row for the bar */
    function same(a, b) {
      for (var k in a) if (String(a[k]) !== String(b[k])) return false;
      return true;
    }
    function check() {
      bar.classList.toggle('on', !same(read(), saved));
    }
    bar.querySelector('[data-save]').onclick = function () {
      Promise.resolve(save(read())).then(function (ok) {
        if (ok) saved = read();
        check();
      });
    };
    bar.querySelector('[data-cancel]').onclick = function () {
      show(ui.cur.name, ui.cur.params);
    };
    el.addEventListener('click', function () {
      setTimeout(check, 0);
    });
    el.addEventListener('input', check);
  }

  /* The guard's servers: the phone takes a change only right after a fresh signer prompt, so the
     prompt comes only when the value changed, and the value is read back. */
  function saveNodes(el) {
    var v = el.querySelector('[data-nodes]').value.trim();
    if (v === D.getSetting('guard_nodes')) return Promise.resolve(true);
    return D.confirmFresh('Change the alert servers').then(function (yes) {
      if (yes) D.setSetting('guard_nodes', v);
      if (D.getSetting('guard_nodes') === v) return true;
      refuse('The alert servers could not be changed.');
      return false;
    });
  }
  /* The guard switch: on is free; off is the phone's own prompt (guardOff), answered through
     __kvGuardOff with 'ok' (stopped), 'cancel' or 'refused'. The switch shows what is running. */
  function guardSwitch(on, paint) {
    var later = function () {
      paint(!!bridge.guardRunning());
      setTimeout(function () {
        paint(!!bridge.guardRunning());
      }, 900);
    };
    if (on) {
      bridge.guardOn(true);
      haptic();
      return later();
    }
    if (!bridge.guardOff) {
      paint(!!bridge.guardRunning());
      return refuse('Update the app to turn alerts off.');
    }
    window.__kvGuardOff = function (st) {
      window.__kvGuardOff = null;
      if (st === 'ok') haptic();
      later();
    };
    bridge.guardOff('__kvGuardOff');
  }
  /* Settings save. A wider signer window is stored by the phone only right after a fresh signer
     prompt, so that change asks first and is read back; the same or a narrower one is free. */
  function signerWin(x) {
    return x === '' || x == null ? 300 : parseInt(x, 10) || 0;
  }
  function saveSettings(v) {
    var was = signerWin(D.getSetting('signer_window')),
      want = signerWin(v.signer_window),
      shots = v.screenshots === 'on' && D.getSetting('screenshots') !== 'on';
    var ask = Promise.resolve(true);
    if (want > was) ask = D.confirmFresh('Ask for your fingerprint less often');
    return ask.then(function (yes) {
      for (var k in v) {
        if (k === 'screenshots' && shots) continue;
        if (k !== 'signer_window' || (yes && want !== was)) D.setSetting(k, v[k]);
      }
      if (signerWin(D.getSetting('signer_window')) !== want) {
        refuse('The fingerprint setting could not be changed.');
        return false;
      }
      return shots ? allowShots() : true;
    });
  }
  function allowShots() {
    return D.confirmFresh('Allow screenshots').then(function (yes) {
      if (yes) D.setSetting('screenshots', 'on');
      if (D.getSetting('screenshots') === 'on') return true;
      refuse('Screenshots could not be turned on.');
      return false;
    });
  }
  /* After a stop the account opens again under a new watch code: the guardian and heir need it */
  function reopensAfterStop() {
    try {
      return !!(D.state.stopped || JSON.parse(D.getSetting('rules_move') || '{}').out);
    } catch (e) {
      return !!D.state.stopped;
    }
  }
  function shareNewCode() {
    if (ui.cur && ui.cur.name === 'lock') {
      ui.afterLock = 'watchcode'; /* put away meanwhile: it opens after the fingerprint */
      return;
    }
    openSheet(
      '<div class="label">Your account is open again</div>' +
        '<div class="line lite mt8">Share your new watch code with your guardian and heir.</div>' +
        '<button class="pill mt8" data-share>Share my watch code</button><button class="ghost" data-no>Later</button>'
    );
    sheet.querySelector('[data-share]').onclick = function () {
      ui.closeSheet();
      go('watchcode', {});
    };
  }

  /* ---- screens ---- */
  /* Home: losing this phone loses the purse and stop button 1, so an account with funds and no
     guardian
     or heir would have no way to move after a loss. One quiet row says so (keys.public read once a
     start). */
  function homeLoneRow(el, s, main) {
    var row = el.querySelector('[data-lone]');
    if (!row) return;
    if (main && D.isLocal() && !ui.pubAsked) {
      ui.pubAsked = true; /* People reads it again after a change */
      D.keysPublic().then(function (r) {
        if (r && r.ok) {
          ui.pub = r.public || {};
          ui.stops = r.stops || null;
        }
        if (ui.cur && ui.cur.name === 'home') refresh();
      });
    }
    row.hidden = !(main && s.balance > 0 && !s.stopped && !hasBackstop());
  }
  /* a guardian or the paper stop holds a stop button, or an heir is named; not read yet counts as
     yes (say nothing) */
  function hasBackstop() {
    if (!D.isLive()) {
      return (D.state.helpers || []).some(function (x) {
        return x && x.role === 'guardian';
      });
    }
    var pb = ui.pub,
      held = /^(guardian|paper)$/;
    if (!pb || !D.isLocal()) return true;
    if (held.test(slotHolder('cancel2_x')) || held.test(slotHolder('cancel3_x'))) return true;
    return !!(pb.heir_x || pb.heir_address);
  }
  function homeBalance(el, s, main, inPurse, bal, ph) {
    var rateOk = !D.isLive() || D.priceIsLive('day'),
      inMoney =
        rateOk &&
        D.getSetting('balance_money') ===
          'on'; /* dollars on the phone too, but only from a real price */ /* the balance can show in dollars; a tap swaps */
    var b = setHtml(el, '[data-bal]', inMoney && bal != null ? money(bal) : bigKas(bal));
    fit(b);
    setText(
      el,
      '[data-usd]',
      inPurse ? 'KAS in your purse, not yet in the account' : bal == null ? '' : inMoney ? fmt(bal) + ' KAS' : rateOk ? under(bal) : ''
    );
    setHtml(
      el,
      '[data-purse-line]',
      main && !inPurse && s.purse > 0
        ? '<span class="lab">Purse:</span> ' + esc(fmt(s.purse) + ' KAS outside the account.' +
            (s.unopened || s.stopped || (D.isLocal() && s.rulesNext) ? '' : ' Tap to add it.'))
        : ''
    );
    el.querySelector('[data-tour]').hidden = !(D.isLive() && D.getSetting('toured') !== '1' && ph !== 'locked');
    el.querySelector('[data-swap]').classList.toggle('off', !rateOk || bal == null);
  }
  function homeLimitLine(s, main, other, ph) {
    var line;
    if (!main) line = other.limit ? other.limit + ' KAS a day available immediately.' :
      'No covenant: payments are sent immediately, without limits. You can add one at any time.';
    /* the phase is the main account's; another account always shows its own line */ else if (ph === 'locked') line = '';
    else if (ph === 'oldRules') line = s.oldRulesNotice;
    /* a record this app version cannot read; the engine's plain sentence, nothing alarming */ else if (ph === 'rulesNext')
      line = rulesLine(); /* saved, on their way, or ready to finish */
    else if (ph === 'noKeys') line =
      'Your account is not set up yet. Complete setup, then add funds.';
    else if (ph === 'unopened') line =
      'Account not yet open. Add KAS to your purse, then open the account.';
    else if (ph === 'stopped') line = 'Emergency stop active. All funds are in your vault.';
    else if (ph === 'stale') line = 'Updating…'; /* the last numbers it saw, until the engine answers */
    /* connecting or open, before the first numbers */
    else if (s.left == null) line = 'Connecting to the network…';
    else
      /* one statement per line */
      line =
        'Available today: ' + fmt(s.left) + ' KAS' +
        (s.refreshSeconds > 0 ? '\nMore available in: ' +
          (s.refreshSeconds < 60 ? 'under a minute' : dur(s.refreshSeconds)) : '') +
        '\nPayments over ' + fmt(s.limit) + ' KAS wait: ' + dur(s.noticeSeconds);
    return line;
  }
  /* each brief line "Label: value": the label in the accent colour */
  function labelLines(line) {
    return line.split('\n').map(function (l) {
      var at = l.indexOf(': ');
      return at < 0 ? esc(l) : '<span class="lab">' + esc(l.slice(0, at + 1)) + '</span>' +
        esc(l.slice(at + 1));
    }).join('\n');
  }
  function homeGuardState(s, main, other) {
    return main
      ? s.stopKeys == null
        ? null
        : { limit: s.limit, notice: s.noticeSeconds, stops: s.stopKeys, heir: s.heir, sheets: s.sheets, stopped: s.stopped }
      : other.limit
        ? {
            limit: other.limit,
            notice: noticeOf(other),
            stops: other.stopKeys || 1,
            heir: !!other.heir,
            sheets: other.sheets || 1,
            stopped: !!other.stopped,
          }
        : null;
  }
  /* Heir Unlock's colour: five steps, calm ink to stop red, as the period runs out (h0..h4) */
  function heirTone() {
    var sh = heirShare();
    return 'h' + (sh == null ? 0 : Math.min(4, Math.floor(sh * 5)));
  }
  /* Home: the Heir Unlock line while the countdown is known (the red box takes over at the end) */
  function heirNotice(el, hl, hred) {
    var n = el.querySelector('[data-heir]');
    n.className = 'notice heirline ' + heirTone();
    n.textContent = ''; /* the countdown lives in the Heir pip, not atop Home */
  }
  function homeAlarm(el, s, main) {
    var al = main ? D.alertToShow() : null,
      quiet = main && !al && D.guardQuiet(),
      box = el.querySelector('[data-alarm]');
    var hl = main && !s.stopped ? heirLeft() : null,
      hred = hl != null && !al && heirRed(); /* the heir clock, red in the box when close, a soft row when approaching */
    var ah = al
      ? '<div class="alarm-text">' +
        esc(al.text) +
        '</div><div class="alarm-acts">' +
        (s.stopped ? '' : '<button class="alarm-stop" data-go="stop">Emergency stop</button>') +
        '<button class="alarm-ack" data-ack>This was me</button></div>'
      : (hred
          ? '<div class="alarm-text">' +
            (hl > 0 ? 'Your heir can claim in ' + esc(dur(hl)) + '.' : 'Your heir can claim now.') +
            ' Any payment from this account restarts the countdown.</div><div class="alarm-acts"><button class="alarm-ack" data-go="heirclock">See the ' +
            'clock</button></div>'
          : '') + (quiet ? '<div class="alarm-text">' +
            'Alerts on this phone have stopped. Turn them back on in Settings > Keys > ' +
              'Alerts on this phone.' +
            '</div>' : '');
    heirNotice(el, hl, hred);
    if (box.dataset.h !== ah) {
      box.dataset.h = ah;
      box.innerHTML = ah;
      box.hidden = !ah;
      box.className = 'alarm' + (al || hred ? ' red' : quiet ? ' amber' : '');
      var ack = box.querySelector('[data-ack]');
      if (ack)
        ack.onclick = function () {
          D.ackAlert();
        };
    }
  }
  function scanDone(p, text, say, off) {
    if (p.what === 'answer') {
      off();
      var f = ui.onScan;
      ui.onScan = null;
      haptic();
      if (f) f(text);
      else back();
      return;
    }
    var m = /(kaspa(?:test)?:[a-z0-9]{20,})/i.exec(text || '');
    if (!m) {
      say('That code is not a Kaspa address.');
      return;
    }
    off();
    ui.draft.to = m[1].toLowerCase();
    ui.draft.label = '';
    haptic();
    back();
  }
  /* one look per tick, in turn: [whole frame?, longest side in px].
     A long code's tiny squares need the centre at 1080; a code filmed off another
     phone's screen often reads only from a smaller, smoothed copy (the moire averages out);
     the whole frame catches a code held off centre */
  var SCAN_TRIES = [
    [0, 1080],
    [1, 1280],
    [0, 720],
    [0, 480],
    [1, 800],
  ];
  function scanFrame(v, cv, cx, n) {
    var t = SCAN_TRIES[n % SCAN_TRIES.length],
      vw = v.videoWidth,
      vh = v.videoHeight,
      side = Math.min(vw, vh),
      sw = t[0] ? vw : side,
      sh = t[0] ? vh : side,
      o = Math.min(1, t[1] / Math.max(sw, sh));
    cv.width = Math.round(sw * o);
    cv.height = Math.round(sh * o);
    cx.imageSmoothingEnabled = true;
    cx.imageSmoothingQuality = 'high';
    cx.drawImage(v, (vw - sw) / 2, (vh - sh) / 2, sw, sh, 0, 0, cv.width, cv.height);
    var img = cx.getImageData(0, 0, cv.width, cv.height);
    return jsQR(img.data, img.width, img.height, { inversionAttempts: 'dontInvert' });
  }
  /* the phone's own code reader (its camera app's), when this WebView has it; else null */
  function nativeReader() {
    try {
      var B = window.BarcodeDetector;
      return B ? new B({ formats: ['qr_code'] }) : null;
    } catch (e) {
      return null;
    }
  }
  /* one look by the phone's reader, never two at once; if it fails once, jsQR alone from then on */
  function nativeLook(v, st, found) {
    if (!v.videoWidth || !st.reader || st.reading) return;
    st.reading = true;
    st.reader.detect(v).then(
      function (r) {
        st.reading = false;
        if (r && r[0] && r[0].rawValue) found(r[0].rawValue);
      },
      function () {
        st.reading = false;
        st.reader = null;
      }
    );
  }
  /* the back camera at 1920x1080 for Scan; got(stream) once it runs, if Scan is still open */
  function openCamera(say, got) {
    var md = navigator.mediaDevices;
    if (!md || !md.getUserMedia) return say('No camera here.');
    var want = { facingMode: 'environment', width: { ideal: 1920 }, height: { ideal: 1080 } };
    md.getUserMedia({ video: want }).then(
      function (s) {
        if (!ui.cur || ui.cur.name !== 'scan')
          return s.getTracks().forEach(function (t) {
            t.stop();
          });
        keepFocus(s);
        if (KVScene.sleep) KVScene.sleep(true); /* the scene rests only while the camera runs */
        if (!window.jsQR) say('This app cannot read codes from the camera.');
        got(s);
      },
      function () {
        say('The camera was refused. Allow it for KVault in the phone\u2019s settings.');
        if (KVScene.sleep) KVScene.sleep(false);
      }
    );
  }
  /* keep the camera focusing on its own (a close code is soft without it); not every camera can */
  function keepFocus(s) {
    try {
      var t = s.getVideoTracks()[0],
        c = t && t.getCapabilities ? t.getCapabilities() : {};
      if (c.focusMode && c.focusMode.indexOf('continuous') >= 0)
        t.applyConstraints({ advanced: [{ focusMode: 'continuous' }] }).catch(function () {});
    } catch (e) {
      /* fixed focus: the look still runs */
    }
  }
  /* Send's Max: everything in the purse, or what the account can send at once today */
  function sendMax(d, amt) {
    var s = D.state,
      ab = acct(),
      left = ui.acct === 'main' ? s.left : ab.left,
      bal = ab.balance != null ? ab.balance : Infinity,
      v = d.src === 'purse' ? s.purse : Math.min(bal, left != null ? left : Infinity);
    if (typeof v === 'number' && isFinite(v) && v > 0) {
      v = Math.floor(v * 100) / 100;
      amt.value = v > 0 ? plainNum(v) : ''; /* under 0.01: empty = everything */
      amt.oninput();
    }
  }
  /* Send's payee chips: a chip fills To and disarms the slide */
  function wireSendPayees(el, d, to) {
    [].forEach.call(el.querySelectorAll('[data-c]'), function (b) {
      b.onclick = function () {
        disarm(el); /* a payee changes what the slide would pay */
        sendPayees().forEach(function (c) {
          if (c[0] === b.dataset.c) {
            d.to = c[1];
            d.label = c[0];
            to.value = c[0];
          }
        });
        [].forEach.call(el.querySelectorAll('[data-c]'), function (x) {
          x.classList.toggle('on', x === b);
        });
      };
    });
  }
  /* the slide's line for a send: the whole address, then when it goes */
  function sendLine(to, waits) {
    var wait = 'Exceeds the amount left today: it waits ' + dur(D.state.noticeSeconds);
    return 'To ' + addrGroups(to) + '. ' + (waits ? wait + ' and can be stopped.' : 'Goes now.');
  }
  function sendOk(el, d, btn) {
    var n = amountOf(d.amt),
      name = d.label || d.to,
      mustWait = onlyTrusted() && !trustedTo(d);
    if (!d.to) {
      setText(el, '[data-e]', 'Enter a recipient address or choose a payee.');
      return;
    }
    if (!D.canMove()) {
      setText(el, '[data-e]', 'Not connected to the network yet. Try again in a moment.');
      return;
    }
    if (isNaN(n)) {
      setText(el, '[data-e]', AMOUNT_BAD);
      return;
    }
    if (d.src === 'purse') {
      sendFromPurse(el, d, btn, n, name);
      return;
    }
    if (n <= 0) return void setText(el, '[data-e]', 'Enter an amount.');
    var addr = d.to,
      label = d.label;
    var waits = !D.fitsLimit(n) || mustWait,
      who = name.length > 22 ? name.slice(0, 20) + '…' : name;
    var self = btn,
      st0 = D.state,
      qd = st0.pending || null, /* a quieted payment still waits, so the warning still counts it */
      og = st0.outgoing;
    /* a warning before a second payment. A large payment while one is waiting goes in line behind it; a payment while the last one is still in its minute lets that one go now. */
    var warn = sendWarn(waits, qd, og, st0);
    var slide = function () {
      armToSlide(
        addr,
        self,
        'Send ' + fmt(n) + ' KAS to ' + who,
        sendLine(addr, waits),
        false,
        'Slide to send',
        'send',
        function () {
          if (draftMoved(el, addr, n)) return;
          go('waiting', { job: 'send', to: addr, label: label, kas: n, wait: mustWait }, { replace: true });
        }
      );
    };
    if (warn && !warn[2])
      openSheet(
        '<div class="label">' +
          esc(warn[0]) +
          '</div><div class="line lite mt8">' +
          esc(warn[1]) +
          '</div><button class="ghost" data-no>Close</button>'
      );
    else if (warn)
      askChoice(warn[0], warn[1], warn[2]).then(function (yes) {
        if (yes) slide();
      });
    else slide();
  }
  function sendFromPurse(el, d, pself, n, name) {
    /* the purse is a source on Send like any other; same address check, slide and signer as Pay from the purse */
    var pto = d.to.toLowerCase(),
      have = D.state.purse,
      pwho = name.length > 22 ? name.slice(0, 20) + '\u2026' : name;
    if (!isKaspaAddress(pto)) {
      setText(el, '[data-e]', 'That is not a Kaspa address.');
      return;
    }
    if (typeof have === 'number' && n > 0 && n >= Math.floor(have * 100) / 100) n = 0; /* Max or more = everything */
    if (typeof have === 'number' && n > have) {
      setText(el, '[data-e]', 'Your purse holds ' + fmt(have) + ' KAS.');
      return;
    }
    armToSlide(
      pto,
      pself,
      'Send ' + (n > 0 ? fmt(n) + ' KAS' : 'everything in your purse') + ' to ' + pwho,
      'To ' + addrGroups(pto) + '. From your purse. Goes now, no notice period.',
      false,
      'Slide to send',
      'send',
      function () {
        if (draftMoved(el, pto, n)) return;
        pself.disabled = true; /* [data-ok] is gone once the slide replaced it; the captured button stays */
        D.pursePay(pto, n).then(function (r) {
          pself.disabled = false;
          if (!r.ok) {
            resetSlide(el);
            setText(el, '[data-e]', r.error || 'The payment failed. No funds were sent.');
            return;
          }
          haptic();
          var sentKas = n > 0 ? n : have;
          d.to = d.label = d.amt = d.note = d.err = '';
          go('sent', { kas: sentKas, to: pwho, kind: 'purse' }, { replace: true });
        });
      }
    );
  }
  function sendWarn(waits, qd, og, st0) {
    var warn =
      waits && qd && D.isLive()
        ? [
            'A payment is already pending',
            'Finish or stop the waiting withdrawal first.',
            null,
          ] /* the vault takes one withdrawal at a time; no queue in Live */
        : waits && qd
          ? [
              'A payment is already pending',
              fmt(qd.amount) +
                ' KAS to ' +
                qd.to +
                ' is still in its notice period, ' +
                dur(qd.secondsLeft) +
                ' left. This payment is queued behind it: its own ' +
                dur(st0.noticeSeconds) +
                ' starts once that one has left, so it arrives in about ' +
                dur(qd.secondsLeft + st0.noticeSeconds) +
                '. Both can still be stopped.',
              'Queue this payment',
            ]
          : !waits && og && og.secondsLeft > 0
            ? [
                'Your previous payment is still processing',
                fmt(og.amount) +
                  ' KAS to ' +
                  og.to +
                  ' is still in its one-minute cancellation window. Sending another releases it now, and this payment starts its own window.',
                'Send both',
              ]
            : null;
    return warn;
  }
  function wirePendingActs(el, a, me) {
    var f = a.querySelector('[data-finish]'),
      l = a.querySelector('[data-let]'),
      k = a.querySelector('[data-skip]'),
      dc = a.querySelector('[data-decline]'),
      sb = a.querySelector('[data-stop]');
    if (sb)
      sb.onclick = function () {
        /* the same slide as the stop screen */
        armSlide(
          sb,
          'Move all your money to your vault',
          'You will need your vault seeds to access these funds again.',
          true,
          'Slide to stop',
          'stop',
          function () {
            go('waiting', { job: 'stop' }, { replace: true });
          }
        );
      };
    if (dc)
      dc.onclick = function () {
        /* decline = the money stays in the account, ready as before; the stop is the other button */
        armSlide(
          dc,
          'Decline this withdrawal',
          'The money stays in your account, ready as before.',
          false,
          'Slide to decline',
          'stop',
          function () {
            Promise.resolve(D.declinePending()).then(function (r) {
              if (r && r.ok === false) {
                setText(el, '[data-e]', r.error);
                show(ui.cur.name, ui.cur.params);
              } else go('home', {}, { root: true });
            });
          }
        );
      };
    if (f)
      f.onclick = function () {
        armSlide(f, 'Finish your withdrawal', 'The money leaves now.', false, 'Slide to finish', 'send', function () {
          go('waiting', { job: 'finish' }, { replace: true });
        });
      };
    if (l)
      l.onclick = function () {
        D.dismissPending();
        go('home', {}, { root: true });
      };
    if (k)
      k.onclick = function () {
        /* sample: land it now; the lock plays on this screen, then home */
        me.was = 0;
        D.skipAhead();
        setTimeout(function () {
          if (ui.cur && ui.cur.name === 'pending') go('home', {}, { root: true });
        }, me.lockHold);
      };
  }
  /* Pending: what waits and what can be done about it, as [body, acts] */
  function pendingHtml(s, q) {
    var body, acts;
    var to = q.to || 'an address your alerts saw',
      seen = q.amount == null && !q.to; /* a withdrawal only the guard saw */
    body =
      (q.amount == null ? '' : '<div class="big">' + bigKas(q.amount) + '</div>') +
      '<div class="sub">To ' +
      esc(to.length > 30 ? to.slice(0, 28) + '…' : to) +
      ', asked by ' +
      esc(q.who || 'not known') +
      '</div>' +
      (seen ?
        '<div class="sub">Caught by your alerts. Details are not yet available on this ' +
          'phone.</div>' : '') +
      (s.queue && s.queue.length
        ? '<div class="sub">In line behind it: ' +
          s.queue
            .map(function (x) {
              return fmt(x.amount) + ' KAS to ' + esc(x.to);
            })
            .join(', ') +
          '</div>'
        : '');
    acts =
      (q.secondsLeft > 0 || seen || !D.isLive() ? '' :
        '<button class="pill" data-finish>Finish</button>') +
      '<div class="twobtn"><button class="pill" data-decline>Decline it</button><button class="pill" data-let>Approve</button></div>' +
      '<button class="pill danger" data-stop>Stop everything</button>' /* the stop itself, not a way to the stop screen; its own row so the slide has room */ +
      '<div class="sub">The emergency stop moves all funds, including this withdrawal, into your vault. Only your vault seeds can release them.</div>' +
      (D.isLive() ? '' : '<button class="ghost" data-skip>Skip ahead</button>');
    return [body, acts];
  }
  /* Heir clock: the countdown, what it means and the warning when it nears */
  function heirClockBody(left, total, sh) {
    var body;
    var warn =
      left == null || sh == null
        ? ''
        : heirRed()
          ? '<div class="line red">' +
            (left > 0 ? 'Your heir can claim in ' + esc(dur(left)) + '.' :
              'Your heir can claim now.') +
            ' Any payment from this account restarts the countdown.</div>'
          : heirAmber()
            ? '<div class="line amber">Approaching: ' + esc(dur(left)) + ' left.</div>'
            : '';
    body =
      '<div class="big ' + heirTone() + '">' +
      (left == null ? '\u2014' : heirBig(left)) +
      '</div>' +
      '<div class="sub">' +
      (left == null
        ? 'How long is left is not known yet.'
        : left > 0
          ? 'left before your heir can claim'
          : 'The inactivity period has run out.') +
      '</div>' +
      '<div class="line">Your heir can claim this account after it hears nothing from you for ' +
      esc(typeof total === 'number' && total > 0 ? dur(total) : 'the inactivity period set on it') +
      '. Any payment, top-up or stop restarts the clock.</div>' +
      warn;
    return body;
  }
  /* Open the account from the purse: the rules dials (mainnet) or the stored ones (reopening) */
  function fundOpen(el, ok, n, re, an) {
    var dials = re
      ? null
      : D.isMainnet()
        ? {
            cap_kas: ui.liveRules.limit,
            delay: Math.round(ui.liveRules.notice * 36000),
            window: Math.round(ui.liveRules.window * 36000),
          }
        : null; /* DAA units at 10 a second */
    var reopen = !!D.state.stopped; /* a stopped account opens again: a new watch code */
    D.fund(n, dials, an || undefined).then(function (r) {
      ok.disabled = false;
      ok.textContent = re ? 'Open the account again' : an ? 'Open account ' + an :
        'Open the account';
      if (!r.ok) {
        resetSlide(el);
        setText(el, '[data-e]', r.error);
        return;
      }
      haptic();
      ui.acct = 'main';
      if (an)
        D.setAccount(an).then(function () {
          go('home', {}, { root: true });
        });
      else {
        go('home', {}, { root: true }); /* Home shows the account just opened */
        if (reopen) shareNewCode();
      }
    });
  }
  function takeOutSlide(el, ok, btn, n, to) {
    var what = n > 0 ? fmt(n) + ' KAS' : 'everything';
    armToSlide(
      to,
      btn,
      'Take ' + what + ' out of your vault',
      to ? 'To ' + addrGroups(to) + '.' : 'Into your purse.',
      false,
      'Slide to move it',
      'send',
      function () {
        ok.disabled = true;
        setText(el, '[data-e]', 'Opening your vault… about half a minute.');
        D.takeOut([], to, n).then(function (r) {
          ok.disabled = false;
          if (r.ok) {
            haptic();
            go('home', {}, { root: true });
          } else {
            resetSlide(el);
            setText(el, '[data-e]', r.error);
          }
        });
      }
    );
  }
  /* a new step slides in from the side it came from */
  function walkSlide(box, dir) {
    box.classList.add(dir > 0 ? 'fromright' : 'fromleft');
    requestAnimationFrame(function () {
      requestAnimationFrame(function () {
        box.classList.remove('fromright', 'fromleft');
      });
    });
  }
  /* keys just made: a new account opens How it works first (once), then its covenant */
  function keysDone() {
    if (D.getSetting('toured') === '1') return go('keysmade', {}, { replace: true });
    go('howitworks', { intro: true, then: 'keysmade' }, { replace: true });
  }
  /* the walk's Back / Next, and Done (or Open my account) on the last step */
  function walkNavHtml(p) {
    var last = ui.step >= STEPS.length - 1;
    return (ui.step ? '<button class="pill" data-prev>Back</button>' : '') + (last
      ? '<button class="pill" data-done>' + (p.intro ? 'Open my account' : 'Done') + '</button>'
      : '<button class="pill" data-next>Next</button>');
  }
  function walkStepHtml(s) {
    return (
      '<h1 class="head">' +
      esc(s[0]) +
      '</h1><p class="line">' +
      esc(s[1]) +
      '</p><p class="why">' +
      esc(s[2]) +
      '</p>' +
      s[3]
        .map(function (i) {
          return '<button class="howlink" data-story="' + i + '">Full story: ' + esc(STORIES[i][0]) + '</button>';
        })
        .join('')
    );
  }
  function copyPurseAddr(el, addr) {
    if (bridge && bridge.copy) {
      bridge.copy(addr.textContent);
      haptic();
      setText(el, '[data-e]', 'Copied.');
    } else
      (navigator.clipboard ? navigator.clipboard.writeText(addr.textContent) : Promise.reject()).then(
        function () {
          setText(el, '[data-e]', 'Copied.');
        },
        function () {
          setText(el, '[data-e]', 'Hold the address to copy it.');
        }
      );
  }
  function takeOutWordsSlide(el, ok, btn, p, a, b) {
    var what = p.kas > 0 ? fmt(p.kas) + ' KAS' : 'everything';
    armToSlide(
      p.to,
      btn,
      'Take ' + what + ' out of your vault',
      p.to ? 'To ' + addrGroups(p.to) + '.' : 'Into your purse.',
      false,
      'Slide to move it',
      'send',
      function () {
        ok.disabled = true;
        setText(el, '[data-e]', 'Opening your vault… about half a minute.');
        D.takeOut([a, b], p.to, p.kas).then(function (r) {
          a = b = '';
          el.querySelector('[data-w1]').value = '';
          el.querySelector('[data-w2]').value = '';
          ok.disabled = false;
          if (r.ok) {
            haptic();
            go('home', {}, { root: true });
          } else {
            resetSlide(el);
            setText(el, '[data-e]', r.error);
          }
        });
      }
    );
  }
  function homeOrError(el, r) {
    if (r.ok) {
      haptic();
      go('home', {}, { root: true });
    } else {
      resetSlide(el);
      setText(el, '[data-e]', r.error);
    }
  }
  function resetKeysDone(el, r) {
    if (r.ok) {
      haptic();
      ui.acct = 'main';
      go('makekeys', {}, { root: true });
    } else {
      resetSlide(el);
      setText(el, '[data-e]', r.error);
    }
  }
  /* ---- the paper stop (docs/PAPER_STOP.md): an emergency stop button written on paper as 24
     words. They show once, are checked, and are never stored; "Use a paper stop" types
     them on any phone, keys or not. ---- */
  var PAPER_WHAT =
    'A paper stop is an emergency stop button written on paper as 24 words. ' +
    'If you lose this phone, type the words on any phone to stop your account and move everything ' +
    'to your vault, which any two of your three vault seeds open.';
  var PAPER_APPLY =
    'Update now adds it to your account at once. Then print the code to keep with your stop words.';
  var PAPER_KEEP =
    'Write these down on paper. Keep them in a different place from your vault seeds: ' +
    'together they open your account and your vault.';
  var PAPER_FIND = 'First the code printed with your stop words: scan it or paste it.';
  /* the engine walks the account's history (up to 3 minutes, Engine 12): say so once */
  var PAPER_LOOK =
    "Checking your account's history. This can take up to 3 minutes: keep this screen open.";
  /* paper.check's meaning comes from the engine's stage alone: nothing is inferred here */
  var PAPER_GONE = 'Already stopped: the funds are in your vault.';
  /* a long-stopped account's history may no longer be on the network: the take-out still works */
  var PAPER_HINT = 'If you know this account was stopped, take your funds out with two ' +
    'vault seeds. A paper stop printed before your last rules update no longer works: use the ' +
    'newest paper, or two vault seeds.';
  var PAPER_ENDED = 'This account already ended. Nothing is left in it to stop.';
  var PAPER_OLD =
    "This paper's code is out of date. Use a newer code (a guardian's phone has one).";
  var PAPER_SENT = 'Sent. Waiting for the network to show it.';
  /* the engine's own words: one keyless call per 2 s */
  var PAPER_WAIT = 'Please wait a moment and try again.';
  var PAPER_SLOT =
    'Your paper stop takes the place of one emergency stop button. Choose which one. ' +
    'If someone held it, they can no longer stop this account.';
  /* a lost phone, or a fingerprint added or removed (that clears the card, stop 1 and the purse) */
  var LOST_LINE =
    "Don't change your phone's fingerprints or lose this phone: no one could move these funds.";
  /* what every Kaspa node sees (the setup line and Settings > About > Privacy) */
  var NODES_LINE =
    "Kaspa network nodes see your account's addresses and payments, like any Kaspa app.";
  /* Settings > About > Privacy, in short (the full text is docs/store/PRIVACY.md) */
  var PRIVACY = [
    'KVault runs no servers and receives nothing from this app: not your balance, your ' +
      'payments, your addresses, your vault seeds or your IP address. There is no account with us, ' +
      'no analytics, no ads and no crash reports.',
    NODES_LINE +
      ' The app finds a public node through the Kaspa community\u2019s node finder; it ' +
      'and the node see your IP address, and the node sees the addresses of your account, purse ' +
      'and vault and every payment you send. Payments are public and permanent on the Kaspa ' +
      'network. Some public nodes answer without encryption.',
    'The dollar price comes from CoinGecko over an encrypted connection. The request names only ' +
      'Kaspa, the dollar and a number of days, never anything about you.',
    'Link codes and watch codes go from phone to phone on the screen, never through us. Anyone ' +
      'holding your watch code can see your balance and waiting withdrawals.',
    'Your card, purse and emergency stop are sealed on this phone behind your fingerprint. ' +
      'Nothing is copied to a cloud backup: your paper vault seeds are the backup.',
    'The app sends no email, no text messages and no push messages through any service. The ' +
      'alerts are made on this phone.',
  ];
  var PAPER_SHOWN =
    '<div class="err">These words were shown already. ' +
    'To make new ones, open Make a paper stop again.</div>';
  /* one fingerprint; the words stay in page memory until read back and stored (paper.confirm) */
  function paperMake(el, mk, slot) {
    mk.disabled = true;
    setText(el, '[data-e]', '');
    setHtml(el, '[data-pbox]', '');
    D.paperMake(slot).then(function (r) {
      mk.disabled = false;
      var w = r && r.ok && r.words;
      if (!w || w.length !== 24)
        return paperRefused(el, mk, (r && r.error) || 'The paper stop could not be made.');
      ui.paper = w.slice();
      r.words = null;
      ui.paperReview = null; /* paper.confirm answers the review */
      go('papershow', {}, { replace: true });
    });
  }
  /* both free buttons hold guardians: People. The engine cannot tell which button is free, or
     the owner's choice was refused: the owner chooses (again). */
  function paperRefused(el, mk, e) {
    var unsure = /cannot tell whether/i.test(e);
    setText(el, '[data-e]', unsure ? '' : e);
    if (/Remove a guardian first/i.test(e)) return paperPeopleBtn(el);
    if (unsure || /slot/i.test(e)) paperSlotAsk(el, mk);
  }
  /* the buttons keys.public does not name a guardian's, under the warning; the tap is the slot */
  function paperSlotAsk(el, mk) {
    D.keysPublic().then(function (k) {
      if (k && k.ok) {
        ui.pub = k.public || {};
        ui.stops = k.stops || null;
      }
      var free = [2, 3].filter(function (n) {
        return slotHolder(n === 3 ? 'cancel3_x' : 'cancel2_x') !== 'guardian';
      });
      if (!free.length) return paperPeopleBtn(el);
      var btns = free.map(function (n) {
        return '<button class="ghost" data-slot="' + n + '">Use Emergency Stop ' + n +
          '</button><p class="sub">' + esc(paperSlotSay(n)) + '</p>';
      });
      setHtml(el, '[data-pbox]', '<p class="sub">' + esc(PAPER_SLOT) + '</p>' + btns.join(''));
      [].forEach.call(el.querySelectorAll('[data-slot]'), function (b) {
        b.onclick = function () {
          paperMake(el, mk, +b.dataset.slot);
        };
      });
    });
  }
  /* what choosing button n replaces, in one plain sentence. The engine's own word comes first:
     'unknown' (docs/PAPER_STOP.md: it may be an old guardian) never reads as "nobody". */
  function paperSlotSay(n) {
    var k = n === 3 ? 'cancel3_x' : 'cancel2_x',
      s = (ui.stops || []).filter(function (t) {
        return t && t.which === n;
      })[0],
      holder = s && s.holder === 'unknown' ? 'unknown' : slotHolder(k),
      x = (ui.pub || {})[k],
      who = holder === 'unknown' && x && D.helperNames()[x];
    if (holder === 'paper') return 'Replaces your old paper stop. Its words stop working.';
    if (holder === 'you') return 'A spare copy of your own stop. Nobody else holds it.';
    if (holder === 'unheld') return 'Nobody holds this button now.';
    if (who) return 'Replaces ' + who + "'s Emergency Stop " + n + ': ' + who +
      ' can no longer stop this account.';
    return 'If a guardian holds this button from before, they lose it.';
  }
  /* both free buttons hold guardians: the owner removes one first, on People */
  function paperPeopleBtn(el) {
    setHtml(el, '[data-pbox]', '<button class="ghost" data-people>Go to People</button>');
    el.querySelector('[data-people]').onclick = function () {
      go('people', {}, { root: true });
    };
  }
  /* three words read back from paper, at random places */
  function paperAsk() {
    var out = [];
    for (var k = 0; k < 60 && out.length < 3; k++) {
      var n = Math.floor(Math.random() * 24);
      if (out.indexOf(n) < 0) out.push(n);
    }
    return out.length === 3 ? out : [2, 10, 19];
  }
  function paperCheckRun(el) {
    var w = ui.paper;
    if (!w) return back();
    var ask = paperAsk(),
      at = 0;
    function next() {
      if (at === ask.length) return paperChecked();
      var n = ask[at],
        right = w[n],
        opts = [right, w[(n + 7) % 24], w[(n + 15) % 24]].sort();
      setText(el, '[data-q]', 'Paper stop, check ' + (at + 1) + ' of 3');
      setText(el, '[data-w]', 'Word ' + (n + 1));
      el.querySelector('[data-pick]').innerHTML = opts
        .map(function (x) {
          return '<button>' + esc(x) + '</button>';
        })
        .join('');
      [].forEach.call(el.querySelectorAll('[data-pick] button'), function (b) {
        b.onclick = function () {
          if (b.textContent !== right)
            return setText(el, '.sub', 'That word does not match. Please check your paper copy.');
          at++;
          next();
        };
      });
    }
    next();
  }
  function paperChecked() {
    go('paperstop', { step: 'apply' }, { replace: true }); /* its after sends the words on */
  }
  /* the words read back go to paper.confirm after one more fingerprint; it stores the change and
     answers the review lines (rule_words). Only then are the words gone from this phone. */
  function paperConfirm(el, btn) {
    if (!ui.paper && ui.paperReview) return paperReview(ui.paperReview);
    if (!ui.paper) return go('paperstop', {}, { replace: true });
    btn.disabled = true;
    setText(el, '[data-e]', '');
    setHtml(el, '[data-pbox]', '');
    D.paperConfirm(ui.paper).then(function (r) {
      btn.disabled = false;
      if (r && r.ok) {
        ui.paper = null;
        ui.paperReview = r;
        return paperReview(r);
      }
      var e = (r && r.error) || 'The paper stop could not be added. Nothing has changed.';
      setText(el, '[data-e]', e);
      if (/new paper stop|just made/i.test(e)) paperAgainBtn(el);
    });
  }
  /* nothing waits to be stored, or not these words: new words, from the start */
  function paperAgainBtn(el) {
    ui.paper = null;
    setHtml(el, '[data-pbox]', '<button class="ghost" data-again>Make new stop words</button>');
    el.querySelector('[data-again]').onclick = function () {
      go('paperstop', {}, { replace: true });
    };
  }
  /* the stored change goes in with Update now, like a guardian change; the code to print comes
     after the reopen. An answer in rules.change's shape gets the usual review; a bare one gets
     Update now alone. */
  function paperReview(r) {
    var rw = r.rule_words && r.rule_words.length ? r.rule_words : null,
      n = r.slot === 3 ? 3 : 2,
      lines = rw || ['Add your paper stop as Emergency Stop ' + n + '.'];
    if (r.route) return rulesReviewSheet(lines, r, true);
    openSheet(
      '<div class="label">Change the rules</div><div class="line mt8">' + esc(lines[0]) + '</div>' +
        '<div class="line">Update now: takes effect now.</div><div class="err" data-serr></div>' +
        '<button class="pill" data-now>Update now</button><button class="ghost" data-no>Not now</button>'
    );
    var nw = sheet.querySelector('[data-now]');
    nw.onclick = function () {
      nowSlide(nw, true);
    };
  }
  /* Home's papers row: red while the printed stop code is out of date (an old paper stop stops
     nothing after Update now), quiet while any paper is still to do, hidden when all are done */
  function paperDueRow(el) {
    var b = el.querySelector('[data-paper]');
    if (!b || !D.isLocal() || D.state.noKeys) return;
    D.paperState().then(function (st) {
      var rows = paperRowsFor(st),
        c = papersCount(rows),
        old = rows.some(function (r) {
          return r.s === 'old';
        });
      b.className = old ? 'notice red' : 'notice quiet';
      b.textContent = old ? 'Your paper stop is out of date. Print the new code.' :
        'Your papers: ' + c[0] + ' of ' + c[1] + ' done';
      b.hidden = !old && c[0] === c[1];
      b.onclick = function () {
        go(old ? 'paperprint' : 'papers', { from: 'home' });
      };
    });
  }
  /* ---- Your papers: every paper a person needs, its state, and the screen that makes it ---- */
  var PAPER_STATE = { done: ['Done', 'go'], todo: ['To do', ''], old: ['Out of date', 'old'],
    opt: ['Optional', ''] };
  var PAPERS_OLD = 'Your rules changed, so the code printed on your paper stop no longer works. ' +
    'Print the new code and throw the old paper away.';
  var SEEDS_NOT_KEPT = 'This phone did not keep a copy of your vault seeds. Write them down ' +
    'now and keep the papers safe.';
  var SEED_NO_COPY = 'This phone has no copy to check it against. Find your paper and make sure ' +
    'all 24 words are there and easy to read.';
  var PAPER_ASK = {
    o: ['Does your heir have their heir seed on paper?', 'Their heir seed lives on their own ' +
      'phone: under their link code, Show my heir seed. Ask them to write it down.'],
    p: ['Are your papers in separate places?', 'Keep the three vault seeds in three places, so ' +
      'one fire or one thief never finds two. The stop paper can stay with you.'],
  };
  /* all three vault seeds done (the person said so before deleting the phone's copy) or not
     (new keys, new seeds) */
  function seedMarks(on) {
    ['s1', 's2', 's3'].forEach(function (t) {
      D.paperMark(t, on);
    });
  }
  function markRow(m, id, t, d) {
    return { id: id, t: t, s: m.indexOf(id) >= 0 ? 'done' : 'todo', d: d };
  }
  function stopPaperRow(st) {
    var say = !st.has ? ['opt', 'Not made. An emergency stop you can use from any phone.'] :
      !st.due ? ['done', 'Your 24 stop words and the printed code are up to date.'] :
      st.printed ? ['old', 'Print the new code. The old one stopped working when your ' +
        'rules changed.'] :
      ['todo', 'Print the code that goes with your 24 stop words.'];
    return { id: 'stop', t: 'Emergency stop paper', s: say[0], d: say[1] };
  }
  function paperRowsFor(st) {
    var m = D.paperMarks(),
      pub = st.pub || {},
      hk = pub.heir_x || pub.heir_address,
      rows = [1, 2, 3].map(function (n) {
        return markRow(m, 's' + n, 'Vault seed ' + n, m.indexOf('s' + n) >= 0 ?
          'Written on paper and read back.' : 'Write it down, then read it back.');
      });
    rows.push(stopPaperRow(st));
    if (hk) rows.push(markRow(m, 'o', 'Heir seed: ' + (D.helperNames()[hk] || 'your heir'),
      'Your heir keeps their own heir seed. Ask them to write it down.'));
    if (watchedList().some(function (w) {
      return w.roles && w.roles.heir;
    }))
      rows.push(markRow(m, 'h', 'Your heir seed',
        'Lets you claim an account you are heir to if this phone is lost.'));
    rows.push(markRow(m, 'p', 'Kept in separate places',
      'Never two vault seeds together. The stop paper can stay with you.'));
    return rows;
  }
  /* [done, counted]: an optional row (no paper stop made) is not counted */
  function papersCount(rows) {
    var need = rows.filter(function (r) {
      return r.s !== 'opt';
    });
    return [need.filter(function (r) {
      return r.s === 'done';
    }).length, need.length];
  }
  function paperRowHtml(r) {
    var s = PAPER_STATE[r.s];
    return '<button class="item" data-paperrow="' + r.id + '"><span><span class="t">' + esc(r.t) +
      '</span><span class="d">' + esc(r.d) + '</span></span><span class="r ' + s[1] + '">' +
      s[0] + '</span></button>';
  }
  function papersAgain() {
    var t = ui.stack[ui.stack.length - 1];
    if (t && t.name === 'papers') show(t.name, t.params);
  }
  /* a paper the phone cannot check: the person says it is done (Yes) or not yet (No) */
  function papersAsk(id, title, say) {
    openSheet('<div class="label">Your papers</div><div class="line mt8">' + esc(title) +
      '</div><div class="sub">' + esc(say) + '</div><button class="pill" data-yes>Yes, it is ' +
      'done</button><button class="ghost" data-no>Not yet</button>');
    [['[data-yes]', true], ['[data-no]', false]].forEach(function (k) {
      sheet.querySelector(k[0]).onclick = function () {
        ui.closeSheet();
        D.paperMark(id, k[1]);
        papersAgain();
      };
    });
  }
  /* a vault seed: from the sealed copy, show it and read it back; with no copy, ask */
  function seedRecheck(el, n) {
    D.sheets().then(function (r) {
      if (r.ok && r.sheets && r.sheets[n]) {
        ui.sheets = r.sheets;
        return go('sheet', { n: n, one: true });
      }
      if (/deleted from this phone/i.test(r.error || ''))
        return papersAsk('s' + (n + 1), 'Is vault seed ' + (n + 1) + ' on paper?', SEED_NO_COPY);
      setText(el, '[data-e]', r.error || 'The vault seeds could not be shown.');
    });
  }
  function paperRowGo(el, id) {
    setText(el, '[data-e]', '');
    if (/^s[123]$/.test(id)) return seedRecheck(el, +id.charAt(1) - 1);
    if (id === 'stop') return go(ui.paperSt && ui.paperSt.has ? 'paperprint' : 'paperstop',
      { from: 'papers' });
    if (id !== 'h') return papersAsk(id, PAPER_ASK[id][0], PAPER_ASK[id][1]);
    D.heirWords().then(function (r) {
      if (!r.ok) return setText(el, '[data-e]', r.error || 'The heir seed could not be shown.');
      ui.heirWords = r.heir_words || null;
      go('heirsheet', { mark: true });
    });
  }
  function wirePapers(el, p) {
    el.querySelector('[data-done]').onclick = function () {
      if (p.after === 'setup') return keysDone();
      go('home', {}, { root: true });
      if (p.share) shareNewCode();
    };
    D.paperState().then(function (st) {
      var rows = paperRowsFor(st),
        c = papersCount(rows);
      ui.paperSt = st;
      setText(el, '[data-count]', c[0] + ' of ' + c[1] + ' done');
      el.querySelector('[data-old]').hidden = !rows.some(function (r) {
        return r.s === 'old';
      });
      setHtml(el, '[data-rows]', rows.map(paperRowHtml).join(''));
      [].forEach.call(el.querySelectorAll('[data-paperrow]'), function (b) {
        b.onclick = function () {
          paperRowGo(el, b.dataset.paperrow);
        };
      });
    });
  }
  function paperInputs() {
    var h = '';
    for (var i = 1; i <= 24; i++)
      h +=
        '<div><i>' + i + '</i><input data-pw size="8" autocapitalize="none" autocomplete="off" ' +
        'autocorrect="off" spellcheck="false" aria-label="Word ' + i + '"></div>';
    return '<div class="words">' + h + '</div>';
  }
  /* the printed code first (it names the account, and survives the camera), then the words */
  function wirePaperUse(el) {
    var code = el.querySelector('[data-code]');
    code.value = ui.paperCode || '';
    ui.paperCode = ''; /* the code is public; it only rides through the camera screen */
    el.querySelector('[data-scan]').onclick = function () {
      ui.onScan = function (text) {
        ui.paperCode = String(text || '');
        back();
      };
      go('scan', { what: 'answer' });
    };
    el.querySelector('[data-check]').onclick = function () {
      paperFind(el, code.value);
    };
    el.querySelector('[data-stop]').onclick = function () {
      paperStopSlide(el, this);
    };
    wordPicker(el, '[data-pw]');
    if (code.value) paperFind(el, code.value);
  }
  /* stop and seed words the keyboard does not learn: after 3 letters the list words that start
     so show as taps (a tap fills the word; a word box moves on to the next). No paste. */
  function wordPicker(el, sel) {
    var box = el.querySelector('[data-wp]'),
      ins = el.querySelectorAll(sel),
      list = bip39List();
    if (!box) return;
    box.onmousedown = function (e) {
      e.preventDefault(); /* the word box keeps its focus and its keyboard */
    };
    [].forEach.call(ins, function (inp, i) {
      inp.onpaste = function (e) {
        e.preventDefault();
      };
      inp.oninput = function () {
        wordHints(box, list, inp, ins[i + 1]);
      };
    });
  }
  function wordHints(box, list, inp, next) {
    var part = /(\S*)$/.exec(inp.value)[1].toLowerCase(),
      found = [];
    for (var k = 0; part.length >= 3 && k < list.length && found.length < 4; k++)
      if (list[k].indexOf(part) === 0) found.push(list[k]);
    box.hidden = !found.length;
    box.innerHTML = found
      .map(function (w, n) {
        return '<button type="button"' + (n ? '' : ' class="on"') + '>' + esc(w) + '</button>';
      })
      .join('');
    [].forEach.call(box.querySelectorAll('button'), function (b) {
      b.onclick = function () {
        var area = inp.tagName === 'TEXTAREA';
        inp.value = inp.value.replace(/\S*$/, b.textContent) + (area ? ' ' : '');
        box.hidden = true;
        (area || !next ? inp : next).focus();
      };
    });
  }
  function paperFind(el, text) {
    var c = String(text || '').trim();
    if (!c) return setText(el, '[data-e]', 'Scan or paste the printed code.');
    setText(el, '[data-e]', '');
    setText(el, '[data-say]', PAPER_LOOK);
    D.paperCheck(c).then(function (r) {
      var no = paperCheckSays(r);
      setText(el, '[data-ids]', paperIdsLine(r, 'Check this matches your paper: '));
      if (no) {
        setText(el, '[data-say]', PAPER_FIND);
        setText(el, '[data-check]', 'Try again');
        return setText(el, '[data-e]', no);
      }
      el.kvPaper = c;
      setText(el, '[data-say]', paperFoundLine(r));
      el.querySelector('[data-find]').hidden = true;
      el.querySelector('[data-words]').hidden = false;
      el.querySelector('[data-stop]').hidden = false;
    });
  }
  /* stopped only on the engine's stage "stopped"; a moved account is an out-of-date code */
  function paperCheckSays(r) {
    if (r.ok && r.stage === 'stopped') return PAPER_GONE;
    if (r.ok && r.stage === 'ended') return PAPER_ENDED;
    if (r.ok && r.live !== false) return '';
    var e = (!r.ok && r.error) || 'The account could not be found.';
    if (/history could not be checked/i.test(e)) return e + ' ' + PAPER_HINT;
    return /account has moved/i.test(e) ? PAPER_OLD : e;
  }
  /* the account the printed code names, to compare with the paper: its code and its vault */
  function paperIdsLine(r, lead) {
    var c = String((r && r.code) || ''),
      v = String((r && r.vault_address) || '');
    if (!/^[0-9a-f]{8}$/i.test(c)) return '';
    var vault = /^kaspa(test)?:[a-z0-9]{20,100}$/.test(v) ? '. Your vault: ' + v : '';
    return lead + c + vault + '.';
  }
  /* the same two from the watch code itself, to write beside the printed code */
  function paperSheetIds(code) {
    var j = null;
    try {
      j = JSON.parse(code);
    } catch (e) {
      j = null;
    }
    if (!j || typeof j !== 'object') return '';
    var ids = { code: String(j.public_sha256 || '').slice(0, 8),
      vault_address: (j.public && j.public.cold_address) || '' };
    return paperIdsLine(ids, 'Write this on the paper too: account code ');
  }
  function paperFoundLine(r) {
    var l = 'We found your account: ' + fmt(+r.kas || 0) + ' KAS.';
    if (r.stage === 'waiting')
      l += ' A withdrawal is waiting on this account. Stopping also stops it.';
    return l + ' Now type your 24 stop words.';
  }
  /* which word number is wrong, never the word itself */
  function paperWordsError(el) {
    var list = bip39List(), /* the same fail-closed list as the vault seed check */
      ins = el.querySelectorAll('[data-pw]');
    if (list.length !== 2048) return 'The word list is missing. Update the app.'; /* fail closed */
    if (ins.length !== 24) return 'Please enter all 24 words.';
    for (var i = 0; i < 24; i++) {
      var w = ins[i].value.trim().toLowerCase();
      if (!w) return 'Word ' + (i + 1) + ' is missing.';
      if (list.indexOf(w) < 0)
        return 'Word ' + (i + 1) + ' is not on the word list. Please check your paper.';
    }
    return '';
  }
  /* the words go straight from the boxes into the one stop call; the boxes are emptied at once */
  function paperStopSlide(el, btn) {
    var bad = paperWordsError(el);
    setText(el, '[data-e]', bad);
    if (bad) return;
    var say = 'Slide to stop and move everything to your vault',
      title = 'Stop my account';
    armSlide(btn, title, 'Everything moves to your vault.', true, say, 'stop', function () {
      var ins = el.querySelectorAll('[data-pw]'),
        sent = D.paperStop(
          [].map.call(ins, function (inp) {
            return inp.value.trim().toLowerCase();
          }),
          el.kvPaper
        );
      [].forEach.call(ins, function (inp) {
        inp.value = '';
      });
      sent.then(function (x) {
        paperStopDone(el, x);
      });
    });
  }
  function paperStopDone(el, x) {
    if (x && x.ok) {
      haptic();
      var done = { kas: +x.kas || 0, sent: x.confirmed === false,
        ids: paperIdsLine(x, 'Account code ') };
      return go('paperdone', done, { replace: true });
    }
    resetSlide(el);
    setText(el, '[data-ids]', paperIdsLine(x || {}, 'Check this matches your paper: '));
    var e = (x && x.error) || 'The stop did not go through. No funds were moved.';
    setText(el, '[data-e]', e === PAPER_WAIT ? e : e + ' Type your words again to try once more.');
  }
  /* ---- take funds out with two vault seeds: no fingerprint, the seeds are the authority.
     The seeds stay in their boxes until the one call, then the boxes are emptied at once ---- */
  var OUT_FIND = 'First the code of your account: scan it or paste it.';
  var OUT_SENT = 'Sent. Waiting for the network to confirm it.';
  var OUT_ADDR = /^kaspa(test)?:[a-z0-9]{20,100}$/;
  var OUT_UNCHECKED = 'We found your vault, but not the account\'s history. Check the code and ' +
    'vault below match your paper, then type two of your three vault seeds.';
  function seedBox(n) {
    return '<label class="field"><span>Vault seed ' + n + '</span><textarea data-vs ' +
      'autocapitalize="none" autocomplete="off" autocorrect="off" spellcheck="false">' +
      '</textarea></label>';
  }
  /* the purse is offered only on a phone with keys whose purse address is known */
  function outPurse() {
    var a = String(D.state.purseAddress || '');
    return !D.state.noKeys && OUT_ADDR.test(a) ? a : '';
  }
  function paperOutHtml() {
    return (
      topbar() +
      '<div class="label">Two vault seeds</div><h1 class="head">Take your funds out with two vault seeds.</h1>' +
      '<p class="line" data-say>' + OUT_FIND + '</p><p class="addr" data-ids></p>' +
      '<div data-find><label class="field"><span>The account code</span>' +
      '<textarea data-code autocapitalize="none" spellcheck="false"></textarea></label>' +
      '<div class="twobtn"><button class="pill" data-scan>Scan the code</button>' +
      '<button class="pill" data-check>Find my account</button></div></div>' +
      '<div data-seeds hidden>' + seedBox(1) + seedBox(2) +
      '<div class="chips" data-wp hidden></div>' +
      (outPurse() ? '<button class="ghost" data-purse>Send to this phone\'s purse</button>' : '') +
      '<label class="field"><span>Send the funds to this address</span><input data-to ' +
      'autocapitalize="none" autocomplete="off" spellcheck="false"></label></div>' +
      '<div data-rev hidden><p class="line">From your vault:</p><p class="addr" data-rv></p>' +
      '<p class="line">To:</p><p class="addr" data-rt></p></div>' +
      '<div class="err" data-e></div><div class="push"></div>' +
      '<button class="pill" data-next hidden>Review</button>' +
      '<button class="pill" data-out hidden>Take my funds out</button>' +
      '<button class="ghost" data-edit hidden>Change</button>'
    );
  }
  function wirePaperOut(el) {
    var code = el.querySelector('[data-code]'),
      pb = el.querySelector('[data-purse]');
    code.value = ui.paperCode || '';
    ui.paperCode = ''; /* the code is public; it only rides through the camera screen */
    el.querySelector('[data-scan]').onclick = function () {
      ui.onScan = function (text) {
        ui.paperCode = String(text || '');
        back();
      };
      go('scan', { what: 'answer' });
    };
    el.querySelector('[data-check]').onclick = function () {
      paperOutFind(el, code.value);
    };
    if (pb)
      pb.onclick = function () {
        el.querySelector('[data-to]').value = outPurse();
      };
    el.querySelector('[data-next]').onclick = function () {
      if (!el.kvBusy) paperOutReview(el);
    };
    el.querySelector('[data-edit]').onclick = function () {
      if (el.kvBusy) return;
      resetSlide(el);
      paperOutClear(el); /* the vault seeds are typed again, never shown back */
      paperOutStep(el, 'seeds');
    };
    el.querySelector('[data-out]').onclick = function () {
      if (!el.kvBusy) paperOutSlide(el, this);
    };
    wordPicker(el, '[data-vs]');
    if (code.value) paperOutFind(el, code.value);
  }
  function paperOutStep(el, step) {
    el.querySelector('[data-find]').hidden = step !== 'find';
    el.querySelector('[data-seeds]').hidden = step !== 'seeds';
    el.querySelector('[data-next]').hidden = step !== 'seeds';
    el.querySelector('[data-rev]').hidden = step !== 'review';
    el.querySelector('[data-out]').hidden = step !== 'review';
    el.querySelector('[data-edit]').hidden = step !== 'review';
  }
  /* the code names the account and its vault; a stopped account is the usual case here. The
     take-out reads the vault's own coins, so it goes on whenever the vault address came back,
     even when the account's history could not be checked (a long-stopped account) */
  function paperOutFind(el, text) {
    var c = String(text || '').trim();
    if (!c) return setText(el, '[data-e]', 'Scan or paste the account code.');
    setText(el, '[data-e]', '');
    setText(el, '[data-say]', PAPER_LOOK);
    D.paperCheck(c).then(function (r) {
      if (!OUT_ADDR.test(String(r.vault_address || ''))) {
        setText(el, '[data-say]', OUT_FIND);
        setText(el, '[data-check]', 'Try again');
        return setText(el, '[data-e]', (!r.ok && r.error) || 'The account could not be found.');
      }
      el.kvCode = c;
      el.kvVault = r.vault_address;
      setText(el, '[data-say]', r.ok ? 'We found your account. Now type two of your three ' +
        'vault seeds.' : OUT_UNCHECKED);
      setText(el, '[data-ids]', paperIdsLine(r, 'Check this matches your paper: '));
      paperOutStep(el, 'seeds');
    });
  }
  /* which seed and which word number is wrong, never the word itself */
  function seedWordsBad(ws, list) {
    if (ws.length !== 24) return 'please enter all 24 words.';
    for (var i = 0; i < 24; i++)
      if (list.indexOf(ws[i]) < 0) return 'word ' + (i + 1) + ' is not on the word list.';
    return '';
  }
  /* the two seeds as strings, or the reason they are refused (a string) */
  function paperOutSeeds(el) {
    var list = bip39List(), /* the same fail-closed list as the vault seed check */
      ins = el.querySelectorAll('[data-vs]'),
      out = [];
    if (list.length !== 2048) return 'The word list is missing. Update the app.';
    for (var i = 0; i < 2; i++) {
      var ws = ins[i].value.trim().toLowerCase().split(/\s+/).filter(Boolean),
        bad = seedWordsBad(ws, list);
      if (bad) return 'Vault seed ' + (i + 1) + ': ' + bad;
      out.push(ws.join(' '));
    }
    if (out[0] !== out[1]) return out;
    return 'Both entries are the same vault seed. Please type two different vault seeds.';
  }
  function paperOutReview(el) {
    var s = paperOutSeeds(el),
      to = el.querySelector('[data-to]').value.trim();
    if (typeof s === 'string') return setText(el, '[data-e]', s);
    s.length = 0; /* checked only; the call reads the boxes again */
    if (!to) return setText(el, '[data-e]', 'Enter the address the funds should go to.');
    if (!OUT_ADDR.test(to))
      return setText(el, '[data-e]',
        'That address is not right for this network. Check it and try again.');
    setText(el, '[data-e]', '');
    setText(el, '[data-rv]', el.kvVault);
    setText(el, '[data-rt]', to === outPurse() ? to + ' (this phone\'s purse)' : to);
    el.kvTo = to;
    paperOutStep(el, 'review');
  }
  function paperOutSlide(el, btn) {
    var to = el.kvTo;
    armToSlide(to, btn, 'Take your funds out', 'From your vault to ' + to + '.', false,
      'Slide to take your funds out', 'send', function () {
        if (el.kvBusy) return;
        el.kvBusy = true; /* one take-out at a time: Change, Review and the button wait */
        el.querySelector('[data-edit]').hidden = true;
        var s = paperOutSeeds(el),
          purse = to === outPurse();
        paperOutClear(el);
        if (typeof s === 'string') return paperOutDone(el, { ok: false, error: s }, to);
        /* this phone's purse: `to` left out, never null */
        D.paperTakeout(s, el.kvCode, purse ? '' : to).then(function (x) {
          paperOutDone(el, x, to);
        });
      });
  }
  function paperOutClear(el) {
    [].forEach.call(el.querySelectorAll('[data-vs]'), function (t) {
      t.value = '';
    });
    setHtml(el, '[data-wp]', '');
  }
  function paperOutDone(el, x, to) {
    el.kvBusy = false;
    if (x && x.ok) {
      haptic();
      var done = { kas: +x.kas || 0, sent: x.confirmed === false, to: String(x.to || to) };
      return go('paperoutdone', done, { replace: true });
    }
    resetSlide(el);
    paperOutStep(el, 'seeds');
    var e = (x && x.error) || 'The funds could not be moved. Nothing was sent.';
    var again = ' Type your vault seeds again to try once more.';
    setText(el, '[data-e]', e === PAPER_WAIT ? e : e + again);
  }
  /* a watched account's code, to take its funds out with two vault seeds from any phone */
  function wireWatchedCode(el, id) {
    var code = '';
    el.querySelector('[data-wcode]').onclick = function () {
      D.watchedCode(id).then(function (r) {
        code = (r && r.ok && r.watch_code) || '';
        if (!code) return setText(el, '[data-e]', (r && r.error) || 'The code could not be made.');
        var svg = KVQR.svg(code);
        if (svg) setHtml(el, '[data-wq]', svg);
        el.querySelector('[data-wqbox]').hidden = !svg;
        el.querySelector('[data-wcopy]').hidden = false;
        setText(el, '[data-e]', '');
      });
    };
    el.querySelector('[data-wcopy]').onclick = function () {
      if (!code || !bridge || !bridge.copy) return;
      bridge.copy(code);
      haptic();
      setText(el, '[data-e]', 'Copied.');
    };
  }
  /* Coins: the account's one coin, then the purse's (Sample: Everyday's) coins from
     D.coinsInfo: how many, the total, what combining costs and what is left after */
  function coinRow(t, d, kas) {
    return '<div class="item"><span><span class="t">' + esc(t) + '</span><span class="d">' +
      esc(d) + '</span></span><span class="r">' + fmt(kas) + ' KAS</span></div>';
  }
  function coinsList(c) {
    var big = c.amounts.slice().sort(function (a, b) {
        return b - a;
      }),
      shown = big.slice(0, 8).map(function (x) {
      return fmt(x);
    });
    if (c.count > 8) shown.push('and ' + (c.count - 8) + ' more');
    return c.count + (c.count === 1 ? ' coin' : ' coins') +
      (c.count ? ': ' + shown.join(', ') : '');
  }
  function coinsSay(c) {
    if (!c) return 'Counting your coins\u2026';
    if (!c.ok) return c.error || 'Your coins could not be counted. Try again in a moment.';
    if (!c.count) return 'Your purse is empty.';
    if (c.count < 2) return 'Nothing to combine.';
    if (c.fee == null || c.total - c.fee <= c.fee)
      return 'These coins cannot be combined in one step.';
    var line = 'Combining ' + c.count + ' coins into one costs about ' + fmt(c.fee) +
      ' KAS. After: one coin of ' + fmt(c.total - c.fee) + ' KAS.';
    return D.isLive() ? line + ' Combining is coming in an update.' : line;
  }
  function coinsPaint(el, c) {
    var s = D.state,
      name = D.isLive() ? 'Purse' : s.everyday && s.everyday.name,
      rows = coinRow(s.account || 'Your account', 'One coin. Your rules keep it that way.',
        s.balance);
    if (!name) c = { ok: false, error: 'Nothing to combine.' }; /* Sample, no Everyday */
    if (c && c.ok && c.count) rows += coinRow(name, coinsList(c), c.total);
    setHtml(el, '[data-l]', rows);
    setText(el, '[data-say]', coinsSay(c));
    var b = el.querySelector('[data-ok]');
    if (b) b.disabled = D.isLive() || !c || !c.ok || c.count < 2 || c.fee == null ||
      c.total - c.fee <= c.fee;
  }
  /* share text: the phone's share sheet (Bridge.share, Engine 12), else the browser's, else
     copy it with a plain line in [sel]. Android's WebView has no navigator.share. */
  function shareText(el, sel, t) {
    try {
      if (bridge && bridge.share) return bridge.share(String(t).slice(0, 8000));
    } catch (e) {
      /* no share sheet on this phone: fall through to copy */
    }
    if (navigator.share) return navigator.share({ text: t }).catch(function () {});
    var done = function () {
      setText(el, sel, 'Copied. Paste it into a message or an email within a minute.');
    };
    if (bridge && bridge.copy) {
      bridge.copy(t); /* Bridge.copy clears the clipboard after a minute */
      return done();
    }
    (navigator.clipboard ? navigator.clipboard.writeText(t) : Promise.reject()).then(done,
      function () {
        setText(el, sel, 'This phone could not share it.');
      });
  }
  /* the statement as plain text: one movement per line, then the totals the screen shows */
  /* KAS a statement line paid out (0 for anything else): the screen and the shared text agree */
  /* a row's amount: the engine's kas (paid, received, fund), else Sample's text */
  function rowKas(a) {
    if (typeof a.kas === 'number') return a.kas;
    var m = /([\d.,]+) KAS/.exec(a.text);
    return a.kind === 'paid' && m ? parseFloat(m[1].replace(/,/g, '')) : 0;
  }
  function sameMonth(at, now) {
    var d = new Date(at * 1000);
    return d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth();
  }
  /* the statement: once the engine dates its lines (`at`), this month's dated rows plus the
     older undated ones, and only dated rows count; no dates on Live = Recent activity, no
     totals. In = received + fund (the opening deposit is money in), Out = paid */
  function statementData() {
    var now = new Date(),
      h = D.state.history || [],
      dated = h.some(function (a) {
        return typeof a.at === 'number';
      }),
      d = { month: dated || !D.isLive(), out: 0, inn: 0, rows: [] };
    h.forEach(function (a) {
      var has = typeof a.at === 'number';
      if (has && !sameMonth(a.at, now)) return;
      var when = has ? alertWhen(a.at) : a.when || (dated ? 'Date not recorded' : '');
      d.rows.push({ a: a, when: when });
      if (dated && !has) return;
      if (a.kind === 'paid') d.out += rowKas(a);
      else if (a.kind === 'received' || a.kind === 'fund') d.inn += rowKas(a);
    });
    return d;
  }
  function statementTotals(d) {
    var bal = 'Balance now ' + fmt(D.state.balance) + ' KAS.';
    if (!d.month) return bal;
    return 'In this month: ' + fmt(d.inn) + ' KAS. Out this month: ' + fmt(d.out) +
      ' KAS' + (D.isLive() ? '' : ', ' + money(d.out)) + '. ' + bal;
  }
  function statementText() {
    var d = statementData(),
      lines = d.rows.map(function (r) {
        return (r.when ? r.when + ': ' : '') + r.a.text;
      });
    return ['KVault statement, ' + (D.state.account || 'your account') + ', ' +
      (d.month ? 'this month' : 'recent activity')]
      .concat(lines.length ? lines : ['Nothing yet.'])
      .concat([statementTotals(d)])
      .join('\n');
  }
  function statementRowHtml(r) {
    var a = r.a,
      v = rowKas(a),
      inn = a.kind === 'received' || a.kind === 'fund',
      sign = a.kind === 'paid' ? '\u2212' : inn ? '+' : '';
    return '<div class="item"><span><span class="t">' + esc(a.text) +
      '</span><span class="d">' + esc(r.when) + '</span></span><span class="r">' +
      (sign && v ? sign + fmt(v) : '') + '</span></div>';
  }
  /* a code's text (the same the picture holds), to send by message when their camera cannot scan */
  function copyCode(el, code) {
    if (!code) return;
    if (bridge && bridge.copy) {
      bridge.copy(code);
      haptic();
      return setText(el, '[data-e]', 'Copied. Paste it into a message within a minute.');
    }
    (navigator.clipboard ? navigator.clipboard.writeText(code) : Promise.reject()).then(
      function () {
        setText(el, '[data-e]', 'Copied.');
      },
      function () {
        setText(el, '[data-e]', 'The code could not be copied.');
      }
    );
  }
  /* a broken camera: the same code, pasted, goes down the same path as a scanned one */
  function wirePaste(el, done, say) {
    var box = el.querySelector('[data-paste]');
    function go1() {
      var t = String(box.value || '').trim().slice(0, 4000);
      if (!t) return say('Paste the code first.');
      done(t);
    }
    el.querySelector('[data-pastego]').onclick = go1;
    /* a paste goes in by itself; the code's own check refuses junk in plain words */
    box.addEventListener('paste', function () {
      setTimeout(go1, 0);
    });
  }
  /* the lock screen's PIN field: six digits are checked against the panic PIN (hashed); a match
     opens the duress view, anything else says no in the words of an ordinary wrong PIN */
  /* wrong PINs: five free, then a wait doubling from 30 s, up to an hour (until app restart) */
  function pinMiss(el) {
    ui.pinFails = (ui.pinFails || 0) + 1;
    var n = ui.pinFails - 5;
    if (n >= 0) ui.pinUntil = Date.now() + Math.min(3600, 30 * Math.pow(2, Math.min(n, 7))) * 1000;
    setText(el, '[data-e]',
      'Not your PIN. Use your fingerprint, or the PIN in the phone\u2019s own prompt.');
  }
  function lockPin(el, pinIn) {
    return function () {
      var v = pinIn.value.replace(/\D/g, '');
      if (v.length < 6) return;
      pinIn.value = '';
      var wait = Math.ceil(((ui.pinUntil || 0) - Date.now()) / 1000);
      if (wait > 0) return setText(el, '[data-e]', 'Too many tries. Wait ' + wait + ' seconds.');
      D.panicMatch(v).then(function (yes) {
        if (ui.el !== el || !ui.cur || ui.cur.name !== 'lock') return;
        if (!yes) return pinMiss(el);
        ui.pinFails = 0;
        D.setDuress(true);
        ui.duress = true;
        ui.afterLock = null;
        go('duress', {}, { root: true });
      });
    };
  }
  /* the duress view's one screen: the purse as if it were all there is (balance, Send, Receive) */
  function duressHtml() {
    var a = String(D.state.purseAddress || '');
    return (
      '<div class="top"><span class="label">KVault</span></div><div class="push"></div>' +
      '<div class="big" data-bal>' + esc(D.state.purse != null ? fmt(D.state.purse) : '\u2026') +
      '</div><div class="line">KAS available</div><div class="err" data-e></div>' +
      '<div class="push"></div><button class="pill" data-go="pursepay">Send</button>' +
      '<button class="ghost" data-showqr>Receive</button><div class="qrbox" data-qrbox hidden>' +
      '<div class="qr" data-qr>' + (a ? KVQR.svg(a) : '') + '</div><div class="sub" data-addr>' +
      esc(a) + '</div></div>'
    );
  }
  function duressAfter(el) {
    el.querySelector('[data-showqr]').onclick = function () {
      el.querySelector('[data-qrbox]').hidden = false;
      this.hidden = true;
    };
    D.funding().then(function (r) {
      if (!r || !r.ok || ui.el !== el) return;
      if (typeof r.kas === 'number') setText(el, '[data-bal]', fmt(r.kas));
      if (!D.state.purseAddress && r.address) {
        el.querySelector('[data-qr]').innerHTML = KVQR.svg(r.address);
        setText(el, '[data-addr]', r.address);
      }
    });
  }
  /* a PIN anyone would try first: one digit six times, or a straight run up or down */
  function panicWeak(v) {
    return /^(\d)\1{5}$/.test(v) || '0123456789'.indexOf(v) >= 0 || '9876543210'.indexOf(v) >= 0;
  }
  /* the panic PIN screen: type it, type it again, then a fresh fingerprint keeps it (hashed) */
  function panicPinAfter(el) {
    var first = '',
      inp = el.querySelector('[data-p]');
    function err(t) {
      inp.value = '';
      setText(el, '[data-e]', t);
    }
    function keep(v) {
      confirmAction('Set your panic PIN', '').then(function (yes) {
        if (!yes) return err('Your fingerprint is needed to set it.');
        D.panicSet(v).then(function (ok) {
          if (!ok) return err('This phone cannot keep a panic PIN safely.');
          go('settings', {}, { replace: true });
        });
      });
    }
    el.querySelector('[data-ok]').onclick = function () {
      var v = inp.value.replace(/\D/g, '');
      if (v.length !== 6) return err('Six digits.');
      if (!first && panicWeak(v)) return err('Choose a PIN that is harder to guess.');
      if (!first) {
        first = v;
        inp.value = '';
        setText(el, '[data-pl]', 'Type it again');
        return setText(el, '[data-e]', '');
      }
      if (v !== first) {
        first = '';
        setText(el, '[data-pl]', 'Choose a PIN, 6 digits');
        return err('The two PINs did not match. Start again.');
      }
      keep(v);
    };
    var off = el.querySelector('[data-off]');
    if (off)
      off.onclick = function () {
        confirmAction('Remove your panic PIN', '').then(function (yes) {
          if (!yes) return;
          D.panicClear();
          go('settings', {}, { replace: true });
        });
      };
  }
  var SCR = {
    /* the duress view, opened by the panic PIN on the lock screen: the purse only. No tabs, no
       menu; go() keeps every other screen out and the data layer refuses every other action. */
    duress: {
      money: true,
      html: duressHtml,
      after: duressAfter,
    },
    /* 0. Lock: the door in front of an account that exists */
    lock: {
      money: true,
      scene: function () {
        return { mode: 'safe', snap: true, mark: 'k' };
      } /* the seal on the unlock screen: our K, traced in fast on arrival */,
      html: function () {
        return (
          '<div class="top"><span class="label">KVault</span>' +
          (D.isLive() ? '' : tag()) +
          '</div><div class="push"></div>' +
          '<button class="sealtap" data-ok aria-label="Open"></button>' /* a glowing centre button, nothing prompts until it is touched */ /* no name, no line; touching the centre of the seal opens */ +
          '<button class="ghost small" data-pin>Use a PIN</button><label class="field" data-pinbox ' +
          'hidden><span>PIN</span><input data-pinv inputmode="numeric" maxlength="6" ' +
          'autocomplete="off" aria-label="PIN"></label><div class="err" data-e></div>'
        );
      },
      after: function (el) {
        var pinBtn = el.querySelector('[data-pin]'),
          pinIn = el.querySelector('[data-pinv]');
        if (pinBtn)
          pinBtn.onclick = function () {
            el.querySelector('[data-pinbox]').hidden = false;
            pinBtn.hidden = true;
            pinIn.focus();
          };
        if (pinIn) pinIn.oninput = lockPin(el, pinIn);
        el.querySelector('[data-ok]').onclick = function () {
          var p =
            D.isLive() && D.phase() === 'locked'
              ? D.retryUnlock()
              : confirmAction('Unlock KVault', ''); /* one fingerprint, then Home */
          p.then(function (yes) {
            if (!yes) return;
            /* the phone's prompt takes the phone's own PIN too, which whoever forced the app
               open may know: the duress view ends only when the app is closed and started again */
            if (ui.duress) return go('duress', {}, { root: true });
            go('home', {}, { root: true });
            if (ui.afterLock) {
              go(ui.afterLock, {});
              ui.afterLock = null;
            }
          });
        };
        if (bridge && shown())
          setTimeout(function () {
            if (ui.cur && ui.cur.name === 'lock' && !ui.bio &&
              shown()) el.querySelector('[data-ok]').click();
          }, 350); /* the prompt opens by itself (no bar) */
      },
      live: function () {},
    },
    /* 1. Welcome */
    welcome: {
      sample: true /* needs demo.js: go() sends Live Home */,
      money: true,
      html: function () {
        return (
          '<div class="label">KVault</div><div class="push"></div><h1 class="head">Savings with a daily limit and an emergency stop.</h1>' +
          '<button class="pill" data-go="words" data-params=\'{"mode":"wallet"}\'>Open a new account</button>' +
          '<button class="ghost" data-go="restore">Restore with my recovery seed</button><button class="ghost" data-go="invite">I was invited</button>' +
          '<button class="ghost" data-go="paperuse">Use a paper stop</button>' +
          '<button class="ghost" data-go="paperout">Take funds out with two vault seeds</button>'
        );
      },
    },
    words: {
      sample: true /* needs demo.js: go() sends Live Home */,
      locked: false,
      html: function (p) {
        var safe = p.mode === 'safe';
        return (
          topbar(tag()) +
          '<h1 class="head">' +
          (safe ? 'Your vault seed' : 'Your recovery seed') +
          '</h1><p class="line">' +
          (safe
            ? 'Write these on paper. They open your vault.'
            : 'Write these words on paper. They restore your account on a new phone.') +
          ' You will see them once.</p>' +
          '<div class="words" data-words></div><div class="sub" data-veil>Confirm with your fingerprint to see them.</div><div class="push"></div>' +
          '<button class="pill" data-go="wordcheck" data-params=\'' +
          JSON.stringify({ mode: p.mode }) +
          "' disabled>I wrote them down</button>"
        );
      },
      /* the words are not even put on the page until the fingerprint says so */
      after: function (el, p) {
        confirmAction(
          p.mode === 'safe' ? 'Show your vault seed' : 'Show your recovery seed',
          'Make sure nobody is looking over your shoulder.'
        ).then(function (yes) {
          if (!yes) {
            back();
            return;
          }
          el.querySelector('[data-words]').innerHTML = WORDS.map(function (w, i) {
            return '<div><i>' + (i + 1) + '</i>' + esc(w) + '</div>';
          }).join('');
          el.querySelector('[data-veil]').remove();
          el.querySelector('[data-go="wordcheck"]').disabled = false;
        });
      },
    },
    wordcheck: {
      sample: true /* needs demo.js: go() sends Live Home */,
      html: function () {
        return (
          topbar(tag()) +
          '<div class="label" data-q></div><h1 class="head" data-w></h1><div class="sub">Tap the right word</div><div class="list pick" data-pick></div><div class="push"></div>'
        );
      },
      after: function (el, p) {
        var ask = [6, 13, 21],
          at = 0;
        function next() {
          if (at === ask.length) {
            if (p.mode === 'safe') return go('movein', {});
            if (p.mode === 'check') {
              back();
              back();
              return;
            }
            D.setSetting('welcomed', '1');
            return go('howitworks', { intro: true }, { root: true }); /* a new wallet starts with the walk-through */
          }
          var n = ask[at],
            right = WORDS[n];
          var opts = [right, WORDS[(n + 7) % 24], WORDS[(n + 15) % 24]].sort();
          setText(el, '[data-q]', 'Check ' + (at + 1) + ' of 3');
          setText(el, '[data-w]', 'Word ' + (n + 1));
          el.querySelector('[data-pick]').innerHTML = opts
            .map(function (w) {
              return '<button>' + esc(w) + '</button>';
            })
            .join('');
          [].forEach.call(el.querySelectorAll('[data-pick] button'), function (b) {
            b.onclick = function () {
              if (b.textContent === right) {
                at++;
                next();
              } else setText(el, '.sub', 'That word does not match. Please check your paper copy.');
            };
          });
        }
        next();
      },
    },
    restore: {
      sample: true /* needs demo.js: go() sends Live Home */,
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Restore your account</h1><label class="field"><span>Your 24-word recovery seed</span><textarea data-w autocapitalize="none" ' +
          'spellcheck="false"></textarea></label><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>Bring it back</button><button class="ghost" data-fill>Use the sample seed</button>'
        );
      },
      after: function (el) {
        var w = el.querySelector('[data-w]');
        el.querySelector('[data-fill]').onclick = function () {
          w.value = WORDS.join(' ');
        };
        el.querySelector('[data-ok]').onclick = function () {
          if (w.value.trim().split(/\s+/).length !== 24) {
            setText(el, '[data-e]', 'Please enter all 24 words.');
            return;
          }
          D.setSetting('welcomed', '1');
          go('home', {}, { root: true });
        };
      },
    },

    /* 2. Home */
    home: {
      money: true,
      tab: 'home',
      scene: function () {
        return D.phase() === 'locked' && ui.acct === 'main' ? { mode: 'safe', snap: true } : {};
      } /* the lock seal while the account waits for the button */,
      html: function () {
        return (
          '<div class="top"><button class="acctbtn" data-pick aria-label="Change account"><span data-acct-name></span><svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 4l4 4 4-4"/></svg></button>' +
          '<span class="topright"><button class="bell" data-go="alerts" aria-label="Alerts"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 3a5 5 0 0 0-5 5v3l-1.5 2.5h13L15 11V8a5 5 0 0 0-5-5zM8 16a2 2 0 0 0 4 0"/></svg></button><button class="menu" data-menu aria-label="Menu"><i></i><i></i><i></i></button></span></div>' +
          '<div class="alarm" data-alarm hidden></div><button class="notice" data-tour hidden>New to KVault? Take the tour.</button><button ' +
          'class="notice quiet" data-notice></button><button class="notice amber" data-heir></button>' +
          '<button class="notice" data-paper hidden></button>' +
          '<div class="err" data-e></div><div class="push"></div>' +
          '<div class="big" data-bal></div><button class="denom" data-swap aria-label="Show in the other unit">' +
          '<span data-usd></span><svg viewBox="0 0 16 16" aria-hidden="true">' +
          '<path d="M5 2v10M2 9l3 3 3-3M11 14V4M8 7l3-3 3 3"/></svg></button>' +
          '<div class="line lines" data-limit></div><button class="notice quiet" data-purse-line></button>' +
          '<button class="sealtap" data-unlock hidden aria-label="Open"></button>' /* the seal itself is the button */ +
          '<div class="guards" data-guards></div>' +
          '<div class="sub" data-lone hidden>If you lose this phone, only a guardian can move this ' +
          'account to your vault. <span data-lonemore hidden>' + esc(LOST_LINE) + ' </span>' +
          '<button class="readmore" data-lonex>Read more</button> ' +
          '<button class="ghost" data-go="people">Add a guardian</button></div>' +
          '<button class="pill" data-rulesfin hidden>Finish updating my rules</button>' +
          '<div class="list" data-watched></div>' /* accounts this phone helps */ +
          '<div class="log" data-log></div>' +
          '<div class="row" data-row></div>'
        );
      },
      after: function (el) {
        var swap = function () {
          if (D.isLive() && !D.priceIsLive('day')) return;
          D.setSetting('balance_money', D.getSetting('balance_money') === 'on' ? 'off' : 'on');
          haptic();
          refresh();
        };
        el.querySelector('[data-bal]').onclick = swap;
        el.querySelector('[data-swap]').onclick = swap;
        el.querySelector('[data-lonex]').onclick = function () {
          var m = el.querySelector('[data-lonemore]');
          m.hidden = !m.hidden; /* the guardian note folded to its first line; Read more opens it */
          this.textContent = m.hidden ? 'Read more' : 'Show less';
        };
        el.querySelector('[data-watched]').addEventListener('click', function (e) {
          var b = e.target.closest('[data-wx]');
          if (!b) return;
          ui.wOpen = ui.wOpen || {};
          ui.wOpen[b.dataset.wx] = !ui.wOpen[b.dataset.wx];
          refresh();
        });
        el.querySelector('[data-pick]').onclick = openAccounts;
        el.querySelector('[data-menu]').onclick = openMenu;
        el.querySelector('[data-notice]').onclick = function () {
          ui.notice = '';
          refresh();
        }; /* the one-time notice goes on a tap */
        el.querySelector('[data-heir]').onclick = function () {
          go('heirclock');
        }; /* the heir clock, when it is approaching */
        el.querySelector('[data-tour]').onclick = function () {
          D.setSetting('toured', '1');
          go('howitworks', {});
        };
        if (D.isLocal() && D.fetchPrices && !D.priceIsLive('day'))
          D.fetchPrices('day').then(function (v) {
            if (v && ui.cur && ui.cur.name === 'home') refresh();
          }); /* the real rate for the dollar view */
        el.querySelector('[data-purse-line]').onclick = function () {
          var s = D.state;
          if (D.isLocal() && s.rulesNext) return;
          if (s.purse > 0 && !s.unopened && !s.stopped) go('topup', {});
          else if (s.unopened) go('fundacct', {});
        }; /* the first open offers the walk */
        el.querySelector('[data-rulesfin]').onclick = function () {
          rulesFinishSlide(this);
        };
        paperDueRow(el);
      },
      live: function (el) {
        var s = D.state,
          main = ui.acct === 'main',
          other = null,
          ph = D.phase(); /* the page's one state model (data.js phase()) */
        if (!main)
          other =
            ui.acct === 'everyday'
              ? s.everyday
              : s.extra.filter(function (a) {
                  return a.name === ui.acct;
                })[0];
        if (!main && !other) {
          ui.acct = 'main';
          main = true;
        }
        var inPurse = main && s.unopened && s.purse > 0,
          bal = main ? (inPurse ? s.purse : s.balance) : other.balance; /* before the account opens, the purse is the balance */
        var anm = String((main ? s.account : other.name) || '');
        if (anm.length > 16) anm = anm.slice(0, 15) + '\u2026'; /* no overflow clip on the span: it cut the halation into a box */
        var an = setText(el, '[data-acct-name]', anm);
        if (an) an.setAttribute('data-t', an.textContent);
        var t = setText(el, '[data-tag]', D.isLive() ? '' : 'Sample');
        if (t) t.title = t.textContent;
        homeBalance(el, s, main, inPurse, bal, ph);
        var line;
        var unopened = main && !!s.unopened; /* keys made, nothing in the covenant yet */
        line = homeLimitLine(s, main, other, ph);
        var brief = /^Available today: /.test(line),
          cut = line.indexOf('\n');
        /* the brief folds to its first line; the purse line opens with it */
        if (brief)
          setHtml(el, '[data-limit]', foldHead('brief', labelLines(cut < 0 ? line : line.slice(0, cut)), 'bare') +
            (foldOpen('brief') && cut >= 0 ? labelLines(line.slice(cut)) : ''));
        else setText(el, '[data-limit]', line);
        el.querySelector('[data-purse-line]').hidden = brief && !foldOpen('brief');
        var lock = el.querySelector('[data-unlock]');
        lock.hidden = !(ph === 'locked' && main);
        lock.onclick = function () {
          D.retryUnlock();
        };
        el.classList.toggle('sealed', ph === 'locked' && main); /* while sealed: no account name, no balance, no bars; the seal alone */
        /* the pips come from the engine's status (stop_keys, heir, sheets); until it has answered they stay dim */
        setHtml(el, '[data-guards]', guardsHtml(homeGuardState(s, main, other)));
        homeLoneRow(el, s, main && ph !== 'locked');
        var p = s.pending,
          banner = !!(p && !p.dismissed); /* not yet acknowledged: the bell's dot is red */
        setHtml(el, '[data-log]', homeLogHtml());
        setHtml(el, '[data-watched]', D.isLocal() && main ? homeHelpedHtml() : '');
        if (D.isLocal()) D.watchTick();
        setText(el, '[data-notice]', ui.notice || ''); /* the phone's one-time notice; empty hides the row */
        setText(el, '[data-e]', s.error);
        var bell = el.querySelector('.bell');
        bellLight(bell, !!p || !!s.outgoing, banner, main);
        /* the watcher's word: red for a withdrawal or lineage break not yet owned up to, amber when the guard is silent.
           The stop button opens the usual stop screen (slide, then fingerprint); "I did this" hides this one alert only. */
        homeAlarm(el, s, main);
        var r = '';
        var pw = '<button data-go="paperuse">Use a paper stop</button>';
        if (main && s.noKeys) r = '<button data-go="makekeys">Make my keys</button>' + pw;
        else if (unopened) r = '<button data-go="receive">Put money in</button><button data-go="fundacct">Open the account</button>';
        setHtml(el, '[data-row]', r);
        var rf = el.querySelector('[data-rulesfin]');
        if (rf) rf.hidden = !(main && D.rulesReady());
      },
    },

    /* 3. Open an account */
    open: {
      tab: 'home',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Open an account</h1><div class="list">' +
          '<button class="item" data-kind="Everyday"><span><span class="t">Everyday</span><span class="d">Spend and receive, like a current account.</span></span></button>' +
          '<button class="item" data-kind="Protected"><span><span class="t">Protected</span><span class="d">Under a covenant: a daily limit, a notice period and an emergency stop.</span></span></button></div>'
        );
      },
      after: function (el) {
        [].forEach.call(el.querySelectorAll('[data-kind]'), function (b) {
          b.onclick = function () {
            if (b.dataset.kind === 'Protected') {
              ui.fresh = { limit: FRESH_RULES.limit, notice: FRESH_RULES.notice, window: FRESH_RULES.window, heir: FRESH_RULES.heir };
              return go('words', { mode: 'safe' });
            }
            D.addAccount({ name: 'Everyday ' + (D.state.extra.length + 2), balance: 0 });
            go('home', {}, { root: true });
          };
        });
      },
    },
    movein: {
      html: function () {
        var e = D.state.everyday;
        return (
          '<div class="top"><span class="label">Last step</span>' +
          tag() +
          '</div><h1 class="head">Move money in</h1><p class="line">From Everyday. You have ' +
          fmt(e.balance) +
          ' KAS there.</p>' +
          '<label class="field"><span>Amount</span><div class="amount"><input data-a inputmode="decimal" aria-label="Amount" ' +
          'placeholder="0"><span>KAS</span></div></label><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>Move it in</button><button class="ghost" data-skip>Later</button>'
        );
      },
      after: function (el) {
        function done(kas) {
          var name = 'Protected ' + (D.state.extra.length + 1);
          D.addAccount({
            name: name,
            balance: 0,
            limit: ui.fresh.limit,
            notice: ui.fresh.notice,
            window: ui.fresh.window,
            heir: ui.fresh.heir,
          });
          if (kas > 0) D.moveIn(kas, name);
          ui.acct = name;
          go('home', {}, { root: true });
          setTimeout(explainScreen, 700); /* a new account opens with its Home explained */
        }
        el.querySelector('[data-ok]').onclick = function () {
          var n = amountOf(el.querySelector('[data-a]').value);
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          done(n);
        };
        el.querySelector('[data-skip]').onclick = function () {
          done(0);
        };
      },
    },

    /* after a covenant change: the rules now in force, the cost, then Home */
    covenantset: {
      money: true,
      scene: function () {
        return { mode: 'locked' };
      },
      html: function (p) {
        var fee = D.isLive()
          ? D.state.fee != null
            ? 'Network fee ' + D.state.fee + ' KAS, under 1 cent.'
            : 'A small network fee is added, under 1 cent.'
          : 'No fee in Sample mode.';
        return (
          '<div class="label">Covenant</div><div class="push"></div><h1 class="head">Covenant locked</h1><div class="line">' +
          esc(p.text || '') +
          '</div><div class="sub">' +
          fee +
          ' The Kaspa network enforces these rules from now on.</div>' +
          '<button class="pill" data-home>Home</button>'
        );
      },
      after: function (el) {
        if (KVScene.pulse) KVScene.pulse('send');
        el.querySelector('[data-home]').onclick = function () {
          go('home', {}, { root: true });
        };
        setTimeout(function () {
          if (ui.cur && ui.cur.name === 'covenantset') go('home', {}, { root: true });
        }, 7000);
      },
    },
    /* the KAS price, on its own screen */
    price: {
      tab: 'price',
      money: true,
      noScene: true /* no tendrils behind the chart */,
      scene: function () {
        return {};
      } /* no second line behind the chart mirroring it */,
      html: function () {
        return (
          '<div class="top"><span class="label">KAS price</span>' +
          tag() +
          menuBtn() +
          '</div><div class="psum" data-psum></div><div class="chart" data-chart></div>' +
          '<div data-supply></div><div class="push"></div>' +
          '<div class="sub mt8">The price of one KAS in dollars. Your money is counted in KAS; its dollar value moves with this line.</div>'
        );
      },
      after: function (el) {
        chartInto(el.querySelector('[data-chart]'));
        supplyInto(el.querySelector('[data-supply]'));
      },
    },
    /* the camera reads a Kaspa address code into the Send screen. The phone grants only our page the camera, video only. */
    scan: {
      money: true,
      html: function (p) {
        var ans = p.what === 'answer';
        return (
          topbar(tag()) +
          '<div class="label">' +
          esc(p.step || (ans ? 'Scan the answer' : 'Scan')) +
          '</div>' + (p.head ? '<h1 class="head">' + esc(p.head) + '</h1>' : '') +
          '<div class="scanbox"><video data-v autoplay playsinline muted></video><i class="scanframe"></i></div><div class="sub center" data-say>' +
          esc(p.say || (ans ? 'Point the camera at the code on the other phone.' :
            'Point the camera at a Kaspa address code.')) +
          '</div><label class="field"><span>No camera? Paste the code here</span>' +
          '<textarea data-paste maxlength="4000" autocomplete="off" spellcheck="false"></textarea></label>' +
          '<button class="ghost" data-pastego>Paste the code instead</button>' +
          '<div class="push"></div>' +
          (ans && !D.isLive() ? '<button class="ghost" data-sample>Use a sample answer</button>' : '')
        );
      },
      after: function (el, p) {
        var sb = el.querySelector('[data-sample]');
        if (sb)
          sb.onclick = function () {
            done('sample-answer');
          };
        wirePaste(el, done, say);
        var v = el.querySelector('[data-v]'),
          stream = null,
          stop = false,
          cv = document.createElement('canvas'),
          cx = cv.getContext('2d', { willReadFrequently: true }),
          nat = { reader: nativeReader(), reading: false, looks: 0 };
        function say(t) {
          setText(el, '[data-say]', t);
        }
        function off() {
          stop = true;
          if (stream)
            stream.getTracks().forEach(function (t) {
              t.stop();
            });
          stream = null;
          v.srcObject = null;
          if (KVScene.sleep) KVScene.sleep(false);
        }
        function done(text) {
          if (!stop) scanDone(p, text, say, off); /* a late reader answer after we left: dropped */
        }
        function tick() {
          if (stop) return;
          if (!ui.cur || ui.cur.name !== 'scan') {
            off();
            return;
          }
          nativeLook(v, nat, done);
          if (v.videoWidth && window.jsQR) {
            var q = scanFrame(v, cv, cx, nat.looks++);
            if (q && q.data) {
              done(q.data);
              if (stop) return;
            }
          }
          setTimeout(tick, 160); /* each look a different size: a full turn in under a second */
        }
        function start() {
          openCamera(say, function (s) {
            stream = s;
            v.srcObject = s;
            tick();
          });
        }
        window.KV_camera = function (ok) {
          if (ok) start();
          else say('The camera was refused.');
          return true;
        };
        if (bridge && bridge.askCamera) bridge.askCamera();
        else start();
      },
    },

    /* 4. Send */
    send: {
      money: true,
      tab: 'send',
      scene: function () {
        var s = D.state,
          n = amountOf(ui.draft.amt) || 0,
          pct = s.limit ? (((s.left || 0) - n) / s.limit) * 100 : 100;
        if (s.stopped) return {};
        return { mode: 'sending', left: n > 0 && !D.fitsLimit(n) ? 30 : pct }; /* the Send tab shows its shape: diamonds leaving the core */
      },
      html: function () {
        var live = D.isLive(),
          d = ui.draft;
        return (
          (D.isLocal() && typeof D.state.purse === 'number'
            ? '<div class="dial"><div class="h"><span>Pay from</span></div><span class="seg" data-src><button data-v="acct"' +
              (d.src === 'purse' ? '' : ' class="on"') +
              '>My account</button><button data-v="purse"' +
              (d.src === 'purse' ? ' class="on"' : '') +
              '>My purse</button></span></div>'
            : '') /* the purse is a source right on the Send screen */ +
          '<div class="sub" data-balline></div><div class="push"></div><label class="field"><span>To</span><input data-to placeholder="Address or name" autocapitalize="none" spellcheck="false" value="' +
          esc(d.label || d.to) +
          '"></label><button class="howlink" data-go="scan">Scan a code</button><div class="sub" data-clip></div>' +
          (function (ps) {
            return ps.length
              ? '<div class="chips"' +
                  (d.src === 'purse' ? ' hidden' : '') +
                  '>' +
                  ps
                    .map(function (c) {
                      var nm = esc(c[0]),
                        on = d.label === c[0] ? ' class="on"' : '';
                      return '<button data-c="' + nm + '"' + on + '>' + nm + '</button>';
                    })
                    .join('') +
                  (live ? '' : '<button data-addnote>Note</button>') +
                  '</div>'
              : '';
          })(sendPayees()) +
          '<label class="field mt8" data-notef' +
          (d.note ? '' : ' hidden') +
          '><input data-note aria-label="Note" placeholder="Add a note" value="' +
          esc(d.note) +
          '"></label>' +
          '<div class="amount mt14"><input data-amt inputmode="decimal" aria-label="Amount in KAS" placeholder="0" value="' +
          esc(d.amt) +
          '"><span>KAS</span><button class="maxb" data-max type="button">Max</button></div>' +
          '<div class="sub" data-usd></div><div class="line" data-say></div><div class="err" data-e>' +
          esc(d.err) +
          '</div><button class="pill" data-ok>Send</button>'
        );
      },
      after: function (el) {
        var d = ui.draft,
          to = el.querySelector('[data-to]'),
          amt = el.querySelector('[data-amt]'),
          note = el.querySelector('[data-note]');
        to.oninput = function () {
          disarm(el);
          d.to = to.value.trim();
          d.label = '';
          [].forEach.call(el.querySelectorAll('[data-c]'), function (b) {
            b.classList.remove('on');
          });
        };
        amt.oninput = function () {
          disarm(el);
          amt.value = amt.value.replace(/[^0-9.]/g, '').replace(/(\..*)\./g, '$1');
          d.amt = amt.value;
          d.err = '';
          setText(el, '[data-e]', '');
          refresh();
          applyScene();
        };
        note.oninput = function () {
          d.note = note.value;
        };
        el.querySelector('[data-max]').onclick = function () {
          /* Max = everything in the purse, or what the account can send immediately today */
          sendMax(d, amt);
        };
        wireSeg(el, '[data-src]', function (b) {
          disarm(el);
          d.src = b.dataset.v;
          d.err = '';
          setText(el, '[data-e]', '');
          var ch = el.querySelector('.chips');
          if (ch) ch.hidden = d.src === 'purse';
          refresh();
          applyScene();
        });
        var addNote = el.querySelector('[data-addnote]');
        if (addNote)
          addNote.onclick = function () {
            el.querySelector('[data-notef]').hidden = false;
            note.focus();
          };
        wireSendPayees(el, d, to);
        el.querySelector('[data-ok]').onclick = function () {
          sendOk(el, d, this);
        };
      },
      live: function (el) {
        var s = D.state,
          n = amountOf(ui.draft.amt) || 0,
          say = '',
          btn = el.querySelector('[data-ok]'),
          d = ui.draft,
          watch = !D.isLive() && D.getSetting('watch_only') === 'on';
        var offline =
          !D.canMove(); /* no Goes now from stale numbers while the network is unreachable; the network comes before the phase on a money screen */
        /* refactor note: Send puts the network ahead of everything, D.phase() puts it after locked..stopped (Home's order), and a
           stopped account is often stale at open; so Send still reads the flags, not the phase, until that is settled */
        if (offline) say = 'Not connected';
        else if (watch) say = 'This phone only watches. Send from your main phone.';
        else if (s.stopped) say = 'This account is stopped. Everything is in your vault.';
        else if (n > 0 && onlyTrusted() && !trustedTo(d))
          say = 'Not a trusted payee: arrives in ' + dur(s.noticeSeconds) + ' unless stopped';
        else if (n > 0)
          say = D.fitsLimit(n)
            ? 'Goes now'
            : 'More than what is left today (' + fmt(s.left) + ' KAS): arrives in ' + dur(s.noticeSeconds) + ' unless stopped';
        setText(el, '[data-say]', say);
        var purse = d.src === 'purse';
        if (purse) {
          if (!offline)
            setText(
              el,
              '[data-say]',
              n > 0 ? 'From your purse. Goes now, no notice period.' : 'Leave the amount empty to send everything in your purse.'
            );
          setText(
            el,
            '[data-balline]',
            typeof s.purse === 'number' ? fmt(s.purse) + ' KAS in your purse, outside the account and its rules' : 'Reading the purse\u2026'
          );
        } else {
          var ab = acct(),
            lf = ui.acct === 'main' ? s.left : ab.left;
          setText(
            el,
            '[data-balline]',
            (ab.balance == null ? '' : fmt(ab.balance) + ' KAS in ' + (ui.acct === 'main' ? s.account : ab.name)) +
              (lf != null && ab.limit ? ', ' + fmt(lf) + ' available immediately today' : '')
          );
        }
        /* pasted address check: the start and end to compare with where it was copied from (a clipboard swap changes both) */
        var clip =
          D.getSetting('clip_check') !== 'off' && !d.label && isKaspaAddress(d.to)
            ? 'Check it against where you copied it: starts ' + d.to.slice(0, 12) + ', ends ' + d.to.slice(-6) + '.'
            : '';
        setText(el, '[data-clip]', clip);
        var fee = s.fee != null ? 'Fee ' + s.fee + ' KAS, under 1 cent' : 'A small fee is added';
        setText(el, '[data-usd]', n > 0 ? (D.isLive() ? fee : under(n) + '. ' + fee) : purse ? fee : fmt(s.left) + ' KAS left today');
        if (btn) {
          btn.disabled = (!!s.stopped && !purse) || watch || offline;
          btn.textContent =
            purse && !(n > 0)
              ? 'Send everything'
              : 'Send'; /* always Send; the line above says when it waits. The purse sends while the account is stopped */
        }
      },
    },
    /* the calm wait while money moves */
    waiting: {
      money: true,
      locked: true,
      /* no glass: one clean full-screen animation while money moves (half glass cut it in two) */
      bare: true,
      scene: function (p) {
        return p.job === 'stop' ? { mode: 'stopped', k: 0.9, slow: true } : p.job === 'send' ? { mode: 'send', left: 30 } : {};
      },
      html: function (p) {
        if (p.job === 'rules')
          return '<div class="push"></div><div class="breath"></div><h1 class="head">Updating your rules\u2026</h1><p class="sub" data-rstep></p><div data-rout></div>';
        var title =
          p.job === 'stop'
            ? 'Moving everything to your vault'
            : p.job === 'finish'
              ? 'Finishing your withdrawal'
              : 'Sending ' + fmt(p.kas) + ' KAS';
        return (
          '<div class="push"></div><div class="breath"></div><h1 class="head">' +
          title +
          '</h1><p class="sub">' +
          (D.isLive() ? 'This takes about 30 seconds. Please keep this screen open.' :
            'One moment.') +
          '</p>'
        );
      },
      after: function (el, p) {
        var reopen = p.job === 'rules' && reopensAfterStop();
        if (p.job === 'rules')
          return (p.now ? D.updateNow(rulesStep(el)) : D.finishRules(rulesStep(el))).then(function (r) {
            if (r.ok) {
              haptic();
              if (p.paper) return go('papers', { share: reopen }, { root: true });
              if (ui.guide) return go('watchcode', { guide: ui.guide }, { root: true });
              go('home', {}, { root: true });
              return reopen && shareNewCode();
            }
            setHtml(
              el,
              '[data-rout]',
              '<div class="err">' + esc(r.error || 'That did not go through.') + '</div><button class="pill" data-home>Home</button>'
            );
            var h = el.querySelector('[data-home]');
            if (h)
              h.onclick = function () {
                go('home', {}, { root: true });
              };
          }); /* a refusal stops here in the engine's words; Home keeps "Finish updating my rules" to try again */
        if (p.job === 'stop')
          return D.stop().then(function (r) {
            if (r.ok) go('stopdone', {}, { replace: true });
            else go('stop', { err: r.error }, { replace: true });
          });
        if (p.job === 'finish')
          return D.finish().then(function (r) {
            go(r.ok ? 'home' : 'pending', { err: r.error }, r.ok ? { root: true } : { replace: true });
          });
        D.send(p.to, p.kas, p.label, p.wait).then(function (r) {
          if (!r.ok) {
            ui.draft.err = r.error || 'The payment failed. No funds were sent.';
            return go('send', {}, { replace: true });
          }
          var d = ui.draft;
          d.to = d.label = d.amt = d.note = d.err = '';
          go('sent', { kas: p.kas, to: p.label || p.to, kind: r.kind }, { replace: true });
        });
      },
    },
    sent: {
      money: true,
      locked: false,
      scene: function (p) {
        return {}; /* a waiting payment shows the glowing lines, not the fuse */
      },
      html: function (p) {
        var pending = p.kind === 'pending' || p.kind === 'queued',
          to = String(p.to || ''),
          who = to.length > 24 ? to.slice(0, 22) + '…' : to;
        return (
          '<div class="label">' +
          (pending ? 'Pending withdrawal' : 'Sent') +
          '</div><div class="push"></div><div class="big">' +
          bigKas(p.kas) +
          '</div><div class="sub">To ' +
          esc(who) +
          '</div>' +
          '<div class="line" data-say></div>' +
          (pending ? '<button class="pill" data-go="pending">See it</button>' : '') +
          '<button class="' +
          (pending ? 'ghost' : 'pill') +
          '" data-home>Done</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-home]').onclick = function () {
          go('home', {}, { root: true });
        };
      },
      live: function (el, p) {
        var s = D.state;
        setText(
          el,
          '[data-say]',
          p.kind === 'purse'
            ? 'From your purse, at once.' + (typeof s.purse === 'number' ? ' ' + fmt(s.purse) + ' KAS left in it.' : '')
            : p.kind === 'queued'
              ? 'It waits in line: its notice period starts when the one before it has left.'
              : p.kind === 'pending'
                ? 'Arrives in ' + dur(s.pending ? s.pending.secondsLeft : s.noticeSeconds) +
                  ' unless you stop it.'
                : fmt(s.left) + ' KAS left today.'
        );
      },
    },

    /* 5. Receive */
    receive: {
      tab: 'receive',
      money: true /* money: the scene is not dimmed (the diamonds must show) */,
      scene: function () {
        return { mode: 'receiving' };
      },
      html: function () {
        return (
          '<div class="top"><span class="label">Receive</span>' +
          tag() +
          menuBtn() +
          '</div><div class="push"></div>' +
          '<button class="pill" data-showqr>Show the code</button><div class="qrbox" data-qrbox hidden><div class="qr">' +
          (rcvAddr() ? KVQR.svg(rcvAddr()) : '') +
          '</div></div>' /* the code comes up on a button, not by itself */ +
          '<button class="rcv-addr" data-addr aria-label="Your address; tap to show all of it">' +
          esc(shortAddr(rcvAddr() || '…')) +
          '</button>' +
          '<div class="sub center">' +
          esc(D.isLocal() ? 'Your purse' : D.state.account || 'Your account') +
          '. Send only Kaspa to it.</div><div class="sub center" data-purse></div><div class="err center" data-e></div><div class="push"></div>' +
          '<div class="twobtn"><button class="pill" data-copy>Copy</button><button class="pill" data-share>Share</button></div>'
        );
      },
      after: function (el) {
        var a = el.querySelector('[data-addr]'),
          full = false,
          A = rcvAddr();
        function paint() {
          A = rcvAddr();
          a.textContent = full ? A : shortAddr(A || '…');
          setHtml(el, '.qr', A ? KVQR.svg(A) : '');
        }
        if (D.isLocal())
          readPurse(el, function (r) {
            /* the purse and what it holds, from the engine */
            ui.purse = r.address;
            setText(el, '[data-purse]', 'It holds ' + fmt(r.kas) + ' KAS.');
            paint();
          });
        a.onclick = function () {
          full = !full;
          paint();
        };
        var sq = el.querySelector('[data-showqr]');
        sq.onclick = function () {
          var box = el.querySelector('[data-qrbox]');
          box.hidden = !box.hidden;
          sq.textContent = box.hidden ? 'Show the code' : 'Hide the code';
          glow(sq);
          haptic();
        }; /* the code can be hidden again */
        el.querySelector('[data-copy]').onclick = function () {
          if (!A) return;
          glow(el.querySelector('[data-copy]'));
          if (bridge && bridge.copy) {
            bridge.copy(A);
            haptic();
            setText(el, '[data-e]', 'Copied.');
            return;
          }
          (navigator.clipboard ? navigator.clipboard.writeText(A) : Promise.reject()).then(
            function () {
              setText(el, '[data-e]', 'Copied.');
            },
            function () {
              full = true;
              paint();
              setText(el, '[data-e]', 'Hold the address to copy it.');
            }
          );
        };
        el.querySelector('[data-share]').onclick = function () {
          glow(el.querySelector('[data-share]'));
          if (A) shareText(el, '[data-e]', A);
        };
      },
    },

    /* 6. Pending withdrawals */
    pending: {
      money: true,
      lockHold: 1600 /* ms the locked shape stays once the withdrawal has landed (the fuse locks when the time is up) */,
      scene: function () {
        return {}; /* the glowing lines while it waits and when it lands; the fuse is gone from countdowns */
      },
      html: function (p) {
        return (
          topbar(D.isLive() ? '' : tag()) +
          '<div class="label">Pending withdrawals</div><div class="push"></div><div data-body></div><div class="err" data-e>' +
          esc(p.err) +
          '</div><div data-acts></div>'
        );
      },
      live: function (el) {
        var s = D.state,
          q = s.pending,
          body,
          acts,
          me = SCR.pending,
          now = Date.now();
        /* the lock plays once when the time is up: live keeps the withdrawal at 0 s, the sample lands it on that very tick (q already gone), so the last second seen counts too */
        var ready = q ? q.secondsLeft != null && q.secondsLeft <= 0 : me.was != null && me.was <= 1 && now - (me.seenAt || 0) < 3000;
        if (ready && !me.lockAt) {
          me.lockAt = now;
          if (KVScene.pulse) KVScene.pulse('lock');
        }
        if (q && q.secondsLeft > 0) me.lockAt = 0;
        me.was = q ? q.secondsLeft : null;
        me.seenAt = now;
        if (!q) {
          body = '<h1 class="head">' + (me.lockAt && now - me.lockAt < me.lockHold ? 'Completed' :
            'Nothing is waiting') + '</h1>';
          acts = '';
        } else {
          var h = pendingHtml(s, q);
          body = h[0];
          acts = h[1];
        }
        var b = setHtml(el, '[data-body]', body),
          a = setHtml(el, '[data-acts]', acts);
        if (q) {
          var c = el.querySelector('[data-cd]');
          if (!c) {
            c = document.createElement('div');
            c.className = 'line';
            c.setAttribute('data-cd', '');
            b.appendChild(c);
          }
          c.textContent =
            q.secondsLeft == null
              ? 'When it lands is not known yet.'
              : q.secondsLeft > 0
                ? 'Lands in ' + clock(q.secondsLeft)
                : D.isLive()
                  ? 'Ready to finish'
                  : 'Landing now';
          var big = b.querySelector('.big');
          if (big) fit(big);
        }
        wirePendingActs(el, a, me);
      },
    },

    /* 6b. The heir clock: like the pending screen, how long until the heir can claim, with a warning as it nears */
    heirclock: {
      money: true,
      scene: function () {
        return {};
      } /* the glowing lines, as on every countdown */,
      html: function () {
        return (
          topbar(D.isLive() ? '' : tag()) +
          '<div class="label">Heir Unlock</div><div class="push"></div><div data-body></div><div data-acts></div>'
        );
      },
      live: function (el) {
        var s = D.state,
          left = heirLeft(),
          total = s.heirDelaySeconds,
          sh = heirShare(),
          body,
          acts;
        if (!s.heir) {
          body =
            '<h1 class="head">No heir named</h1><div class="line">An heir can claim what is in ' +
              'this account if it hears nothing from you for the inactivity period you set. ' +
              'Never what is in your vault: only your vault seed opens it.</div>';
          acts = D.isLive()
            ? '<button class="pill" data-heir-set>Name an heir</button>'
            : '<button class="pill" data-heir-add>Name an heir</button>'; /* heir.set on this phone */
        } else {
          body = heirClockBody(left, total, sh);
          acts = D.isLive()
            ? '<div class="sub">' +
              'Any payment or top-up from this phone restarts it.</div><button class="ghost" data-heir-set>Change the heir</button>'
            : '<button class="pill" data-renew>Restart the clock</button><button class="ghost" data-skip>Skip ahead</button>'; /* heir.set on this phone */
        }
        var b = setHtml(el, '[data-body]', body),
          a = setHtml(el, '[data-acts]', acts);
        var big = b && b.querySelector('.big');
        if (big) fit(big);
        var r = a && a.querySelector('[data-renew]'),
          k = a && a.querySelector('[data-skip]'),
          ha = a && a.querySelector('[data-heir-add]');
        if (r)
          r.onclick = function () {
            D.renewClock();
            haptic();
            refresh();
            applyScene();
          };
        if (k)
          k.onclick = function () {
            D.skipHeir();
            refresh();
            applyScene();
          }; /* sample: a fifth of the silence passes, to see the warning and the lock */
        if (ha)
          ha.onclick = function () {
            go('persondetail', { kind: 'Heir' });
          };
        var hs = a && a.querySelector('[data-heir-set]');
        if (hs) hs.onclick = openHeirSheet;
      },
    },

    /* 7. Emergency stop */
    stop: {
      money: true,
      scene: function () {
        return { mode: 'stopped', k: ui.drag * 0.85, left: 100, charge: ui.drag, heat: ui.drag };
      } /* green to red as the slide goes */,
      html: function (p) {
        return (
          topbar() +
          '<div class="label">Emergency stop</div><div class="push"></div><div class="head red">Stop everything</div><div class="line" data-say></div>' +
          '<div class="err" data-e>' +
          esc(p.err) +
          '</div>' +
          '<button class="pill danger" data-ok>Stop everything</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-ok]').onclick = function () {
          armSlide(
            this,
            'Move all your money to your vault',
            'You will need your vault seeds to access these funds again.',
            true,
            'Slide to stop',
            'stop',
            function () {
              go('waiting', { job: 'stop' }, { replace: true });
            }
          );
        };
      },
      live: function (el) {
        setText(
          el,
          '[data-say]',
          'This moves all ' +
            fmt(D.state.balance) +
            ' KAS in ' +
            D.state.account +
            ' into your vault. You will need your vault seeds to access these funds again.'
        );
      },
    },
    stopdone: {
      money: true,
      scene: function () {
        return { mode: 'stopped', k: 1, slow: true };
      },
      html: function () {
        return '<div class="label">Stopped</div><div class="push"></div><div class="big" data-b>' +
          '</div><div class="sub">In your vault</div><div class="line">No phone can spend these ' +
          'funds. Only your vault seeds can release them.</div><button class="pill" ' +
          'data-go="safe">Open your vault</button><button class="ghost" data-home>Home</button>';
      },
      after: function (el) {
        el.querySelector('[data-home]').onclick = function () {
          go('home', {}, { root: true });
        };
      },
      live: function (el) {
        var b = setHtml(el, '[data-b]', bigKas(D.state.safe));
        fit(b);
      },
    },

    /* 8. Safe */
    safe: {
      money: true,
      tab: 'safe',
      scene: function () {
        return { mode: 'locked' };
      } /* the hexagons on the vault */,
      html: function () {
        return (
          topbar() +
          '<div class="label">Vault</div><div class="push"></div><div class="big" data-b></div><div class="sub" data-usd></div>' +
          '<div class="line mt8">Three vault seeds; any two of them open it.</div>' +
          (D.isLocal() ? '<button class="ghost" data-go="showsheets">Show my seeds</button>' : '') +
          '<div class="list" data-stops></div><div class="push"></div>' +
          '<button class="pill danger" data-go="stop" data-vstop hidden>Emergency stop</button>' +
          '<button class="pill" data-go="takeout">Take money out</button>'
        );
      },
      live: function (el) {
        var s = D.state,
          b = setHtml(el, '[data-b]', bigKas(s.safe));
        fit(b);
        setText(el, '[data-usd]', s.safe == null ? '' : under(s.safe));
        /* the emergency stop lives here (moved off Home): main account, open, not stopped */
        el.querySelector('[data-vstop]').hidden = !(ui.acct === 'main' && !s.stopped && !s.noKeys &&
          !s.unopened);
        var stops = D.isLive()
          ? s.history.filter(function (h) {
              return /stop/i.test(h.text);
            })
          : s.stops;
        setHtml(
          el,
          '[data-stops]',
          stops
            .slice(0, 2)
            .map(function (h) {
              return (
                '<div class="item"><span><span class="t">' + esc(h.text) + '</span><span class="d">' + esc(h.when) + '</span></span></div>'
              );
            })
            .join('')
        );
      },
    },
    /* Purse -> account, once the account is open (engine topup) */
    topup: {
      tab: 'home',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Add to your account</h1><p class="sub">Money moves from the purse into your account and comes under its rules.</p>' +
          '<div class="sub" data-purse>Reading the purse\u2026</div>' +
          '<label class="field"><span>Amount, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" placeholder="All of it"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Add it</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]'),
          have = null,
          amt = el.querySelector('[data-amt]');
        ok.disabled = true;
        readPurse(el, function (r) {
          have = r.kas;
          ok.disabled = !(have >= 1.05);
          el.querySelector('[data-max]').onclick = function () {
            if (have >= 1.05) amt.value = String(Math.floor((have - 1) * 100) / 100);
          };
          if (have >= 1.05) amt.placeholder = 'All of it: ' + fmt(Math.floor((have - 1) * 100) / 100);
        });
        ok.onclick = function () {
          var n = amountOf(amt.value);
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          if (!n) n = Math.floor((have - 1) * 100) / 100;
          if (n <= 0) {
            setText(el, '[data-e]', 'Type an amount.');
            return;
          }
          if (have != null && n + 1 > have) {
            setText(el, '[data-e]', 'The purse holds ' + fmt(have) + ' KAS. Leave about 1 KAS for the network fee.');
            return;
          }
          ok.disabled = true;
          ok.textContent = 'Adding\u2026';
          setText(el, '[data-e]', '');
          D.topup(n).then(function (r) {
            ok.disabled = false;
            ok.textContent = 'Add it';
            if (r.ok) {
              haptic();
              go('home', {}, { root: true });
            } else {
              resetSlide(el);
              setText(el, '[data-e]', r.error);
            }
          });
        };
      },
    },
    takeout: {
      money: true,
      scene: function () {
        return { mode: 'locked' };
      },
      html: function () {
        if (D.isLocal()) {
          var body =
            '<div class="line">Your vault opens with any two of your three vault seeds, typed from paper. Nothing on this phone can open it alone.</div>' +
            '<div class="sub">In your vault: ' +
            fmt(D.state.safe) +
            ' KAS.</div>' +
            '<label class="field"><span>How much, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" placeholder="All of it"></label>' +
            '<div class="dial"><div class="h"><span>Where to</span></div><span class="seg" data-to><button data-v="purse" class="on">My purse</button><button data-v="other">Another address</button></span></div>' +
            '<label class="field" data-addrbox hidden><span>Kaspa address</span><input data-addr autocapitalize="none" spellcheck="false" placeholder="kaspa:…"></label>';
          return (
            topbar() +
            '<h1 class="head">Take money out</h1>' +
            body +
            '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Next: your seeds</button>'
          );
        }
        return (
          topbar(tag()) +
          '<h1 class="head">Take money out</h1><div class="line">Your vault opens with two of your three vault seeds, typed from paper.</div><div ' +
          'class="push"></div>' +
          '<button class="pill" data-go="takeoutwords">Type the words from paper</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]');
        if (!ok) return;
        wireFold(el);
        var box = el.querySelector('[data-addrbox]');
        el.querySelector('[data-max]').onclick = function () {
          if (D.state.safe != null) el.querySelector('[data-amt]').value = String(Math.floor(D.state.safe * 100) / 100);
        };
        wireSeg(el, '[data-to]', function (b) {
          box.hidden = b.dataset.v !== 'other';
        });
        ok.onclick = function () {
          var n = amountOf(el.querySelector('[data-amt]').value),
            other = el.querySelector('[data-to] .on').dataset.v === 'other',
            to = other ? el.querySelector('[data-addr]').value.trim().toLowerCase() : '';
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          if (other && !isKaspaAddress(to)) {
            setText(el, '[data-e]', 'That is not a Kaspa address.');
            return;
          }
          if (n > 0 && D.state.safe != null && n >= Math.floor(D.state.safe * 100) / 100) n = 0; /* Max or more = everything */
          if (n > 0 && D.state.safe != null && n > D.state.safe) {
            setText(el, '[data-e]', 'Your vault holds ' + fmt(D.state.safe) + ' KAS.');
            return;
          }
          if (D.state.sheetsOnPhone) {
            /* the phone keeps the sheets sealed: it signs, no typing */
            takeOutSlide(el, ok, this, n, to);
            return;
          }
          go('takeoutwords', { kas: n, to: to });
        };
      },
    },
    takeoutwords: {
      money: true,
      scene: function () {
        return { mode: 'locked', heat: ui.drag };
      },
      html: function (p) {
        if (D.isLocal())
          return (
            topbar() +
            '<h1 class="head">Two of your seeds</h1><p class="sub">Type any two of the three seeds you wrote down, 24 words each. They are used once and not ' +
            'kept.</p>' +
            '<label class="field"><span>First seed</span><textarea data-w1 autocapitalize="none" autocomplete="off" autocorrect="off" ' +
            'spellcheck="false"></textarea></label>' +
            '<label class="field"><span>Second seed</span><textarea data-w2 autocapitalize="none" autocomplete="off" autocorrect="off" ' +
            'spellcheck="false"></textarea></label><div class="chips" data-wp hidden></div>' +
            '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Move it</button>'
          );
        return (
          topbar(tag()) +
          '<h1 class="head">Take money out</h1><label class="field"><span>Your 24-word vault seed</span><textarea data-w autocapitalize="none" spellcheck="false"></textarea></label><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>Move it</button><button class="ghost" data-fill>Use the sample seed</button>'
        );
      },
      after: function (el, p) {
        if (D.isLocal()) {
          var list = bip39List(),
            ok = el.querySelector('[data-ok]');
          wordPicker(el, '[data-w1],[data-w2]');
          function sheet(sel) {
            var ws = el.querySelector(sel).value.trim().toLowerCase().split(/\s+/).filter(Boolean);
            if (list.length !== 2048) return 'The word list is missing. Update the app.'; /* fail closed */
            if (ws.length !== 24) return 'Please enter all 24 words.';
            for (var i = 0; i < 24; i++)
              /* never echo a seed word */
              if (list.indexOf(ws[i]) < 0) return 'Word ' + (i + 1) +
                ' is not on the list.';
            return ws.join(' ');
          }
          ok.onclick = function () {
            var a = sheet('[data-w1]'),
              b = sheet('[data-w2]');
            if (a.indexOf(' ') < 0 || a.split(' ').length !== 24) {
              setText(el, '[data-e]', 'First seed: ' + a);
              return;
            }
            if (b.indexOf(' ') < 0 || b.split(' ').length !== 24) {
              setText(el, '[data-e]', 'Second seed: ' + b);
              return;
            }
            if (a === b) {
              setText(el, '[data-e]',
                'Both entries are the same seed. Please enter two different seeds.');
              return;
            }
            takeOutWordsSlide(el, ok, this, p, a, b);
          };
          return;
        }
        var w = el.querySelector('[data-w]');
        el.querySelector('[data-fill]').onclick = function () {
          w.value = WORDS.join(' ');
        };
        el.querySelector('[data-ok]').onclick = function () {
          if (w.value.trim().split(/\s+/).length !== 24) {
            setText(el, '[data-e]', 'Please enter all 24 words.');
            return;
          }
          armSlide(
            this,
            'Take ' + fmt(D.state.safe) + ' KAS out of your vault',
            'Back into your account.',
            false,
            'Slide to move it',
            'send',
            function () {
              D.takeOutOfSafe();
              go('home', {}, { root: true });
            }
          );
        };
      },
    },

    /* 9. People */
    people: {
      tab: 'people',
      money: true /* money: the scene is not dimmed; the lines are the picture */,
      scene: function () {
        return { mode: 'people' };
      } /* no drawn tree; the scene's own lines take the Celtic tree-of-life weave (scene.js peoplePt) */,
      /* Sample and live alike; Sample rows are demo.js D.state.helpers and D.state.watched */
      html: peopleLiveHtml,
      after: wirePeopleLive,
      live: livePeopleLive,
    },
    /* guardians and heirs (docs/PEOPLE_SCREENS.md): the helper's link code, heir sheet, the owner's watch code, a watched account */
    linkcode: {
      tab: 'people',
      html: function (p) {
        return (
          topbar() +
          '<div class="label">' + (p.guide ? 'Step 1 of 3 \u00b7 You show' : 'Your link code') +
          '</div><h1 class="head">' + (p.guide ? 'Show this to the person you protect.' :
            'Share this code with the account holder.') + '</h1>' +
          '<p class="line">They scan it on their phone and choose your role: guardian, heir, or both. It contains nothing secret.</p>' +
          '<div class="qrbox"><div class="qr big" data-qr></div></div><div class="sub center" data-ln></div>' +
          '<button class="ghost" data-copycode hidden>Copy the code</button><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-make hidden>Make my link code</button>' +
          (p.guide ? '<p class="sub center" data-nx hidden>Once they have added you, ' +
            'they show you ' +
            'their code. Tap below when they are ready.</p><button class="pill" data-next hidden>' +
            'Next: scan their code</button>' : '') +
          '<button class="ghost" data-hw hidden>Show my heir seed</button>'
        );
      },
      after: function (el) {
        var nx = el.querySelector('[data-next]');
        if (nx) nx.onclick = scanWatch;
        var mk = el.querySelector('[data-make]'),
          hw = el.querySelector('[data-hw]'),
          cc = el.querySelector('[data-copycode]');
        mk.onclick = function () {
          openMakeLink(el);
        };
        hw.onclick = function () {
          D.heirWords().then(function (r) {
            if (!r.ok) return setText(el, '[data-e]', r.error ||
              'The heir seed could not be shown.');
            ui.heirWords = r.heir_words || null;
            go('heirsheet', {});
          });
        };
        D.linkCode().then(function (r) {
          if (!r.ok) {
            setText(el, '[data-e]', 'This phone has no link code yet.');
            mk.hidden = false;
            openMakeLink(el);
            return;
          }
          if (!r.code || !KVQR.svg(r.code)) return setText(el, '[data-e]', 'The code could not be drawn. Try again.');
          setHtml(el, '[data-qr]', KVQR.svg(r.code));
          setText(el, '[data-ln]', r.name || '');
          if (nx) nx.hidden = el.querySelector('[data-nx]').hidden = false;
          hw.hidden = false;
          cc.hidden = false;
          cc.onclick = function () {
            copyCode(el, r.code);
          };
        });
      },
    },
    heirsheet: {
      tab: 'people',
      money: true,
      locked: false,
      html: function () {
        var w = ui.heirWords;
        return (
          topbar() +
          '<div class="label">Your heir seed</div><h1 class="head">Write these on paper.</h1>' +
          '<p class="line">These 24 words are your heir seed. If this phone is lost, they are the only way to claim an account you are heir to. Keep them ' +
          'somewhere safe, never a photo.</p>' +
          (w ? wordsHtml(w) :
            '<div class="err">These words were shown already. "Show my heir seed" under your ' +
              'link code shows them again.</div>') +
          '<div class="push"></div>' +
          (w ? '<button class="pill" data-ok>I wrote them down</button>' : '')
        );
      },
      after: function (el, p) {
        var b = el.querySelector('[data-ok]');
        if (b)
          b.onclick = function () {
            go('heircheck', { first: !!p.first }, { replace: true });
          };
      },
    },
    heircheck: {
      tab: 'people',
      money: true,
      html: function () {
        return (
          topbar() +
          '<div class="label" data-q></div><h1 class="head" data-w></h1><div class="sub">Tap the right word</div><div class="list pick" data-pick></div><div class="push"></div>'
        );
      },
      after: function (el, p) {
        var sh = ui.heirWords;
        if (!sh) return back();
        var ask = [3, 11, 20],
          at = 0;
        function next() {
          if (at === ask.length) {
            ui.heirWords = null; /* read back: the words are gone from this page */
            D.paperMark('h');
            return p.first ? go('linkcode', {}, { replace: true }) : back();
          }
          var n = ask[at],
            right = sh[n];
          setText(el, '[data-q]', 'Heir seed, check ' + (at + 1) + ' of 3');
          setText(el, '[data-w]', 'Word ' + (n + 1));
          el.querySelector('[data-pick]').innerHTML = [right, sh[(n + 7) % 24], sh[(n + 15) % 24]]
            .sort()
            .map(function (w) {
              return '<button>' + esc(w) + '</button>';
            })
            .join('');
          [].forEach.call(el.querySelectorAll('[data-pick] button'), function (b) {
            b.onclick = function () {
              if (b.textContent !== right) return setText(el, '.sub',
                'That word does not match. Please check your paper copy.');
              at++;
              next();
            };
          });
        }
        next();
      },
    },
    watchcode: {
      tab: 'people',
      html: function (p) {
        return (
          topbar() +
          '<div class="label">' + (p.guide ? 'Step 3 of 3 \u00b7 You show' : 'Your watch code') +
          '</div><h1 class="head">' + (p.guide ? 'Now let ' + esc(p.guide) + ' scan this.' :
            'Share this code with your guardians.') + '</h1>' +
          '<p class="line">' + (p.guide ? 'This code lets their phone see your account. ' +
            'It cannot move funds.' : 'Their phone then sees this account: what is in it, ' +
            'any waiting withdrawal and the heir clock. It cannot move money. ' +
            'Show it again after you change who helps you.') + '</p>' +
          '<label class="field"><span>Your name, as they will see it</span><input data-on maxlength="40"></label><button class="pill" data-show>Show the code</button>' +
          '<div class="qrbox" data-box hidden><div class="qr big" data-qr></div></div>' +
          '<button class="ghost" data-copycode hidden>Copy the code</button><div class="err" data-e></div>' +
          (p.guide ? '<button class="pill" data-done>Done</button>' : '')
        );
      },
      after: function (el, p) {
        var inp = el.querySelector('[data-on]'),
          code = '';
        if (p.guide) {
          ui.guide = null; /* the guided add ends here */
          el.querySelector('[data-done]').onclick = function () {
            go('people', {}, { root: true });
          };
        }
        /* the same text the picture holds, to send by message when their camera cannot scan */
        el.querySelector('[data-copycode]').onclick = function () {
          copyCode(el, code);
        };
        D.linkCode().then(function (r) {
          if (r.ok && r.name && !inp.value) inp.value = r.name;
          /* step 3 shows the code at once */
          if (p.guide && inp.value) el.querySelector('[data-show]').click();
        });
        el.querySelector('[data-show]').onclick = function () {
          var v = inp.value.trim();
          if (!v) return setText(el, '[data-e]', 'Type your name.');
          setText(el, '[data-e]', '');
          D.watchCode(v).then(function (r) {
            if (!r.ok) return setText(el, '[data-e]', r.error);
            if (!r.code || !KVQR.svg(r.code)) return setText(el, '[data-e]', 'The code could not be drawn. Try again.');
            code = r.code;
            setHtml(el, '[data-qr]', KVQR.svg(r.code));
            el.querySelector('[data-box]').hidden = false;
            el.querySelector('[data-copycode]').hidden = false;
          });
        };
      },
    },
    watched: {
      tab: 'people',
      html: function () {
        return (
          topbar() +
          '<div class="label" data-wn></div><h1 class="head" data-wk></h1><p class="line lines" data-wl></p><p class="sub role" data-wr></p>' +
          '<div class="err" data-e></div><div class="push"></div><div data-wb></div>' +
          '<div class="qrbox" data-wqbox hidden><div class="qr big" data-wq></div></div>' +
          '<button class="pill" data-wcopy hidden>Copy the code</button>' +
          '<button class="ghost" data-wcode>Show this account\'s code</button>' +
          '<button class="ghost" data-forget>Forget this account</button>'
        );
      },
      after: function (el, p) {
        function cur() {
          return watchedList().filter(function (w) {
            return w.id === p.id;
          })[0];
        }
        el.querySelector('[data-wb]').addEventListener('click', function (e) {
          var b = e.target.closest('[data-ws],[data-wd],[data-wc]');
          var w = cur();
          if (b && w) watchedMove(el, b, w, b.hasAttribute('data-ws') ? 'ws' : b.hasAttribute('data-wd') ? 'wd' : 'wc');
        });
        el.querySelector('[data-forget]').onclick = function () {
          var w = cur();
          confirmAction('Forget ' + watchedName(w), 'This phone stops watching it. Their rules do not change.').then(function (yes) {
            if (!yes) return;
            D.watchForget(p.id).then(function (r) {
              if (!r.ok) return setText(el, '[data-e]', r.error);
              go('people', {}, { root: true });
            });
          });
        };
        D.loadWatched();
        wireWatchedCode(el, p.id);
      },
      live: function (el, p) {
        var w = watchedList().filter(function (x) {
          return x.id === p.id;
        })[0];
        setText(el, '[data-wn]', watchedName(w));
        setText(el, '[data-wk]', w && w.st && w.st.kas != null ? fmt(w.st.kas) + ' KAS' : '');
        setText(el, '[data-wl]', w ? watchedLine(w) : 'Reading the account…');
        setText(el, '[data-wr]', w ? rolesWord(w.roles) : '');
        if (!el.querySelector('.slide')) setHtml(el, '[data-wb]', watchedButtons(w)); /* never redraw under a slide in progress */
      },
    },
    persondetail: {
      tab: 'people',
      html: function (p) {
        var verb = {
          Sponsored: 'Sponsor a newcomer',
          Guardian: 'Add a guardian',
          Allowance: 'Give an allowance',
          Heir: 'Name an heir',
          Payee: 'Add a trusted payee',
        }[p.kind];
        return (
          topbar(tag()) +
          '<h1 class="head">' +
          verb +
          '</h1><label class="field"><span>Their name</span><input data-n placeholder="Name"></label>' +
          (p.kind === 'Payee'
            ? '<label class="field"><span>Their address</span><input data-a placeholder="kaspa:…" autocapitalize="none" spellcheck="false"></label>'
            : '') +
          (p.kind === 'Heir'
            ? '<div class="dial"><div class="h"><span>Inactivity period before they can claim ' +
              'it</span><b data-l>6 months</b></div><input type="range" aria-label="Months of ' +
              'inactivity" data-r min="1" max="24" step="1" value="6"></div><div class="sub">' +
              'Inactivity means no payment from this account for that long. Every payment you make restarts it.</div>'
            : '') +
          (p.kind === 'Guardian' || p.kind === 'Heir' || p.kind === 'Payee'
            ? ''
            : '<div class="dial"><div class="h"><span>Daily limit</span><b data-l>50 KAS</b></div><input type="range" aria-label="Daily limit" data-r min="5" max="200" step="5" value="50"></div>') +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Make the link</button>'
        );
      },
      after: function (el, p) {
        var r = el.querySelector('[data-r]');
        if (r)
          r.oninput = function () {
            setText(el, '[data-l]', p.kind === 'Heir' ? r.value + (r.value === '1' ? ' month' : ' months') : r.value + ' KAS');
          };
        el.querySelector('[data-ok]').onclick = function () {
          var n = el.querySelector('[data-n]').value.trim();
          if (!n) {
            setText(el, '[data-e]', 'Enter their name.');
            return;
          }
          if (p.kind === 'Payee') {
            var addr = el.querySelector('[data-a]').value.trim();
            if (!isKaspaAddress(addr)) {
              setText(el, '[data-e]', 'That is not a Kaspa address.');
              return;
            }
            ui.people.push({ name: n, role: 'Trusted payee', note: 'Payments to ' + addr.slice(0, 14) + '… are paid immediately.', addr: addr });
            D.addHistory('Added ' + n + ' as a trusted payee', 'people');
            go('people', {}, { replace: true });
            return;
          }
          var note =
            p.kind === 'Sponsored'
              ? 'They spend from it. You hold the stop until they take it over.'
              : p.kind === 'Guardian'
                ? n + ' holds a copy of your emergency stop.'
                : p.kind === 'Heir'
                  ? 'After ' + r.value + ' months of inactivity, ' + n +
                    ' can claim what is in this account.'
                  : 'They spend up to ' + (r ? r.value : 50) + ' KAS a day. You hold the stop.';
          function link() {
            ui.people.push({ name: n, role: p.kind, note: note });
            if (p.kind === 'Heir' && !D.isLive()) {
              var ah = acct();
              ah.heir = true;
              ah.heirMonths = +r.value;
              refresh();
            }
            go('shareinvite', { name: n }, { replace: true });
          }
          if (p.kind === 'Heir' || p.kind === 'Guardian') {
            /* the heir and the guardians live in the coin; naming one is a change of the rules (the sheet writes the history line) */
            if (D.isLocal()) {
              if (p.kind === 'Heir') openHeirSheet();
              else openKeysFact('Guardians');
              return;
            } /* on this phone the heir is heir.set, guardians are part of the keys */
            openChangeRules(p.kind === 'Heir' ? 'Name ' + n + ' as your heir' : 'Add ' + n + ' as a guardian', note, link);
            return;
          }
          D.addHistory('Invited ' + n + ' (' + p.kind.toLowerCase() + ')', 'people');
          link();
        };
      },
    },
    shareinvite: {
      tab: 'people',
      html: function (p) {
        return (
          topbar(tag()) +
          '<h1 class="head">Send this to ' +
          esc(p.name) +
          '</h1><div class="addr">kaspavault.app/join/sample-' +
          encodeURIComponent(p.name.toLowerCase()) +
          '</div><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-share>Share the link</button><button class="ghost" data-go="invite">See what they see</button><button class="ghost" data-done>Done</button>'
        );
      },
      after: function (el, p) {
        el.querySelector('[data-share]').onclick = function () {
          var t = 'Join me on KVault: kaspavault.app/join/sample-' +
            encodeURIComponent(p.name.toLowerCase());
          shareText(el, '[data-e]', t);
        };
        el.querySelector('[data-done]').onclick = function () {
          go('people', {}, { replace: true });
        };
      },
    },

    /* 10. Invite link flow (the newcomer's side) */
    invite: {
      sample: true /* needs demo.js: go() sends Live Home */,
      html: function () {
        return (
          topbar(tag()) +
          '<div class="push"></div><h1 class="head">Anna set up a protected account for you</h1><p class="line">You spend from it. She holds the emergency stop until you take it over.</p>' +
          '<div class="sub" data-w></div><button class="pill" data-take disabled>Take over the stop</button><button class="ghost" data-skip>Skip to week two</button><button class="ghost" data-in>Open my account</button>'
        );
      },
      after: function (el) {
        function upd() {
          el.querySelector('[data-take]').disabled = !ui.weekOver;
          setText(el, '[data-w]', ui.weekOver ? 'Your first week is over. The stop can be yours.' : 'This opens after your first week.');
        }
        upd();
        el.querySelector('[data-skip]').onclick = function () {
          ui.weekOver = true;
          upd();
        };
        el.querySelector('[data-take]').onclick = function () {
          setText(el, '[data-w]', 'The stop is yours now. Anna is no longer needed.');
          this.disabled = true;
        };
        el.querySelector('[data-in]').onclick = function () {
          D.setSetting('welcomed', '1');
          go('home', {}, { root: true });
        };
      },
    },

    /* 11. Alerts */
    alerts: {
      html: function () {
        return topbar('<span class="label">Alerts</span>') + '<div class="list" data-l></div>';
      },
      after: function (el) {
        el.querySelector('[data-l]').addEventListener('click', function (e) {
          var b = e.target.closest('[data-to]');
          if (b && b.dataset.to) go(b.dataset.to, {});
        });
      },
      live: function (el) {
        setHtml(el, '[data-l]', logHtml(200, true) || '<div class="sub">Nothing yet.</div>');
        alertsMark();
      },
    },

    /* coins: money sits in coins; many small ones cost more to spend (Kaspa storage mass), so combine them when it is quiet */
    coins: {
      tab: 'settings',
      money: true,
      dark: true /* a Settings page that moves nothing on Live yet */,
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Coins</h1><p class="line">Your money sits in coins, the way cash sits in notes. A payment uses some and leaves change. Many small coins cost more to spend, so combine them when things are quiet.</p><div class="list" data-l></div><div class="sub mt8" data-say></div><div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Combine into one</button>'
        );
      },
      live: function (el) {
        coinsPaint(el, ui.coins);
      },
      after: function (el) {
        var tag = (ui.coinsTag = {}); /* only the latest visit's answer is kept */
        ui.coins = null;
        coinsPaint(el, null);
        D.coinsInfo().then(function (c) {
          if (ui.coinsTag !== tag) return;
          ui.coins = c;
          if (el.isConnected) coinsPaint(el, c);
        });
        el.querySelector('[data-ok]').onclick = function () {
          armSlide(
            this,
            'Combine your Everyday coins into one',
            'A small fee, under a cent.',
            false,
            'Slide to combine',
            'send',
            function () {
              var r = D.combineCoins();
              if (r.ok) go('home', {}, { root: true });
              else {
                setText(el, '[data-e]', r.error);
                show(ui.cur.name, ui.cur.params);
              }
            }
          );
        };
      },
    },
    /* who can stop this account and who is heir; changes go through People, behind a warning */
    guardians: {
      tab: 'settings',
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Guardians</h1><p class="line">People who can stop this account for ' +
          'you, and your heir. A stop only ever sends your money to your own vault.</p>' +
          '<div class="list" data-gl></div><div class="push"></div>' +
          '<button class="ghost" data-change>Change guardians or heir</button>'
        );
      },
      after: function (el) {
        guardiansPaint(el);
        el.querySelector('[data-change]').onclick = guardiansWarn;
        if (!D.isLocal()) return;
        D.keysPublic().then(function (r) {
          if (!r || !r.ok)
            return setHtml(el, '[data-gl]', '<div class="sub">Your guardians could not be ' +
              'read. Try again in a moment.</div>');
          ui.pub = r.public || {};
          ui.stops = r.stops || null;
          guardiansPaint(el);
        });
      },
    },
    /* the watcher (all in one): this phone watches while on; a second one covers the rest */
    watcher: {
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Alerts on this phone</h1>' +
          '<p class="line">This phone watches your money ' +
          'on the Kaspa network and raises an alert when a withdrawal is asked for or ' +
          'something is not right. It watches while it is on. For the hours it is off, or if ' +
          'it is lost, a second phone watching is wise: it holds no keys, only your addresses.' +
          (D.isLocal() && (D.state.accounts || []).length > 1 ? ' It watches account 1.' : '') +
          '</p><div class="list">' +
          (D.isLocal()
            ? '<div class="item"><span class="t">Watch from this phone</span>' +
              '<span class="seg" data-guard><button data-v="1">On</button>' +
              '<button data-v="0">Off</button></span></div>'
            : toggleRow('watch_phone', 'Watch from this phone')) +
          row('seconddevice', 'Set up a second phone', 'A second phone or tablet watches too') +
          '</div>' +
          (D.isLocal()
            ? '<label class="field"><span>Second node, optional</span><input data-nodes autocapitalize="none" spellcheck="false" placeholder="wss://…" value="' +
              esc(D.getSetting('guard_nodes')) +
              '"></label><div class="sub">If you leave this empty, the app picks a second ' +
              'encrypted ' +
              'public node for you. Only encrypted (wss://) connections are allowed.</div>' +
              '<div class="sub" data-gsay></div>'
            : '')
        );
      },
      after: function (el) {
        var seg = el.querySelector('[data-guard]');
        if (!seg || !bridge || !bridge.guardOn) return;
        function paint(on) {
          [].forEach.call(seg.children, function (x) {
            x.classList.toggle('on', (x.dataset.v === '1') === on);
          });
          setText(
            el,
            '[data-gsay]',
            on
              ? 'Alerts are on. A small notification stays in your notification bar while it ' +
                'runs.'
              : 'Alerts are off. Nothing watches your money while the app is closed.'
          );
        }
        paint(!!bridge.guardRunning());
        [].forEach.call(seg.children, function (b) {
          b.onclick = function () {
            var on = b.dataset.v === '1';
            saveNodes(el).then(function (ok) {
              if (ok) guardSwitch(on, paint);
              else paint(!!bridge.guardRunning());
            });
          };
        });
      },
    },
    /* the panic PIN: six digits typed on the lock screen open the duress view (the purse only).
       Kept hashed with a salt (data.js); setting or removing it takes a fresh fingerprint. */
    panicpin: {
      tab: 'settings',
      html: function () {
        var set = D.panicIsSet();
        return (
          topbar(D.isLive() ? '' : tag()) +
          '<h1 class="head">Panic PIN</h1><p class="line">For a moment when someone makes you open ' +
          'the app. Type this PIN on the unlock screen instead of using your fingerprint: the app ' +
          'opens showing only your purse. Your account, your vault, your emergency stops, your ' +
          'people and your rules stay out of sight, and nothing there can move your account\u2019s ' +
          'funds. Keep only a small amount in the purse.</p>' +
          '<label class="field"><span data-pl>' + (set ? 'New PIN' : 'Choose a PIN') +
          ', 6 digits</span><input data-p inputmode="numeric" maxlength="6" autocomplete="off">' +
          '</label><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>' + (set ? 'Change it' : 'Set it') + '</button>' +
          (set ? '<button class="ghost" data-off>Remove it</button>' : '')
        );
      },
      after: panicPinAfter,
    },
    /* a statement: this month's movements and the total out, shareable as text */
    statements: {
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Statement</h1><div class="sub" data-hd></div>' +
          '<div class="list" data-l></div><div class="sub mt8" data-tot></div>' +
          '<div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-share>Share it</button>'
        );
      },
      live: function (el) {
        var d = statementData();
        setText(el, '[data-hd]', (d.month ? 'This month, ' : 'Recent activity, ') +
          (D.state.account || 'your account'));
        setHtml(el, '[data-l]', d.rows.map(statementRowHtml).join('') ||
          '<div class="sub">Nothing yet.</div>');
        setText(el, '[data-tot]', statementTotals(d));
      },
      after: function (el) {
        el.querySelector('[data-share]').onclick = function () {
          shareText(el, '[data-e]', statementText());
        };
      },
    },

    /* 12. Protection settings */
    /* every rule on one screen, edited first, confirmed once (phone) */
    changerules: {
      html: changeRulesHtml,
      after: changeRulesAfter,
    },
    protection: {
      html: function () {
        var s = acct(),
          g = ui.guards,
          n = noticeOf(s);
        function rule(key, title, right, who) {
          return (
            '<button class="item" data-explain="' +
            key +
            '"><span><span class="t">' +
            title +
            '</span>' +
            (who ? '<span class="d" ' + who + '></span>' : '') +
            '</span><span class="r">' +
            esc(right) +
            '</span></button>'
          );
        }
        var rules = g
          ? rule('limit', 'Daily limit', fmt(s.limit) + ' KAS') +
            rule('notice', 'Notice period', dur(n)) +
            rule('stops', 'Emergency stop', g.stops + (g.stops === 1 ? ' button' : ' buttons'),
              'data-whostops') +
            rule(
              'heir',
              'Heir',
              !g.heir ? 'none' :
                ui.acct === 'main' && heirLeft() != null ? dur(heirLeft()) + ' left' : 'named',
              'data-whoheir'
            ) +
            rule('safe', 'Vault', g.sheets === 1 ? '1 seed' : '2 of ' + g.sheets)
          : rule('limit', 'No covenant on this account', 'Protect it');
        return (
          topbar(tag()) +
          '<h1 class="head">Your rules</h1><p class="line">The rules on this account. Tap one to change it. The Kaspa network enforces them.</p><div class="list">' +
          rules +
          '</div>' +
          (D.isLocal() && g ? '<button class="pill mt8" data-go="changerules">Change the rules</button>' : '') +
          '<div class="label mt20">Who hears about a pending withdrawal</div><div class="list mt0">' +
          toggleRow('alert_me', 'You') +
          hearRows() +
          '</div>' +
          (D.isLive()
            ? ''
            : '<div class="label mt20">More protection</div><div class="list mt0">' +
              toggleRow(
                'only_trusted',
                'Immediate payments to trusted payees only',
                'Payments to anyone else wait for the notice period, whatever the amount. Add payees under People.',
                true
              ) +
              toggleRow('watch_only', 'This phone only watches', 'It can see and stop, not send.', true) +
              toggleRow(
                'clip_check',
                'Check pasted addresses',
                'Shows the start and end of an address you paste, to compare with where it came from.'
              ) +
              '</div>')
        );
      },
      after: function (el, p) {
        if (p && p.focus)
          setTimeout(function () {
            if (ui.guards) openDial(p.focus);
          }, 350);
        if (ui.guards && ui.acct === 'main') wireRulesWho(el);
        var KEYS = ['alert_me', 'alert_sam', 'only_trusted', 'watch_only', 'clip_check'];
        function read() {
          var v = {};
          KEYS.forEach(function (k) {
            if (el.querySelector('[data-toggle="' + k + '"]')) v[k] = toggleState(el, k);
          });
          return v;
        }
        stageChanges(el, read, function (v) {
          return confirmAction('Save these settings', 'They apply on this phone now.').then(function (yes) {
            if (!yes) return false;
            KEYS.forEach(function (k) {
              if (v[k] !== undefined) D.setSetting(k, v[k]);
            });
            return true;
          });
        });
      },
    },

    /* Library: sections of words, each with its plain line and its technical line; the search line filters as you type */
    glossary: {
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Library</h1><input class="libq" id="libq" type="search" placeholder="Search" autocomplete="off" autocapitalize="off" aria-label="Search the library">' +
          '<div class="gloss lib"><p class="line" data-none hidden>Nothing matches. Try another word.</p>' +
          LIBRARY.map(function (s) {
            return (
              '<div class="label" data-sec>' +
              esc(s.title) +
              '</div>' +
              s.items
                .map(function (w) {
                  return '<div class="word"><b>' + esc(w.term) + '</b><span>' + esc(w.plain) + '</span><i>' + esc(w.tech) + '</i></div>';
                })
                .join('')
            );
          }).join('') +
          '</div>'
        );
      },
      after: function (el) {
        var q = el.querySelector('#libq');
        if (!q) return;
        q.oninput = function () {
          /* match term, plain or tech, case-insensitive; a section hides when none of its words match */
          var s = q.value.trim().toLowerCase(),
            any = false;
          [].forEach.call(el.querySelectorAll('[data-sec]'), function (h) {
            var n = 0,
              w = h.nextElementSibling;
            while (w && w.classList.contains('word')) {
              var hit = !s || w.textContent.toLowerCase().indexOf(s) >= 0;
              w.hidden = !hit;
              if (hit) n++;
              w = w.nextElementSibling;
            }
            h.hidden = !n;
            any = any || n > 0;
          });
          el.querySelector('[data-none]').hidden = any;
        };
      },
    },
    /* How it works: a walk-through, one idea per step; swipe or Next / Back; each step can open its full story */
    howitworks: {
      scene: function () {
        var m = STEP_SCENE[ui.step];
        if (!m) return {};
        if (typeof m === 'string') return { mode: m, dim: false };
        var o = { dim: false };
        for (var key in m) o[key] = m[key];
        return o;
      },
      html: function (p) {
        ui.step = Math.min(STEPS.length - 1, Math.max(0, +p.step || 0));
        D.setSetting('toured', '1'); /* the Home offer goes once the walk has been seen */
        return (
          topbar('<span class="label" data-count></span>') +
          '<div class="marks" data-marks>' +
          STEPS.map(function () {
            return '<i></i>';
          }).join('') +
          '</div>' +
          '<div class="step" data-step></div><div class="push"></div><div class="twobtn" data-nav></div>'
        );
      },
      after: function (el, p) {
        var box = el.querySelector('[data-step]'),
          nav = el.querySelector('[data-nav]'),
          marks = el.querySelector('[data-marks]');
        el.classList.add('walk');
        function draw(dir) {
          var s = STEPS[ui.step];
          box.innerHTML = walkStepHtml(s);
          applyScene(); /* this step's rule shape behind it */
          setText(el, '[data-count]', ui.step + 1 + ' of ' + STEPS.length);
          [].forEach.call(marks.children, function (m, i) {
            m.classList.toggle('on', i <= ui.step);
          });
          nav.innerHTML = walkNavHtml(p);
          var dn = nav.querySelector('[data-done]');
          if (dn)
            dn.onclick = function () {
              if (p.then) go(p.then, {}, { replace: true });
              else if (p.intro) {
                go('home', {}, { root: true });
                setTimeout(explainScreen, 700);
              } else back();
            };
          if (dir) walkSlide(box, dir);
        }
        function step(dir) {
          var n = ui.step + dir;
          if (n < 0 || n >= STEPS.length) return;
          ui.step = n;
          draw(dir);
        }
        nav.onclick = function (e) {
          if (e.target.closest('[data-next]')) step(1);
          else if (e.target.closest('[data-prev]')) step(-1);
        };
        box.onclick = function (e) {
          var b = e.target.closest('[data-story]');
          if (b) openStory(+b.dataset.story);
        };
        var sx = null,
          sy = 0;
        el.addEventListener('pointerdown', function (e) {
          sx = e.clientX;
          sy = e.clientY;
        });
        el.addEventListener('pointerup', function (e) {
          if (sx == null) return;
          var dx = e.clientX - sx,
            dy = e.clientY - sy;
          sx = null;
          if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy) * 1.5) step(dx < 0 ? 1 : -1);
        });
        el.addEventListener('pointercancel', function () {
          sx = null;
        });
        draw(0);
      },
    },

    /* 13. Settings */
    settings: {
      tab: 'settings',
      html: function () {
        var live = D.isLive();
        return (
          '<div class="top"><span class="label">Settings</span>' +
          (live ? '' : tag()) +
          menuBtn() +
          '</div>' +
          '<div class="label mt8">Money</div><div class="list mt0">' +
          row('protection', 'Your rules', 'Daily limit, notice period, alerts, trusted payees') +
          (live ? '' : row('open', 'Open an account')) +
          row('coins', 'Coins', 'Combine small coins') +
          row('statements', 'Statement') +
          '</div>' +
          '<div class="label mt20">Keys</div><div class="list mt0">' +
          (D.isLocal()
            ? (D.state.noKeys ? row('makekeys', 'Make my keys',
              'On this phone, with one fingerprint') : '') +
              row('keysmade', 'Covenant code', 'And its public file') +
              row('papers', 'Your papers', 'Check you have every paper: seeds, stop, heir') +
              row('showsheets', 'Show my seeds', 'Your three vault seeds, after a fingerprint') +
              row('resetkeys', 'Lost your vault seeds?',
                'Starts over with new keys. The old vault cannot be opened without two seeds.') +
              row('paperstop', 'Make a paper stop', 'An emergency stop button written on paper') +
              row('paperuse', 'Use a paper stop',
                'Type its 24 words on any phone to stop an account') +
              row('pursepay', 'Pay from the purse', 'To any address, at once') +
              row('panicpin', 'Panic PIN', 'Opens the app showing only your purse',
                D.panicIsSet() ? 'Set' : 'Not set') +
              row('burnsheets', 'Delete the vault seeds on this phone',
                'Only paper opens the vault after this')
            : '') +
          (D.isLocal()
            ? ''
            : row('words', 'Test your recovery seed',
              'Check you can still find and read your recovery seed').replace(
                'data-go="words"',
                'data-go="words" data-params=\'{"mode":"check"}\''
              ) + row('restore', 'Restore an account', 'From your 24-word recovery seed')) +
          row('guardians', 'Guardians', 'Who can stop this account, and your heir') +
          row('watcher', 'Alerts on this phone',
            D.isLocal() ? 'This phone watches your money' : 'This phone, and a second one') +
          (D.isLocal() ? '' : row('seconddevice', 'Second device')) +
          '</div>' +
          (D.isLocal()
            ? '<div class="sub" data-fpwarn>Adding a fingerprint to this phone clears your ' +
              'everyday keys; your vault seeds stay.</div>'
            : '') +
          '<div class="label mt20">Help</div><div class="list mt0">' +
          row('howitworks', 'How it works') +
          row('glossary', 'Glossary') +
          row('prefs', 'Preferences') +
          row('about', 'About') +
          '</div>'
        );
      },
    },
    prefs: {
      tab: 'settings',
      html: function () {
        var eur = D.getSetting('currency') === 'eur';
        return (
          topbar(tag()) +
          '<div class="list"><div class="item"><span class="t">Currency</span><span class="seg" data-cur><button data-v="usd"' +
          (eur ? '' : ' class="on"') +
          '>Dollars</button><button data-v="eur"' +
          (eur ? ' class="on"' : '') +
          '>Euros</button></span></div>' +
          '<div class="item"><span class="t">Lock when you leave the app</span><span class="seg" data-lock>' +
          [
            ['0', 'At once'],
            ['60', '1 min'],
            ['300', '5 min'],
            ['900', '15 min'],
          ]
            .map(function (o) {
              return (
                '<button data-v="' +
                o[0] +
                '"' +
                (String(parseInt(D.getSetting('lock_after'), 10) || 0) === o[0] ? ' class="on"' : '') +
                '>' +
                o[1] +
                '</button>'
              );
            })
            .join('') +
          '</span></div>' +
          '<div class="item"><span class="t">Ask for my fingerprint</span><span class="seg" data-signer>' +
          [
            ['0', 'Every payment'],
            ['300', '5 min'],
            ['900', '15 min'],
            ['3600', '1 hour'],
          ]
            .map(function (o) {
              var cur = D.getSetting('signer_window') === '' ? '300' : String(parseInt(D.getSetting('signer_window'), 10) || 0);
              return '<button data-v="' + o[0] + '"' + (cur === o[0] ? ' class="on"' : '') + '>' + o[1] + '</button>';
            })
            .join('') +
          '</span></div>' /* the signer window in Java reads signer_window; sheets still take a fresh fingerprint */ +
          toggleRow('balance_money', 'Show your balance in dollars', 'Tap the balance on Home to swap any time.', true) +
          toggleRow(
            'send_grace',
            'One-minute cancellation window',
            'An on-the-spot send waits a minute on this phone with a Stop. Off, it leaves at once and is final within a second.',
            true
          ) +
          toggleRow(
            'screenshots',
            'Allow screenshots',
            'Off keeps your balance and your words out of screenshots and screen recordings. On this phone, your words and money screens stay hidden either way.',
            true
          ) +
          (D.isLocal() ? '' : row('seconddevice', 'Second device')) +
          '</div>'
        );
      },
      after: function (el) {
        wireSeg(el, '[data-cur]');
        wireSeg(el, '[data-signer]');
        wireSeg(el, '[data-lock]');
        var keys = ['balance_money', 'send_grace', 'screenshots']; /* alert_push: read by nothing */
        function read() {
          var v = {
            currency: el.querySelector('[data-cur] .on').dataset.v,
            lock_after: el.querySelector('[data-lock] .on').dataset.v,
            signer_window: el.querySelector('[data-signer] .on').dataset.v,
          };
          keys.forEach(function (k) {
            v[k] = toggleState(el, k);
          });
          return v;
        }
        stageChanges(el, read, saveSettings);
      },
    },
    /* this phone makes and seals its own three keys after one fingerprint; the key bytes never
       reach this page. */
    /* Make my keys: two doors, the simple one first. */
    makekeys: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Make my keys</h1><p class="sub">Your account needs three keys: a card for everyday payments, a stop, and a purse.</p>' +
          '<p class="sub">' + esc(NODES_LINE) + '</p>' +
          '<div class="list mt8"><button class="item" data-phone><span><span class="t">Make everything on this phone</span><span class="d">One tap and one ' +
          'fingerprint, then three vault seeds to write down.</span></span></button></div>' +
          '<div class="err" data-e></div><div class="push"></div>'
        );
      },
      after: function (el) {
        var b = el.querySelector('[data-phone]');
        b.onclick = function () {
          b.disabled = true;
          setText(el, '[data-e]', '');
          D.makeKeys({ mode: 'phone', heir_x: '', auto_heir: true, hard_limit: false }).then(function (r) {
            /* no heir for now: Change the rules adds one later */
            b.disabled = false;
            /* the phone has an account */
            if (r.already_has_keys) return go('home', {}, { root: true });
            if (!r.ok) {
              setText(el, '[data-e]', r.error);
              return;
            }
            haptic();
            if (r.sheets && r.sheets.length) {
              seedMarks(false);
              D.paperMark('p', false);
              ui.seedsKept = r.seeds_kept !== false;
              ui.sheets = r.sheets;
              go('sheet', { n: 0 }, { replace: true });
            } /* the words live only in this page's memory until read back */ else keysDone();
          });
        };
      },
    },
    /* One safe sheet of 24 words, shown once; the engine gave them with the keys and the page never stores them. */
    sheet: {
      tab: 'settings',
      money: true,
      locked: false,
      html: function (p) {
        var sh = ui.sheets && ui.sheets[p.n];
        return (
          topbar() +
          '<div class="label">Vault seed ' +
          (p.n + 1) +
          ' of 3</div><h1 class="head">Write these on paper.</h1><p class="line">Any two of the three seeds open your vault. Keep them in three places, ' +
          'never a photo. Make sure nobody is looking over your shoulder.</p>' +
          (sh && ui.seedsKept === false ? '<p class="line oldline">' + esc(SEEDS_NOT_KEPT) +
            '</p>' : '') +
          (sh
            ? '<div class="words">' +
              sh
                .map(function (w, i) {
                  return '<div><i>' + (i + 1) + '</i>' + esc(w) + '</div>';
                })
                .join('') +
              '</div>'
            : '<div class="err">These words were shown already. If you did not write them down, start over under Settings, Keys.</div>') +
          '<div class="push"></div>' +
          (sh
            ? '<button class="pill" data-go="sheetcheck" data-params=\'' +
              JSON.stringify({ n: p.n, one: !!p.one }) + "'>I wrote them down</button>"
            : '')
        );
      },
    },
    sheetcheck: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<div class="label" data-q></div><h1 class="head" data-w></h1><div class="sub">Tap the right word</div><div class="list pick" data-pick></div><div class="push"></div>'
        );
      },
      after: function (el, p) {
        var sh = ui.sheets && ui.sheets[p.n];
        if (!sh) {
          back();
          return;
        }
        var ask = [2, 10, 19],
          at = 0;
        function next() {
          if (at === ask.length) {
            D.paperMark('s' + (p.n + 1));
            if (p.one) {
              ui.sheets = null;
              ui.stack.pop(); /* the seed's words screen too: straight back to Your papers */
              return back();
            }
            if (p.n < 2) return go('sheet', { n: p.n + 1 }, { replace: true });
            ui.sheets = null; /* all three read back: the words are gone from this page */
            return go('papers', { after: 'setup' }, { replace: true });
          }
          var n = ask[at],
            right = sh[n],
            opts = [right, sh[(n + 7) % 24], sh[(n + 15) % 24]].sort();
          setText(el, '[data-q]', 'Vault seed ' + (p.n + 1) + ', check ' + (at + 1) + ' of 3');
          setText(el, '[data-w]', 'Word ' + (n + 1));
          el.querySelector('[data-pick]').innerHTML = opts
            .map(function (w) {
              return '<button>' + esc(w) + '</button>';
            })
            .join('');
          [].forEach.call(el.querySelectorAll('[data-pick] button'), function (b) {
            b.onclick = function () {
              if (b.textContent === right) {
                at++;
                next();
              } else setText(el, '.sub', 'That word does not match. Please check your paper copy.');
            };
          });
        }
        next();
      },
    },
    /* Your papers: the backup checklist (Settings, Home's papers row, after setup, after
       Update now) */
    papers: {
      tab: 'settings',
      money: true,
      html: function (p) {
        return (
          topbar() +
          '<div class="label">Backup</div><h1 class="head">Your papers</h1>' +
          '<p class="sub" data-count></p><p class="line oldline" data-old hidden>' +
          esc(PAPERS_OLD) +
          '</p><div class="list mt8" data-rows></div><div class="err" data-e></div>' +
          '<div class="push"></div><button class="pill" data-done>' +
          (p.after === 'setup' ? 'Next' : 'Done') + '</button>'
        );
      },
      after: function (el, p) {
        wirePapers(el, p);
      },
    },
    /* Settings > Paper stop: what it is, then make it; step 'apply' once the words are read back */
    paperstop: {
      tab: 'settings',
      money: true,
      html: function (p) {
        var apply = p.step === 'apply';
        return (
          topbar() +
          '<div class="label">Paper stop</div><h1 class="head">' +
          (apply ? 'Your paper stop is ready.' : 'An emergency stop on paper.') +
          '</h1><p class="line">' +
          (apply ? PAPER_APPLY : PAPER_WHAT) +
          '</p>' +
          '<div class="err" data-e></div><div class="push"></div>' +
          (apply
            ? '<button class="pill" data-apply>Add my paper stop</button>'
            : '<button class="pill" data-make>Make my paper stop</button>') +
          '<div data-pbox></div>' +
          (apply ? '' : '<button class="ghost" data-go="paperuse">Use a paper stop</button>')
        );
      },
      after: function (el) {
        var ap = el.querySelector('[data-apply]'),
          mk = el.querySelector('[data-make]');
        if (ap)
          ap.onclick = function () {
            paperConfirm(el, ap);
          };
        if (ap && ui.paper) paperConfirm(el, ap); /* straight on from the read-back */
        if (mk)
          mk.onclick = function () {
            paperMake(el, mk);
          };
      },
    },
    /* the 24 paper stop words, shown once: the engine gave them and the page never stores them */
    papershow: {
      tab: 'settings',
      money: true,
      locked: false,
      html: function () {
        var w = ui.paper;
        return (
          topbar() +
          '<div class="label">Paper stop</div><h1 class="head">Your 24 stop words</h1><p class="line">' +
          PAPER_KEEP +
          '</p>' +
          (w ? wordsHtml(w) : PAPER_SHOWN) +
          '<div class="push"></div>' +
          (w ? '<button class="pill" data-wrote>I wrote them down</button>' : '')
        );
      },
      after: function (el) {
        var b = el.querySelector('[data-wrote]');
        if (b)
          b.onclick = function () {
            go('papercheck', {}, { replace: true });
          };
      },
    },
    papercheck: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<div class="label" data-q></div><h1 class="head" data-w></h1><div class="sub">Tap the right word</div>' +
          '<div class="list pick" data-pick></div><div class="push"></div>'
        );
      },
      after: function (el) {
        paperCheckRun(el);
      },
    },
    /* the watch code to print with the words; showing it records its short hash (paper_shown) */
    paperprint: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<div class="label">Print the code</div><h1 class="head">Your paper stop code</h1>' +
          "<p class=\"line\">Keep this code with your stop words. It shows your account's rules and balance and never moves money.</p>" +
          '<div class="qrbox" data-box hidden><div class="qr" data-qr></div></div><p class="addr" data-ids></p>' +
          '<div class="err" data-e></div>' +
          '<div class="push"></div><button class="pill" data-done>Done</button>'
        );
      },
      after: function (el, p) {
        el.querySelector('[data-done]').onclick = function () {
          if (p.from === 'papers') return back();
          go('home', {}, { root: true });
          if (p.share) shareNewCode();
        };
        D.paperSheet().then(function (r) {
          var code = String((r && r.ok && r.watch_code) || '');
          if (!code || code.length > 2953)
            return setText(el, '[data-e]', (r && r.error) || 'The code could not be made.');
          setHtml(el, '[data-qr]', KVQR.svg(code));
          el.querySelector('[data-box]').hidden = false;
          setText(el, '[data-ids]', paperSheetIds(code));
          D.paperShown(code);
        });
      },
    },
    /* Use a paper stop: no keys on the phone, no fingerprint (the words are the authority) */
    paperuse: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<div class="label">Use a paper stop</div><h1 class="head">Stop your account from any phone.</h1>' +
          '<p class="line" data-say>' +
          PAPER_FIND +
          '</p><p class="addr" data-ids></p><div data-find><label class="field"><span>The printed code</span>' +
          '<textarea data-code autocapitalize="none" spellcheck="false"></textarea></label>' +
          '<div class="twobtn"><button class="pill" data-scan>Scan the code</button>' +
          '<button class="pill" data-check>Find my account</button></div></div>' +
          '<div data-words hidden>' +
          paperInputs() +
          '<div class="chips" data-wp hidden></div></div><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-stop hidden>Stop my account</button>'
        );
      },
      after: function (el) {
        wirePaperUse(el);
      },
    },
    paperdone: {
      money: true,
      html: function (p) {
        return (
          '<div class="push"></div><h1 class="head">' +
          (p.sent ? PAPER_SENT : 'Your account is stopped.') +
          '</h1><p class="line">' +
          fmt(+p.kas || 0) +
          ' KAS is moving to your vault. Any two of your three vault seeds open it.</p>' +
          (p.ids ? '<p class="addr">' + esc(p.ids) + '</p>' : '') +
          '<div class="push"></div>' +
          '<button class="ghost" data-go="paperout">Take your funds out with two vault seeds</button>' +
          '<button class="pill" data-done>Done</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-done]').onclick = function () {
          go('home', {}, { root: true });
        };
      },
    },
    paperout: {
      tab: 'settings',
      money: true,
      html: paperOutHtml,
      after: function (el) {
        wirePaperOut(el);
      },
    },
    paperoutdone: {
      money: true,
      html: function (p) {
        return (
          '<div class="push"></div><h1 class="head">' +
          (p.sent ? OUT_SENT : 'Your funds are out of the vault.') +
          '</h1><p class="line">' +
          fmt(+p.kas || 0) +
          ' KAS left your vault. It goes to:</p>' +
          (p.to ? '<p class="addr">' + esc(p.to) + '</p>' : '') +
          '<div class="push"></div><button class="pill" data-done>Done</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-done]').onclick = function () {
          go('home', {}, { root: true });
        };
      },
    },
    /* The rule block code (the sha of the public block). The public file can be copied out. */
    keysmade: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Your covenant</h1><p class="sub">This code names the rules your money will live under.</p>' +
          '<div class="code" data-code>\u2026</div><div class="sub center mt8" data-purse></div><div class="err center" data-e></div><div class="push"></div>' +
          '<div class="twobtn"><button class="pill" data-copy>Copy the public file</button><button class="pill" data-go="receive">Put money in</button></div>'
        );
      },
      after: function (el) {
        var exp = '';
        D.keysPublic().then(function (r) {
          if (!r.ok) {
            setText(el, '[data-e]', r.error);
            return;
          }
          var code = String(r.public_sha256 || '');
          setText(el, '[data-code]', code ? code.match(/.{1,4}/g).join(' ') : 'No code yet.');
          if (r.funding_address) {
            ui.purse = r.funding_address;
            setText(el, '[data-purse]', 'The purse: ' + shortAddr(r.funding_address));
          }
          exp = JSON.stringify(r.export || {});
        });
        el.querySelector('[data-copy]').onclick = function () {
          if (!exp) return;
          if (bridge && bridge.copy) {
            bridge.copy(exp);
            haptic();
            setText(el, '[data-e]', 'Copied.');
            return;
          }
          (navigator.clipboard ? navigator.clipboard.writeText(exp) : Promise.reject()).then(
            function () {
              setText(el, '[data-e]', 'Copied.');
            },
            function () {
              setText(el, '[data-e]', 'Copying is not available here.');
            }
          );
        };
      },
    },
    /* Open the account: money moves from the purse into the covenant (engine `fund`, the funding key, a fingerprint). */
    fundacct: {
      tab: 'home',
      money: true,
      html: function (p) {
        var an = p && typeof p.account === 'number' ? p.account : 0; /* opening a further account (2..8) from the picker */
        var re = reopening(p),
          rw =
            re && D.state.rulesNext && D.state.rulesNext.words
              ? D.state.rulesNext.words
                  .map(function (t) {
                    return '<div>' + esc(String(t)) + '</div>';
                  })
                  .join('')
              : ''; /* the stored rules, one line each */
        var intro = fundIntro(an, re, rw);
        return (
          topbar() +
          '<h1 class="head">' +
          (re ? 'Open the account again' : an ? 'Open account ' + an : 'Open the account') +
          '</h1>' +
          intro +
          '<div class="sub" data-purse>Reading the purse\u2026</div><button class="rcv-addr" data-addr hidden></button><button class="ghost small" data-copy hidden>Copy the purse address</button>' +
          '<label class="field"><span>Amount, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" placeholder="10"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>' +
          (re ? 'Open the account again' : an ? 'Open account ' + an : 'Open the account') +
          '</button>'
        );
      },
      after: function (el, p) {
        var ok = el.querySelector('[data-ok]'),
          have = null,
          addr = el.querySelector('[data-addr]'),
          cp = el.querySelector('[data-copy]');
        var an = p && typeof p.account === 'number' ? p.account : 0; /* 0 = the current account */
        var re = reopening(p); /* no dials: the engine applies the stored rules and clears them */
        wireFold(el);
        el.querySelector('[data-max]').onclick = function () {
          if (have >= 1.05) el.querySelector('[data-amt]').value = plainNum(Math.floor((have - 1) *
            100) / 100);
        };
        ok.disabled = true; /* until the purse is read and holds enough (never silent on an empty purse) */
        readPurse(el, function (r) {
          have = r.kas;
          ui.purse = r.address;
          var enough = have >= 1.05;
          setText(
            el,
            '[data-purse]',
            enough
              ? 'Your purse holds ' + fmt(have) + ' KAS. What you do not move stays in it.'
              : 'Your purse holds ' + fmt(have) +
                ' KAS. Add funds first: send test KAS to this address, then return here.'
          );
          addr.hidden = cp.hidden = enough;
          addr.textContent = r.address;
          ok.disabled = !enough;
        });
        cp.onclick = function () {
          copyPurseAddr(el, addr);
        };
        ok.onclick = function () {
          var n = amountOf(el.querySelector('[data-amt]').value);
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          if (n <= 0) {
            setText(el, '[data-e]', 'Type an amount.');
            return;
          }
          if (have != null && n + 1 > have) {
            setText(el, '[data-e]', 'The purse holds ' + fmt(have) + ' KAS. Leave about 1 KAS for the network fee.');
            return;
          }
          ok.disabled = true;
          ok.textContent = 'Opening\u2026';
          setText(el, '[data-e]', '');
          fundOpen(el, ok, n, re, an);
        };
      },
    },
    /* The sealed sheets: shown any time after a fingerprint; or burned from the phone on purpose, so only paper opens the vault. */
    showsheets: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Your vault seeds</h1><p class="sub">Any two of the three open your vault. Write them on paper and keep them in three places; ' +
          'never a photo.</p><div data-sheets></div><div class="err" data-e></div><div class="push"></div>'
        );
      },
      after: function (el) {
        D.sheets().then(function (r) {
          if (!r.ok) {
            setText(el, '[data-e]', r.error);
            return;
          }
          el.querySelector('[data-sheets]').innerHTML = (r.sheets || [])
            .map(function (sh, k) {
              return (
                '<div class="label mt20">Vault seed ' +
                (k + 1) +
                '</div><div class="words">' +
                sh
                  .map(function (w, i) {
                    return '<div><i>' + (i + 1) + '</i>' + esc(w) + '</div>';
                  })
                  .join('') +
                '</div>'
              );
            })
            .join('');
        });
      },
    },
    burnsheets: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Delete the seeds on this phone</h1><p class="line">This phone keeps a sealed copy of your three vault seeds, so it can show them ' +
          'again and open the vault for you. Deleting the copy means only your paper seeds open the vault from then on. Nothing on this phone, and nobody ' +
          'who takes it, can.</p>' +
          '<p class="sub">It cannot be undone. If the paper is lost, the money in the vault is lost with it.</p>' +
          '<div class="dial"><div class="h"><span>I have all three seeds on paper and read them back</span></div><span class="seg" data-sure><button ' +
          'data-v="0" class="on">No</button><button data-v="1">Yes</button></span></div>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok disabled>Delete them</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]');
        wireSeg(el, '[data-sure]', function (b) {
          ok.disabled = b.dataset.v !== '1';
        });
        ok.onclick = function () {
          armSlide(
            this,
            'Delete the vault seeds on this phone',
            'Only paper opens the vault after this.',
            true,
            'Slide to delete them',
            'stop',
            function () {
              D.burnSheets().then(function (r) {
                if (r.ok && !r.burn_not_done) {
                  haptic();
                  seedMarks(true);
                  setText(el, '[data-e]', 'Deleted. Only your paper seeds can open the vault now.');
                  ok.disabled = true;
                } else {
                  resetSlide(el);
                  setText(el, '[data-e]', r.error);
                }
              });
            }
          );
        };
      },
    },
    /* Pay from the purse to any address (engine purse.pay, the purse key) */
    pursepay: {
      tab: 'home',
      money: true,
      html: function () {
        return (
          topbar() +
          (ui.duress
            ? '<h1 class="head">Send</h1><p class="sub">It goes where you say, at once.</p>'
            : '<h1 class="head">Pay from the purse</h1><p class="sub">Money in the purse is ' +
              'outside the account and its rules. It goes where you say, at once.</p>') +
          '<div class="sub" data-purse>Reading the purse\u2026</div>' +
          '<label class="field"><span>To</span><input data-to autocapitalize="none" spellcheck="false" placeholder="kaspa:\u2026"></label>' +
          '<label class="field"><span>Amount, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" placeholder="All of it"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>' +
          (ui.duress ? 'Send' : 'Pay') + '</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]'),
          have = null,
          amt = el.querySelector('[data-amt]');
        el.querySelector('[data-max]').onclick = function () {
          if (have != null) amt.value = String(Math.floor(have * 100) / 100);
        };
        readPurse(el, function (r) {
          have = r.kas;
        });
        ok.onclick = function () {
          var to = el.querySelector('[data-to]').value.trim().toLowerCase(),
            n = amountOf(el.querySelector('[data-amt]').value);
          if (!isKaspaAddress(to)) {
            setText(el, '[data-e]', 'That is not a Kaspa address.');
            return;
          }
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          if (have != null && n > 0 && n >= Math.floor(have * 100) / 100) n = 0; /* Max or more = everything, the fee comes off the top */
          if (have != null && n > have) {
            setText(el, '[data-e]', 'The purse holds ' + fmt(have) + ' KAS.');
            return;
          }
          armToSlide(
            to,
            this,
            'Pay ' + (n > 0 ? fmt(n) + ' KAS' : 'everything in the purse'),
            'To ' + addrGroups(to) + '. At once, no notice period.',
            false,
            'Slide to pay',
            'send',
            function () {
              ok.disabled = true;
              D.pursePay(to, n).then(function (r) {
                ok.disabled = false;
                homeOrError(el, r);
              });
            }
          );
        };
      },
    },
    /* What a claim paid to this phone's heir address: outside every account and its rules; a plain
       payment out, signed
       with this phone's heir key after the slide and one fingerprint */
    heirpurse: {
      tab: 'people',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Received as heir</h1><p class="sub">Funds you claimed as an heir. They are outside any account and its rules, and go where you ' +
          'say at once.</p>' +
          '<div class="sub" data-purse>Reading\u2026</div>' +
          '<label class="field"><span>To</span><input data-to autocapitalize="none" spellcheck="false" placeholder="kaspa:\u2026"></label>' +
          '<label class="field"><span>Amount, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" ' +
          'placeholder="All of it"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Send</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]'),
          have = null,
          amt = el.querySelector('[data-amt]');
        el.querySelector('[data-max]').onclick = function () {
          if (have != null) amt.value = String(Math.floor(have * 100) / 100);
        };
        D.heirPurse().then(function (r) {
          if (!r.ok) return setText(el, '[data-purse]', r.error);
          have = r.kas;
          setText(el, '[data-purse]', 'You hold ' + fmt(r.kas) + ' KAS received as heir.');
        });
        ok.onclick = function () {
          var to = el.querySelector('[data-to]').value.trim().toLowerCase(),
            n = amountOf(amt.value);
          if (!isKaspaAddress(to)) return setText(el, '[data-e]', 'That is not a Kaspa address.');
          if (isNaN(n)) return setText(el, '[data-e]', AMOUNT_BAD);
          /* Max or more = everything, the fee comes off the top */
          if (have != null && n > 0 && n >= Math.floor(have * 100) / 100) n = 0;
          if (have != null && n > have) return setText(el, '[data-e]', 'You hold ' + fmt(have) +
            ' KAS.');
          var title = 'Send ' + (n > 0 ? fmt(n) + ' KAS' : 'all of it'),
            line = 'To ' + addrGroups(to) + '. At once; this cannot be undone.';
          armToSlide(to, this, title, line, false, 'Slide to send', 'send', function () {
            ok.disabled = true;
            D.heirPay(to, n).then(function (r) {
              ok.disabled = false;
              if (!r.ok) {
                resetSlide(el);
                return setText(el, '[data-e]', r.error ||
                  'The payment failed. No funds were sent.');
              }
              haptic();
              D.addHistory('Sent ' + (n > 0 ? fmt(n) + ' KAS' : 'all funds') + ' received as heir.',
                'people');
              go('people', {}, { root: true });
            });
          });
        };
      },
    },
    /* Fresh keys on this phone (engine keys.reset): only once the purse and the account are empty; the old folder is kept aside. */
    resetkeys: {
      tab: 'settings',
      money: true,
      html: function (p) {
        if (!p.sure)
          return (
            topbar() +
            '<div class="label">Warning</div><h1 class="head">Lost your vault seeds?</h1>' +
            '<p class="line">If no two of your vault seeds exist anywhere, the money in your vault can never be opened ' +
            'again. Replacing your keys does not bring it back: it starts over with a new purse, ' +
            'a new account and a new vault.</p>' +
            '<p class="line">If you still have two seeds, you do not need this. Close it.</p>' +
            '<div class="push"></div><button class="pill" data-back>Close</button>' +
            '<button class="ghost small" data-go="resetkeys" data-params="' + esc(JSON.stringify({ sure: true })) +
            '">I understand, continue</button>'
          );
        return (
          topbar() +
          '<div class="label">Second warning: this cannot be undone</div>' +
          '<h1 class="head">Replace your keys</h1><p class="line">New keys mean a new purse, a new account and a new vault. The old ones are set aside, not ' +
          'deleted. This works only once the purse and the account are empty: pay the purse out first.</p>' +
          '<div class="dial"><div class="h"><span>The vault’s seeds are lost: leave the vault behind</span></div><span class="seg" data-leave><button ' +
          'data-v="0" class="on">No</button><button data-v="1">Yes</button></span></div>' +
          '<div class="sub">Yes means the money in the old vault stays where it is, for good. Only choose it when no two seeds exist anywhere.</div>' +
          '<label class="field"><span>Type REPLACE to go on</span><input data-typed maxlength="12" autocomplete="off"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Start fresh</button>'
        );
      },
      after: function (el, p) {
        var ok = el.querySelector('[data-ok]');
        if (!p.sure || !ok) return;
        wireSeg(el, '[data-leave]');
        ok.onclick = function () {
          var leave = el.querySelector('[data-leave] .on').dataset.v === '1';
          if (String(el.querySelector('[data-typed]').value).trim().toUpperCase() !== 'REPLACE')
            return setText(el, '[data-e]', 'Type REPLACE first.');
          confirmAction(
            'Replace your keys',
            leave ? 'The old vault and its money are left behind, for good.' : 'The old keys are set aside.',
            true
          ).then(function (yes) {
            if (!yes) return;
            ok.disabled = true;
            D.resetKeys(leave).then(function (r) {
              ok.disabled = false;
              resetKeysDone(el, r);
            });
          });
        };
      },
    },
    seconddevice: {
      sample: true /* needs demo.js: go() sends Live Home */,
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Second device</h1><p class="line">A second phone or tablet can watch too. It watches your addresses and alerts you to every ' +
          'withdrawal. It holds no keys and cannot move money. Linking one is not part of this app.</p><div class="list">' +
          toggleRow(
            'alert_second',
            'Send alerts to it too',
            'A withdrawal or a stop reaches the second device as well, so a lost phone does not mean a missed alert.',
            true
          ) +
          '</div>'
        );
      },
    },
    about: {
      tab: 'settings',
      html: function () {
        return (
          topbar() +
          '<h1 class="head">KVault</h1><p class="line">Version 1.0</p><p class="sub">Early release. In Sample mode, balances are simulated and nothing ' +
          'leaves this phone.</p>' +
          '<div class="list mt8">' +
          row('privacy', 'Privacy', 'What leaves this phone, and who sees it') +
          '</div><div class="push"></div>' +
          (D.isLive() ? '' : '<button class="ghost" data-reset>Start over</button>')
        );
      },
      after: function (el) {
        var r = el.querySelector('[data-reset]');
        if (r)
          r.onclick = function () {
            D.resetSample();
            D.setSetting('welcomed', '');
            ui.acct = 'main';
            go('welcome', {}, { root: true });
          };
      },
    },
    /* Settings > About > Privacy: the essentials of the privacy policy (docs/store/PRIVACY.md) */
    privacy: {
      tab: 'settings',
      html: function () {
        return (
          topbar() +
          '<div class="label">Privacy</div><h1 class="head">We never receive your information.</h1>' +
          PRIVACY.map(function (t) {
            return '<p class="line">' + esc(t) + '</p>';
          }).join('') +
          (bridge && bridge.openUrl
            ? '<div class="push"></div>' +
              '<button class="ghost" data-web>Read the policy on the web</button>'
            : '')
        );
      },
      after: function (el) {
        var b = el.querySelector('[data-web]');
        if (!b) return;
        b.onclick = function () {
          /* the phone opens only this address, in the phone's browser, never inside the app */
          if (!bridge.openUrl(PRIVACY_URL)) refuse('The policy page could not be opened.');
        };
      },
    },
  };

  var PRIVACY_URL = 'https://sites.google.com/view/kaspavault-privacy';
  /* ---- start ---- */
  D.subscribe(function () {
    refresh();
    applyScene();
  });
  KVScene.init(document.getElementById('scene'));
  D.boot();
  go(D.getSetting('welcomed') === '1' || D.isLive() ? 'lock' : D.demo ? 'home' : 'welcome', {}, { root: true });
  if (document.fonts && document.fonts.ready)
    document.fonts.ready.then(function () {
      [].forEach.call(document.querySelectorAll('.big,.clock'), fit);
    });
  window.KVApp = { go: go, back: back, screens: SCR, ui: ui };
})();
