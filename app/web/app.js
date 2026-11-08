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
      zero: 'The limit refreshes right away, so it is no limit at all.',
    },
    heir: {
      label: 'Heir can claim after a silence of',
      presets: [2160, 4320, 8760, 17520],
      none: 'No silence',
      zero: 'Your heir could take the money today.',
    },
  };
  var UNIT = { minutes: 1 / 60, hours: 1, days: 24 };
  var HEIR_WARN = {
    amber: 0.75,
    red: 0.9,
  }; /* share of the heir silence run before the clock warns; Live takes red from the engine (heir_warn_share) */
  /* the KAS chart (price, Fibonacci levels and moving averages): sizes in SVG units (one unit = one CSS pixel), colours live in style.css */
  var CHART = {
    h: 190,
    top: 10,
    bottom: 20,
    left: 0,
    right: 46 /* drawing height and the margins: price labels sit in the right margin, time labels in the bottom one */,
    grid: 4,
    times: 4 /* horizontal price lines and time labels along the bottom */,
    dot: 3 /* radius of the dot on the last point */,
    ma: [
      { key: 'chart_ma20', n: 20, cls: 'ma20', label: 'MA 20' },
      { key: 'chart_ma50', n: 50, cls: 'ma50', label: 'MA 50' },
    ] /* simple moving averages: setting key, window in points, class, chip text */,
    fib: {
      key: 'chart_fib',
      levels: [0, 23.6, 38.2, 50, 61.8, 78.6, 100],
    } /* Fibonacci retracement, in percent from the range high (0) to its low (100) */,
    ranges: [
      ['day', '1D'],
      ['week', '1W'],
      ['month', '1M'],
      ['quarter', '3M'],
      ['year', '1Y'],
      ['all', 'All'],
    ],
  };

  /* the People tree (a circle of woven tendrils, like a tree of life): sizes in SVG units, colours in style.css */
  var PEOPLE_TREE = {
    w: 340,
    h: 220,
    trunk: 0.5 /* drawing size; trunk = where the roots meet the branches, as a share of the height */,
    rootSpread: 0.85,
    branchSpread: 0.8 /* how wide the roots (guardians) and branches (heir, guard, payees) fan, as a share of the width */,
    weave: 18,
    weaveTurns: 2.5,
    curve: 0.55 /* sideways amplitude in px, sine turns along a tendril, how far the control points lean */,
    line: 1.6,
    thin: 0.8,
    glow: 3,
    dot: 5,
    font: 11 /* stroke widths, the glow blur, the owner dot, label size */,
  };

  /* The drawer-device tutorial, one step per page: making the keys on a device that never goes online */
  var OFFLINE_STEPS = [
    [
      'Pick the drawer device',
      'An old laptop or phone you can wipe and keep in a drawer. Reinstall it from scratch, then turn off wifi and Bluetooth for good. From now on it never connects. A used machine with old malware is not offline.',
    ],
    ['Put the vault program on it', 'Carry it over on a USB stick that holds nothing else.'],
    [
      'Make the keys there',
      'On the drawer device choose "Make my stop key and safe sheets". It can mix in dice rolls you type, for old hardware with doubtful randomness. It writes a small file holding only public halves, like an account number.',
    ],
    [
      'Write the three sheets',
      'Three sheets of 24 words, shown one at a time; read three words back from each. Keep them in three places, never with the device, never as a photo.',
    ],
    [
      'Carry the public half over',
      'As a code the drawer device shows, or on the stick. It holds only public halves and is safe to show. On this phone: "I have a drawer device", scan or paste it. This phone then makes its own card key and shows a rule block code.',
    ],
    [
      'Carry that code back once',
      'The drawer device works out its own rule block code. The two codes must match letter for letter. If they differ, stop and start over: something changed on the way. This one check catches a swapped vault address.',
    ],
    ['Lock the drawer', 'Put a password on the key file, power the device off, and store it like a fourth sheet.'],
    [
      'Test with small money',
      'Press the emergency stop from this phone: it shows a code, the drawer device signs, this phone scans the answer. Then take a little from the vault with sheets 1 and 2, 2 and 3, and 1 and 3.',
    ],
  ];
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
    people: [
      { name: 'Mum', role: 'Sponsored', note: 'She spends from it. You hold her emergency stop.' },
      { name: 'Sam', role: 'Guardian', note: 'Sam holds a copy of your emergency stop.' },
    ],
  };

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
      return '&#' + c.charCodeAt(0) + ';';
    });
  }
  function plainNum(n) {
    var s = String(n);
    return /e/i.test(s) ? Number(n).toFixed(8).replace(/0+$/, '').replace(/\.$/, '') : s;
  }
  /* exact, never rounded here: Live numbers are shown as the vault gave them */
  function shortAddr(a) {
    return a.length > 26 ? a.slice(0, 14) + '\u2026' + a.slice(-8) : a;
  }
  function fmt(n) {
    if (n == null) return '—';
    var p = plainNum(n).split('.');
    p[0] = p[0].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return p.join('.');
  }
  function money(kas) {
    var eur = D.getSetting('currency') === 'eur',
      v = kas * D.state.rate * (eur ? 0.92 : 1);
    return (eur ? '€' : '$') + v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }
  function under(kas) {
    return D.isLive() ? 'KAS' : money(kas);
  }
  function priceHead() {
    return D.isLive() ? 'KAS' : 'KAS ' + money(1).replace(/^about /, '');
  } /* the chart header */
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
      room = innerWidth - 56,
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
      setText(el, '[data-purse]', 'Your purse holds ' + fmt(r.kas) + ' KAS.');
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
      'KaspaVault keeps your money like a bank account with a daily limit and an emergency stop. Start a new account, bring one back with your recovery seed, or open an invitation from someone you know.',
    words:
      'These words are your paper copy. The recovery seed brings the app back on a new phone; the vault seed opens your vault and is never typed into this phone.',
    wordcheck: 'A quick check that your paper copy is right. Tap the word that matches the number we ask for.',
    makekeys:
      'Your account needs three keys. The simple way makes all of them on this phone with one fingerprint and gives you three sheets of words to write down. The advanced way brings in the public file from a drawer device that never goes online.',
    keysdrawer:
      'For a drawer device: paste or scan the public file it made and, if you want one, the heir\u2019s public key. One fingerprint seals this phone\u2019s own keys. The tutorial button walks through making the drawer device.',
    sheet:
      'One of your three safe sheets. Any two open your vault, so they must outlive this phone: paper, three places, never a photo. This phone keeps a copy, and adding a fingerprint leaves it in place.',
    sheetcheck: 'A quick check that your paper copy is right. Tap the word that matches the number asked for.',
    offlinehow: 'How a drawer device is made and used, one step per page.',
    showsheets: 'Your three vault sheets, kept sealed on this phone. Any two open the vault. Paper, three places, never a photo.',
    burnsheets: 'Burn the sealed copy and only your paper sheets open the vault from then on. It cannot be undone.',
    pursepay: 'The purse is outside the account and its rules. A payment from it goes at once, no notice period, no limit.',
    resetkeys:
      'New keys, new purse, new account, new vault. Only once the purse and the account are empty. The old keys are set aside, not deleted.',
    keysmade:
      'The rule block code names the rules your money lives under. Your offline device shows the same code. The purse is where money goes in before the account opens.',
    topup:
      'Money moves from the purse into your open account and comes under its rules. Leave about 1 KAS in the purse for the network fee.',
    fundacct: 'Money moves from the purse into your account and comes under its rules. Leave about 1 KAS in the purse for the network fee.',
    restore: 'Type the 24-word recovery seed from your paper to bring your account back onto this phone. Nothing else is needed.',
    home: 'Your balance, what can still leave today on the spot, and five bars for the five rules on this account: daily limit, notice period, emergency stop, heir and vault. Tap a bar to change that rule; a dim bar is off. The log shows what happened. Your vault and the emergency stop are at the bottom.',
    open: 'An Everyday account is for spending and receiving. A Protected account adds a covenant: a daily limit, a notice period and an emergency stop.',
    plans:
      'A covenant is the set of rules your money lives under. It sets how much can leave on the spot each day, how long large payments wait, and what happens in an emergency. The Kaspa network itself enforces it, so nobody can skip it, not even you from this phone. Pick the one that fits. You can change it later.',
    glossary:
      'Every term in crypto and Kaspa this app leans on, each with a plain line and a technical one. The search line filters as you type.',
    dials:
      'Your own settings for this account: the daily limit, how long large payments wait, how often the limit refreshes, and how long a silence before your heir can claim. Pick a preset, or choose Custom and type a number. Zero is allowed, and we say plainly what it costs.',
    movein: 'Moving money from Everyday into your new protected account. You can also do this later.',
    send: 'Payments under today’s limit go right away. Large ones become a pending withdrawal that waits for the notice period, so you can stop them.',
    sent: 'Your payment is on its way, or waiting as a pending withdrawal. A pending one can still be stopped until it lands.',
    receive: 'Share this address, or let someone scan the code, to get paid. Money that arrives goes into this account.',
    pending: 'A large payment waiting out its notice period. Decline it if you did not ask for it. "That was me" only quiets the alert: it still waits, and you can decline it from Alerts until it leaves.',
    stop: 'The emergency stop moves all your money, pending withdrawals included, into your vault at once. Nobody can pay out of the vault from a phone: not you, not a thief, not us. Only your vault seed, kept on paper somewhere else, takes it out again.',
    stopdone: 'Everything is in your vault now. It stays there until you open the vault with your vault seed.',
    safe: 'Your vault holds money the emergency stop moved out of reach. Taking it out needs your vault seed, which never lives on this phone.',
    takeout:
      'Type the 24 words of your vault seed from paper to move the money back into your account. Take your time; a wrong word simply does nothing.',
    people:
      'Everyone linked to this account: guardians who hold a copy of your emergency stop, your heir, trusted payees who are paid on the spot, and anyone you sponsor or give an allowance to.',
    addperson:
      'Choose how someone is linked to you. A guardian can stop your account for you; a sponsored person gets a protected account you watch over.',
    persondetail:
      'Give the person a name and, where it applies, a daily limit. You can remove the link from People; for a guardian or an heir that means updating your rules.',
    shareinvite: 'Send this link to the person. It opens their own KaspaVault with the account you set up for them.',
    invite:
      'Someone set up a protected account for you and holds its emergency stop for now. After your first week you can take the stop over.',
    alerts:
      'Everything that happened on your account: payments, pending withdrawals, stops and invitations. Tap one to see where it leads.',
    protection:
      'The rules on this account, one row each: tap a row to change that rule. Below them, who hears about a pending withdrawal, and the extra protections on this phone.',
    settings:
      'Money: your covenant, coins and statement. Keys: your sheets, the guard and fresh keys. Help: how it works, the library, preferences and about.',
    prefs:
      'Your currency, which alerts you get, and whether screenshots are allowed. A second device can hold your vault seed so this phone never has to.',
    seconddevice: 'A second phone or tablet can keep your vault seed, so this one never sees them. Linking one is not part of this app.',
    about: 'Which build this is, and whether it is Live on this phone or the Sample. Start over wipes the sample and begins again.',
    heirclock:
      'How long until your heir could claim this account. The clock runs while the account hears nothing from you; any payment, top-up or stop restarts it. The line turns amber as the time approaches and red when it is close.',
    howitworks:
      'Short steps, one idea each, on how and why your account keeps your money safe. Swipe or tap Next; each step links to its full story. The ? on any screen brings you back to the step that fits.',
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
    addperson: 13,
    persondetail: 13,
    shareinvite: 13,
    invite: 13,
    protection: 7,
    seconddevice: 12,
    about: 16,
    receive: 10,
    settings: 15,
    price: 17,
  };
  /* the How-it-works screen: [title, story] */
  var STORIES = [
    [
      'Your account has a daily limit',
      'Every day you can spend up to your limit on the spot, with a tap and your fingerprint. The limit refreshes once a day. Anything a thief could grab in a hurry is capped at that amount.',
    ],
    [
      'Large payments wait',
      'A payment above your daily limit does not leave at once. It becomes a pending withdrawal, waits out your notice period, and you get a message. If it is yours, let it go through; if it is not, stop it.',
    ],
    [
      'The emergency stop',
      'Slide the stop and all your money moves into your vault in one go, pending withdrawals included. Nobody can spend from the vault: not you, not a thief, not us. Your vault seed, kept on paper, opens it again.',
    ],
    [
      'Your two sets of words',
      'Your recovery seed brings the app and your account back on a new phone. Your vault seed opens your vault. Both live on paper, and the vault seed is never typed into this phone, so a thief with your phone gets neither.',
    ],
    [
      'Guardians and sponsoring',
      'A guardian is someone you trust who holds a copy of your emergency stop; if you lose your phone, they can stop everything for you. Sponsoring works the other way round: you set up a protected account for a newcomer and hold their stop until they are ready to take it over.',
    ],
    [
      'What a thief can and cannot do with your unlocked phone',
      'They can see your balance and spend up to today’s limit. They cannot take more without waiting out the notice period, during which you or a guardian stop everything. They cannot reach your vault, because its words are not on the phone.',
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
      'Your seed',
      'Your recovery seed is 24 words written on paper. They are the only way to bring this wallet back on a new phone. Your vault has its own seed, which stays off this phone.',
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
      'Your balance, what can still leave today on the spot, and five bars for the five rules on this account: limit, notice, stops, heir and vault. Tap a bar to change that rule. The log underneath shows what happened.',
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
      'One red button moves everything in the account, pending withdrawals included, into your vault. No phone can pay out of the vault. Only your vault seed, kept on paper somewhere else, opens it again.',
      'Why the vault is separate and offline: a thief with your phone cannot reach it, and neither can we.',
      [2, 3],
    ],
    [
      'Three stops and guardians',
      'Your emergency stop can exist as three copies: yours, one with a guardian you trust, one kept somewhere safe. Any one of them can stop the account. A guardian presses theirs for you when your phone is lost. Changing or removing them later means updating your rules.',
      'Why three: stopping must always be possible, and a stop can only ever move money into your vault, never out of it.',
      [4],
    ],
    [
      'People',
      'Everyone linked to this account: guardians who hold a copy of your stop, your heir, trusted payees who are paid on the spot, and anyone you sponsor.',
      'Each of them is a rule the network enforces, not a promise.',
      [4],
    ],
    [
      'The heir after silence',
      'You can name an heir. If the account hears nothing from you for the silence you set, your heir can take the money. Every payment you make, even a small one, starts the silence again. The emergency stop is stronger than the heir. Once the money is in your vault only the vault seed opens it, so a stop pressed in time keeps a false heir out too. Changing or removing them later means updating your rules.',
      'Why silence, and why it resets: your money should not be lost with you, and while you are here it stays yours.',
      [],
    ],
    [
      'Settings',
      'Money holds your covenant, your coins and your statement. Keys holds recovery, the guard and a second phone or tablet. Help holds this walk-through and what the words mean.',
      'Everything is a dial, and every dial is on a screen you can reach.',
      [],
    ],
    [
      'What we can and cannot promise',
      'The rules were tested against the attacks we could think of: stolen phones, rushed payments, a lost stop. Everything is open source, so anyone can read the rules and test them. Nobody can return money that has left, and whoever holds both your sets of words holds everything.',
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
  /* the screen each step shows, as a small live copy of the real screen in Sample data; the five basics, the promise and Why Kaspa show none */
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
  function peek(name) {
    /* a non-interactive copy of a screen, scaled by .peek in style.css; data hooks run so lists and numbers fill */
    var def = SCR[name];
    if (!def) return null;
    var wrap = document.createElement('div');
    wrap.className = 'peek';
    var el = document.createElement('section');
    el.className = 'ui' + (def.tab ? ' tabbed' : '');
    try {
      el.innerHTML = def.html({});
      glassify(el);
    } catch (e) {
      return null;
    }
    wrap.appendChild(el);
    try {
      if (def.after) def.after(el, {});
    } catch (e) {}
    try {
      if (def.live) def.live(el, {});
    } catch (e) {}
    el.setAttribute('inert', '');
    return wrap;
  }
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
      how: 'You hold one. People you trust, your guardians, can hold others, so someone can press it if you lose your phone.',
      why: 'Once it is pressed, nobody can spend that money from a phone: not you, not a thief, not us.',
      mine: function (g) {
        return 'You have ' + g.stops + (g.stops === 1 ? ' button.' : ' buttons.');
      },
    },
    heir: {
      title: 'Heir',
      what: 'Someone who can claim this account if you go silent for a long time.',
      how: 'Every payment you make restarts the clock. Your heir can only claim after the full silence has passed. An emergency stop puts the money in your vault, and nothing leaves a vault without its seed: not for a thief, not for your heir.',
      why: 'Your money is not lost if something happens to you.',
      mine: function (g) {
        return g.heir ? 'You have named an heir.' : 'You have not named one. Tap the Heir bar on Home to name someone.';
      },
    },
    safe: {
      title: 'Vault',
      what: 'Where your money goes when an emergency stop is pressed.',
      how: 'It opens only with your vault seed, kept on paper and never on this phone. With three seeds on paper, any two open it, so losing one is not a disaster.',
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
  function nameSheet(title, value, cb) {
    openSheet(
      '<div class="label">' +
        esc(title) +
        '</div><label class="field mt8"><span>Name</span><input data-nm value="' +
        esc(value) +
        '" maxlength="24"></label><button class="pill" data-save>Save</button><button class="ghost" data-no>Close</button>'
    );
    var inp = sheet.querySelector('[data-nm]');
    inp.focus();
    inp.select();
    sheet.querySelector('[data-save]').onclick = function () {
      var v = inp.value.trim();
      if (!v) return;
      ui.closeSheet();
      cb(v);
    };
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
    var items = [
      ['explain', 'Explain this screen'],
      ['alerts', 'Alerts'],
      ['protection', 'Your rules'],
      ['howitworks', 'How it works'],
      ['glossary', 'Library: crypto and Kaspa'],
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
  function dialBody(key, s, g) {
    var body = '';
    if (key === 'limit')
      body =
        '<div class="dial"><div class="h"><span>On the spot each day</span><b data-l>' +
        fmt(s.limit) +
        ' KAS</b></div><input type="range" aria-label="Daily limit" data-lr min="0" max="500" step="5" value="' +
        s.limit +
        '"><div class="custom"><input data-lnum inputmode="decimal" aria-label="Daily limit, any amount" placeholder="Or type any amount"></div></div><div class="sub">Anything above it is a large payment: it waits the notice period and can be pulled back. At 0, every payment is a large payment.</div>';
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
    else if (key === 'heir') {
      /* the named heir is the option here; nobody named yet means naming one first */
      var hp = ui.people.filter(function (q) {
        return q.role === 'Heir';
      });
      body = hp.length
        ? '<div class="dial"><div class="h"><span>Your heir</span></div><span class="seg" data-seg>' +
          hp
            .map(function (q, i) {
              return '<button data-v="' + i + '"' + (i === 0 ? ' class="on"' : '') + '>' + esc(q.name) + '</button>';
            })
            .join('') +
          '</span></div>' +
          '<div class="dial"><div class="h"><span>Silence before ' +
          esc(hp[0].name) +
          ' can take it</span><b data-l>' +
          (s.heir ? (s.heirMonths || 6) + ' months' : 'No heir') +
          '</b></div><input type="range" aria-label="Months of silence" data-hr min="0" max="24" step="1" value="' +
          (s.heir ? s.heirMonths || 6 : 0) +
          '"><div class="custom"><input data-hnum inputmode="numeric" aria-label="Months of silence, any number" placeholder="Or type the months"></div></div>' +
          '<div class="sub">0 means no heir. Every payment you make restarts the silence. After an emergency stop nothing can be taken from your vault, not by your heir either: only your vault seed opens it.</div><button class="ghost" data-heir-add>Name someone else</button>'
        : '<div class="line">You have not named an heir yet. Name one, and this bar sets how long a silence they wait.</div><div class="sub">An heir can take what is in this account if it hears nothing from you for the silence you set. Never from your vault: after an emergency stop only your vault seed opens it.</div><button class="pill" data-heir-add>Name an heir</button>';
    } else if (key === 'safe')
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
        '</span></div><div class="sub">Three seeds on paper, any two open the vault: losing one is not a disaster.</div>';
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
                : 'Set the silence before ' + heirName + ' can take it to ' + hv + (hv === 1 ? ' month' : ' months')
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
     The only change is updating the rules: the money moves under the new ones. Up to the daily limit goes on the spot,
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
      '<div class="dial"><div class="h"><span>Silence before your heir can take it</span><b data-l>' +
      monthsOf(m) +
      '</b></div><input type="range" aria-label="Months of silence" data-hr min="1" max="24" step="1" value="' +
      m +
      '"><div class="custom"><input data-hnum inputmode="numeric" aria-label="Months of silence, any number" placeholder="Or type the months"></div></div>' +
      '<div class="sub">Every payment you make restarts the silence.</div>'
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
      openSheet(reviewSheetHtml(ch.lines, r || {}));
      wireFold(sheet);
      var st = sheet.querySelector('[data-start]');
      if (st)
        st.onclick = function () {
          rulesSlide(st, r);
        };
      var nw = sheet.querySelector('[data-now]');
      if (nw)
        nw.onclick = function () {
          nowSlide(nw);
        };
    });
  }
  /* the instant route: one slide, then "Updating your rules..." runs stop, take-out and reopen (D.updateNow) */
  function nowSlide(btn) {
    armSlide(btn, 'Update now', 'Your money stays yours, on the same keys.', false, 'Slide to update', 'send', function () {
      ui.closeSheet();
      go('waiting', { job: 'rules', now: true }, { replace: true });
    });
  }
  /* the slide on the review sheet: 'move' pays the purse, then 'now' finishes at once and 'notice' waits on Home; 'open' finishes straight away */
  function rulesSlide(btn, r) {
    armSlide(btn, 'Update my rules', 'Your money stays yours, on the same keys.', false, 'Slide to update', 'send', function () {
      if (r.route === 'open') {
        ui.closeSheet();
        return go('waiting', { job: 'rules' }, { replace: true });
      }
      D.moveRules(r).then(function (x) {
        if (!x.ok) {
          setText(sheet, '[data-serr]', x.error || 'That did not go through. Nothing has moved.');
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
    stop: 'Your money is going to your safe.',
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
    armSlide(btn, 'Finish updating my rules', 'Your money stays yours, on the same keys.', false, 'Slide to finish', 'send', function () {
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
          ? '<div class="sub">Update now stops your account: your money goes to your safe, comes straight back to your purse and opens again under the new rules.</div>'
          : '') +
        '<div class="sub">The network holds your rules with your money, so your money passes through your purse and comes back under the new rules, on the same keys.</div>' +
          '<div class="sub">1 KAS always stays in your safe; you can take it out any time.</div>' +
          (r.fee_kas == null ? '' : '<div class="sub">The payment and the safe step cost ' + (+r.fee_kas).toFixed(4) + ' KAS.</div>')
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
        '</div><div class="line mt8">These are part of your keys. To change them, start fresh keys (Settings &gt; Keys).</div>' +
        '<button class="pill" data-fresh>Start fresh keys</button><button class="ghost" data-no>Close</button>'
    );
    sheet.querySelector('[data-fresh]').onclick = function () {
      ui.closeSheet();
      go('resetkeys', {});
    };
  }
  /* the heir on this phone's keys (engine heir.set, keyless): a kaspa: address; refused in the engine's words while any account is open or holds money */
  function openHeirSheet() {
    var has = !!D.state.heir;
    openSheet(
      '<div class="label">' +
        (has ? 'Change the heir' : 'Name an heir') +
        '</div><div class="line mt8">Your heir can take what is in an account after it hears nothing from you for the silence set on it. Never what is in your vault.</div>' +
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
        ' KAS) moves on the spot; the rest waits the notice period (' +
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
        ' has no covenant: payments leave at once, with no limit, no notice period and no emergency stop.</div><div class="sub">Put it under one and the five bars come alive: 100 KAS a day on the spot, 24 h notice above it. Change any rule afterwards from its bar.</div>' +
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
            { text: fmt(FRESH_RULES.limit) + ' KAS a day on the spot, ' + dur(FRESH_RULES.notice * 3600) + ' notice above it.' },
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
          tech: "A plain pay-to-public-key address derived from the recovery seed. Any coin at it is spent by one signature from this phone's key, with no script rules.",
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
            'Where your money goes after a stop. Nobody can spend from it with a phone; it takes two vault seeds and the offline signer.',
          tech: "The app's name for the vault address: a 2-of-3 multisig script whose hash the covenant commits to at creation.",
        },
        {
          term: 'Vault seed',
          plain:
            'The 24 words that open your vault. Three sheets are made and any two open it. They live on paper and are never typed into this phone.',
          tech: 'Three BIP-39 mnemonics, one per vault key. The covenant stores only the hash of the 2-of-3 script, so the words are needed only to spend from the vault.',
        },
        {
          term: 'Recovery seed',
          plain: 'The words that bring your account back on a new phone. They do not open the vault.',
          tech: "A BIP-39 mnemonic for the hot keys: the Everyday address, the card key and this phone's stop key. The covenant coin is found again on the network by its covenant id.",
        },
        {
          term: 'Guardian',
          plain:
            'Someone you trust who holds a copy of your emergency stop, in case you lose your phone. They can stop or decline, never spend.',
          tech: 'A holder of one of the up to three stop keys set at creation. Their key can sign only the cancel and decline entries.',
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
            'A PIN you type on the unlock screen when someone makes you open the app. The app opens quiet, showing only your Everyday money.',
          tech: "A second PIN kept in the phone's secure store. Matching it sets quiet mode before Home renders; the protected account and the vault are neither shown nor spendable in that state.",
        },
        {
          term: 'Quiet mode',
          plain:
            'The app as the panic PIN shows it: Everyday only, the protected account and the vault out of sight, nothing else to find.',
          tech: 'A display state only. It hides the protected account and the vault entry; no rule on the network changes, so the daily limit and the stop still hold.',
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
            "Seed words are 12 or 24 ordinary words that encode a wallet's master secret. Anyone with the words can rebuild every key, so they are kept on paper only.",
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
            'Real-money addresses start with kaspa:. Test-network addresses start with kaspatest: and hold coins with no value. The wallet refuses to mix the two.',
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
          term: 'The stop key',
          plain:
            'The stop key moves everything in the account into your vault, even while a withdrawal is pending. It can only send money there, nowhere else, so it is safe to share.',
          tech: 'Cancel requires a stop key signature and an output paying the whole coin, less at most maxFee, to the fixed cold script hash. Up to three stop keys are set at creation.',
        },
        {
          term: 'Decline',
          plain:
            'Decline sends a waiting withdrawal back to locked without moving any money. Use it when the request is wrong but there is no need to empty the account.',
          tech: 'Decline needs a stop key signature; it clears the pending destination and amount, leaves window counters unchanged and renews the coin under the same covenant.',
        },
        {
          term: 'Heir and the silence clock',
          plain:
            'An heir can take the money if the account sits untouched for a long time, six months by default. Any payment or action restarts the clock.',
          tech: 'Inherit allows the heir key after inheritDelay in DAA score since the coin was last renewed. With autoHeir set, anyone may send the coin to the heir script once the silence has passed.',
        },
        {
          term: '2 of 3',
          plain: 'The vault has three seeds on paper, and any two open it. Lose one and nothing is lost; a thief with one has nothing.',
          tech: 'The vault address is a 2-of-3 multisig script; the covenant commits to its hash. Spending from it needs two Schnorr signatures from the three vault keys.',
        },
        {
          term: 'Max fee and the minimum left behind',
          plain:
            'Two guard rails: no action may burn more than a small fixed fee, and every coin the rules create must be big enough to be worth spending later.',
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
            "A hot key lives on a device that talks to the internet; a cold key never does. Your phone's key is hot, the vault is cold. Cold is slower and much harder to steal.",
          tech: 'A hot key signs on a networked device and is exposed to its software. A cold key is generated and used on an offline device; only signed transactions cross over.',
        },
        {
          term: 'Air gap',
          plain:
            'An air gap means no cable, wifi or Bluetooth between the signing device and the network. Data moves only by QR code or typed text that you can see.',
          tech: 'The online wallet builds an unsigned transaction, carries it to the offline device as hex or QR, and brings back the signature. The private key never leaves the offline side.',
        },
        {
          term: 'Offline signer',
          plain:
            'The offline signer is the program on the cold device that reads a request, shows you exactly what it will do, and signs it only if you agree.',
          tech: 'It reconstructs the transaction from the carried bytes, displays outputs and amounts, signs with the cold key and returns the signature for submission by the online wallet.',
        },
        {
          term: 'Secure chip / Keystore',
          plain:
            "The phone's secure chip seals your spending key and stop key behind your fingerprint. Apps, backups and even a copy of the phone's storage cannot read them.",
          tech: 'Android Keystore (StrongBox where present) stores keys non-exportable. The chip does ECDSA only, not Schnorr, so it seals the key and the app signs in memory while unlocked.',
        },
        {
          term: 'Fingerprint vs PIN',
          plain:
            'Fingerprint is fast and cannot be read over your shoulder; a PIN works when the sensor fails. Both only unlock the chip; neither is the key itself.',
          tech: 'Biometric or PIN authentication gates key use for a short validity window. A failed biometric falls back to the device credential; both are checked by the secure hardware.',
        },
        {
          term: 'Pairing and certificate pin',
          plain:
            'When the phone is a remote control for the laptop, they are paired once with an 8-digit code and check words. After that the phone only trusts that laptop.',
          tech: "Pairing exchanges a token over TLS; the phone stores the laptop's certificate fingerprint and refuses any other certificate, defeating a fake laptop on the same wifi.",
        },
        {
          term: 'Check words',
          plain:
            'Check words are a few words both devices show after pairing. If they match, you are connected to your own device and not an impostor.',
          tech: "Check words are a short word encoding of a hash over both sides' ephemeral keys and certificate, compared out of band by eye so a man in the middle shows a mismatch.",
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
            'Paper holds the vault seed. It cannot be hacked, but it can burn or be read, so keep the sheets in separate places and never photograph them.',
          tech: 'A paper backup records BIP-39 words that regenerate a key. For the 2-of-3 vault, three sheets are stored apart; any two suffice, so one lost or stolen sheet is harmless.',
        },
      ],
    },
    {
      title: 'Safety habits',
      items: [
        {
          term: 'Never type seed words into a phone',
          plain:
            'The vault seed goes on paper and into an offline device only. A phone that talks to the internet is where thieves look first.',
          tech: 'Any networked device may leak keyboard input through malware, clipboard or screen capture. The vault key must only ever be entered on the air-gapped signer.',
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
          term: 'Two guards',
          plain:
            'Two guards watch the network from two different machines. If one is silenced or lied to, the other still raises the alert and the notice period still works.',
          tech: "Two independent guards poll separate nodes for the covenant coin's state. A pending request seen by either triggers alerts, so one compromised node cannot hide a withdrawal.",
        },
        {
          term: 'What a stop costs you',
          plain:
            "A stop moves everything to your vault. Getting it back out needs two vault seeds and the offline signer, so expect a day's work. Pressing stop is never wrong.",
          tech: 'Cancel pays the full coin, minus at most maxFee, to the cold script. Re-funding a vault needs a 2-of-3 signing round and a new topup; nothing is lost but time.',
        },
        {
          term: 'What a thief can and cannot do',
          plain:
            'With your unlocked phone a thief can spend up to the daily limit, once. They cannot empty the account, change the rules or touch your vault.',
          tech: 'The covenant enforces the limit and notice period on every spend regardless of who signs. Bigger withdrawals are visible for the notice period and can be stopped or declined.',
        },
        {
          term: 'If the laptop is lost',
          plain:
            'Press stop from your phone or ask a guardian to. The money goes to your vault. Then set up a fresh account from the vault seeds.',
          tech: 'Any stop key holder can cancel the coin to the cold script. The lost device holds only hot keys bounded by the limit; the pairing token can be revoked from the phone.',
        },
        {
          term: 'If a vault seed is lost',
          plain:
            'One lost vault seed is fine: any two of three still open the vault. Move the money to a new vault soon, so a second loss cannot lock you out.',
          tech: "A 2-of-3 script tolerates one missing key. Spend the cold coins to a new 2-of-3 script with three fresh keys, then set that script's hash in a new vault.",
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
    if (!SCR[name].tab || name === 'home') return; /* Home has its own; inner screens have no menu */
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
      ['[data-limit]', 'Today', 'What can still leave today on the spot, and when the limit refreshes.'],
      ['.pips', 'Your rules', 'The five rules on this money. Tap a bar to change that one rule.'],
      ['[data-log]', 'Activity', 'The last things that happened. Anything still leaving has its own Stop.'],
      ['[data-row] .s', 'Emergency stop', 'Moves everything into your vault at once.'],
      ['[data-row] button:first-child', 'Vault', 'Where stopped money waits. Only your vault seed opens it.'],
      ['[data-menu]', 'Menu', 'Alerts, your covenant, the explanations and the settings.'],
    ],
    send: [
      ['[data-to]', 'To', 'An address, a name from your payees, or scan a code.'],
      ['[data-amt]', 'Amount', 'In KAS. Up to your daily limit it goes at once; above it, it waits the notice period and can be stopped.'],
      [
        '[data-ok]',
        'Send',
        'Your fingerprint or PIN, then a slide. Up to your limit it is final within a second, unless you turned on the minute to change your mind.',
      ],
    ],
    receive: [
      ['.qrbox', 'Your code', 'Anyone can scan this to pay you.'],
      ['[data-addr]', 'Your address', 'Tap to see all of it. Send only Kaspa to it.'],
      ['[data-copy]', 'Copy', 'Puts the address on the clipboard.'],
      ['[data-share]', 'Share', 'Sends the address to someone.'],
    ],
    people: [
      [
        '[data-l]',
        'People',
        'Guardians hold a copy of your stop. An heir can take the money after a long silence. Trusted payees are paid on the spot.',
      ],
      ['[data-go="addperson"]', 'Add someone', 'A guardian, an heir, a trusted payee, or someone you sponsor.'],
    ],
    price: [['.chart', 'KAS price', 'One KAS in dollars over a day, a week or a month.']],
    settings: [
      ['.list', 'Money', 'Your rules, your coins, your statement.'],
      ['.list:nth-of-type(2)', 'Keys', 'Recovery, the guard, a second device.'],
      ['.list:nth-of-type(3)', 'Help', 'How it works, and what the words mean.'],
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
    if (over) for (var k in over) o[k] = over[k];
    if (over && over.mode && over.mode !== 'stopped' && over.k === undefined)
      o.k = 0; /* a stopped account collapses the ribbon to a point; a shape needs it whole */
    o.dim = !(def && def.money);
    if (o.charge === undefined) o.charge = ui.drag; /* a held slide charges the background */
    if (o.due === undefined) o.due = null; /* only a Notice screen pins the fill; elsewhere the scene's own clock runs */
    if (ui.cur) o.screen = ui.cur.name;
    o.accent = acctColor(ui.acct); /* the account's colour tints the scene */
    KVScene.setState(o);
    document.getElementById('scene').style.opacity = def && def.noScene ? '0' : ''; /* a screen may switch the art off (Price) */
  }

  /* ---- navigation: one screen fades up as the last fades out ---- */
  function show(name, params) {
    if (KVScene.sleep) KVScene.sleep(false); /* whatever happened in the scanner, a new screen wakes the scene */
    var def = SCR[name],
      el = document.createElement('section');
    el.className = 'ui enter' + (def.tab ? ' tabbed' : '');
    ui.cur = { name: name, params: params || {} };
    if (bridge && bridge.screen) bridge.screen(name); /* the phone decides FLAG_SECURE from the screen name */
    el.innerHTML = def.html(ui.cur.params);
    addHelp(el, name);
    var mb = el.querySelector('[data-menu]');
    if (mb && name !== 'home') mb.onclick = openMenu; /* Home wires its own */
    glassify(el); /* bottom-half glass sheet behind everything below the top bar */
    stage.appendChild(el);
    var old = ui.el;
    ui.el = el;
    if (def.after) def.after(el, ui.cur.params);
    if (def.live) def.live(el, ui.cur.params);
    [].forEach.call(el.querySelectorAll('.big,.clock'), fit);
    document.body.classList.toggle('notabs', !def.tab);
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
  function go(name, params, opts) {
    if (ui.stopExplain) ui.stopExplain(); /* the explainer belongs to the screen it was opened on */
    opts = opts || {};
    if (opts.root) ui.stack = [];
    else if (opts.replace) ui.stack.pop();
    ui.stack.push({ name: name, params: params || {} });
    show(name, params);
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
  /* the phone's one-time notices ("pair your phone again", "fingerprints changed") sit on Home until tapped */
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
    if (ui.bio || !hasAccount() || !ui.cur || ui.cur.name === 'lock' || SCR[ui.cur.name].locked) return;
    if (ui.closeSheet) ui.closeSheet(false);
    go('lock', {}, { root: true });
  }
  function lockAfterSecs() {
    return parseInt(D.getSetting('lock_after'), 10) || 0;
  } /* fewer prompts; 0 = at once */
  function wentAway() {
    var secs = lockAfterSecs();
    clearTimeout(ui.awayT);
    if (!secs) return lockNow();
    ui.awayAt = Date.now();
    ui.awayT = setTimeout(lockNow, secs * 1000);
  }
  function cameBack() {
    /* back on the lock screen: the phone's prompt opens by itself */
    clearTimeout(ui.awayT);
    if (ui.awayAt && Date.now() - ui.awayAt >= lockAfterSecs() * 1000) lockNow();
    ui.awayAt = 0;
    if (ui.cur && ui.cur.name === 'lock' && !ui.bio && ui.el) {
      var b = ui.el.querySelector('[data-ok]');
      if (b)
        setTimeout(function () {
          if (!ui.bio && ui.cur && ui.cur.name === 'lock') b.click();
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
      else openDial(t.dataset.explain);
    } /* a bar opens that one dial; no covenant yet: the offer to put one on */ else if (t.dataset.toggle) {
      var on = t.getAttribute('aria-checked') !== 'true';
      t.setAttribute('aria-checked', on);
      if (!t.closest('[data-staged]')) D.setSetting(t.dataset.toggle, on ? 'on' : 'off'); /* a staged screen saves from its bar */
    } else if (t.dataset.acct) {
      ui.acct = t.dataset.acct;
      refresh();
      applyScene();
    } else go(t.dataset.go, t.dataset.params ? JSON.parse(t.dataset.params) : {});
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
  function logRows(n) {
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
            ? 'A withdrawal the guard reported'
            : 'Withdrawal of ' + fmt(pq.amount) + ' KAS to ' + (pq.to || 'an address the guard saw'),
        sub:
          pq.secondsLeft > 0 ? 'Leaves in ' + clock(pq.secondsLeft) : pq.secondsLeft == null ? 'Reported by the guard' : 'Ready to leave',
        stop: 'pending',
        go: 'pending',
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
  function logHtml(n) {
    var rows = (ui.logRows = logRows(n));
    return rows
      .map(function (r, i) {
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

  /* ---- "protected by": five pips that show what guards this money ---- */
  function guardsHtml(g) {
    var pips = g
      ? [
          [true, 'Limit', fmt(g.limit) + ' a day'],
          [true, 'Notice', dur(g.notice)],
          [true, 'Stops', g.stops + (g.stops === 1 ? ' button' : ' buttons')],
          [!!g.heir, 'Heir', g.heir ? 'named' : 'none'],
          [true, 'Vault', g.sheets === 1 ? '1 seed' : '2 of ' + g.sheets],
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
            '</span></button>'
          );
        })
        .join('') +
      '</div>'
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

  /* ---- KAS price: the chart (line coloured by direction, grid, scrub, MA and Fib tools) over the scene's calm glowing line ---- */
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
  function chartFib(C, lo, hi, x0, x1, Y) {
    var s = '';
    C.fib.levels.forEach(function (p) {
      /* retracement from the range high down to its low */
      var fv = hi - ((hi - lo) * p) / 100,
        fy = Y(fv);
      s +=
        '<line class="fib" x1="' +
        x0 +
        '" x2="' +
        x1 +
        '" y1="' +
        fy +
        '" y2="' +
        fy +
        '"/><text class="fibl" x="' +
        (x0 + 4) +
        '" y="' +
        fy +
        '" dy="-2.5">' +
        p +
        '% ' +
        chartMoney(fv) +
        '</text>';
    });
    return s;
  }
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
    if (tools.fib) s += chartFib(C, lo, hi, x0, x1, Y);
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
    s +=
      '<circle class="dot" cx="' +
      X(n - 1) +
      '" cy="' +
      Y(vals[n - 1]) +
      '" r="' +
      C.dot +
      '"/>' +
      '<line class="cursor" data-cursor x1="0" x2="0" y1="' +
      y0 +
      '" y2="' +
      (y0 + ph) +
      '" visibility="hidden"/></svg>';
    return s;
  }
  function chartScrub(box, points, range, restore) {
    /* a finger across the chart reads the price and time at that point in the header; let go: back to the last price */
    var svg = box.querySelector('svg'),
      cur = svg && svg.querySelector('[data-cursor]'),
      k = box.querySelector('.k'),
      when = box.querySelector('[data-when]');
    if (!svg || !cur) return;
    var n = points.length;
    function at(e) {
      var r = svg.getBoundingClientRect(),
        W = +svg.getAttribute('viewBox').split(' ')[2],
        x = ((e.clientX - r.left) / r.width) * W;
      var pw = W - CHART.right - CHART.left,
        i = Math.max(0, Math.min(n - 1, Math.round(((x - CHART.left) / pw) * (n - 1))));
      var px = (CHART.left + (n > 1 ? (i / (n - 1)) * pw : pw / 2)).toFixed(1);
      cur.setAttribute('x1', px);
      cur.setAttribute('x2', px);
      cur.setAttribute('visibility', 'visible');
      k.textContent = 'KAS ' + chartMoney(points[i][1]);
      when.textContent = chartWhen(points[i][0], range);
    }
    function off() {
      cur.setAttribute('visibility', 'hidden');
      when.textContent = '';
      restore();
    }
    svg.onpointerdown = function (e) {
      svg.setPointerCapture && svg.setPointerCapture(e.pointerId);
      at(e);
    };
    svg.onpointermove = function (e) {
      if (e.pressure > 0 || e.buttons) at(e);
    };
    svg.onpointerup = svg.onpointercancel = off;
  }
  function chartAskPrices() {
    ui.priceAsked = ui.priceAsked || {};
    if (D.fetchPrices && !D.priceIsLive(chartRange) && !(ui.priceAsked[chartRange] > Date.now() - 600000)) {
      /* real prices arrive and redraw; a miss is asked again next time */
      ui.priceAsked[chartRange] = Date.now();
      D.fetchPrices(chartRange).then(function (v) {
        if (!v) {
          delete ui.priceAsked[chartRange];
          var b3 = ui.el && ui.cur && ui.cur.name === 'price' && !D.priceIsLive(chartRange) ? ui.el.querySelector('[data-chart]') : null;
          if (b3) b3.innerHTML = '<div class="sub">The price service did not answer. Come back in a moment.</div>';
          return;
        }
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
      '<button data-tool="' +
      CHART.fib.key +
      '"' +
      (tools.fib ? ' class="on"' : '') +
      '>Fib</button></div>'
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
  function chartInto(box) {
    if (!box) return;
    var series = D.priceSeries(chartRange);
    chartAskPrices();
    if (!series) {
      box.innerHTML = '<div class="sub">Fetching the KAS price\u2026</div>';
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
    var pv = points.map(function (p) {
        return p[1];
      }),
      plo = Math.min.apply(null, pv),
      phi = Math.max.apply(null, pv);
    var first = pv[0],
      last = pv[pv.length - 1],
      ch = ((last - first) / first) * 100,
      up = ch >= 0;
    var tools = { fib: D.getSetting(CHART.fib.key) === 'on' };
    CHART.ma.forEach(function (m) {
      tools[m.key] = D.getSetting(m.key) === 'on';
    });
    var head = esc(priceHead());
    var legend = CHART.ma
      .filter(function (m) {
        return tools[m.key];
      })
      .map(function (m) {
        return '<span class="lg ' + m.cls + '">' + m.label + '</span>';
      })
      .join('');
    var W = Math.max(240, Math.round(box.clientWidth || 360)); /* one SVG unit per pixel keeps the hairlines crisp */
    box.innerHTML =
      '<div class="ch"><span class="k">' +
      head +
      '</span><span class="' +
      (up ? 'up' : 'down') +
      '">' +
      (up ? '+' : '') +
      ch.toFixed(1) +
      '%</span><span class="when" data-when></span>' +
      legend +
      '</div>' +
      chartSvg(points, chartRange, tools, W) +
      chartControls(tools) +
      '<div class="sub">Low ' +
      chartMoney(plo) +
      ', high ' +
      chartMoney(phi) +
      (D.priceIsLive(chartRange) ? ', from CoinGecko' : ', sample prices until the real ones arrive') +
      '</div>';
    wireChartButtons(box);
    chartScrub(box, points, chartRange, function () {
      box.querySelector('.k').textContent = priceHead();
    });
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

  /* ---- fingerprint, then slide: every money action. The button asks for the fingerprint; on yes it becomes a slide
     control. The slide charges the background; when it completes, one pulse runs through the scene and the action runs.
     A refused fingerprint changes nothing. ---- */
  function armSlide(btn, title, text, danger, label, kind, action) {
    confirmAction(title, text, danger).then(function (yes) {
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
    s.parentNode.replaceChild(s.kvBtn, s);
    ui.drag = 0;
    applyScene();
  }
  /* the slide goes back to its button when the user edits what was confirmed */
  function disarm(root) {
    var s = root.querySelector('.slide[data-armed]');
    if (!s || !s.kvBtn) return;
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

  /* ---- screens ---- */
  function treeSmooth(pts) {
    /* Catmull-Rom through the points, as cubic curves */
    var d = 'M' + pts[0][0].toFixed(1) + ' ' + pts[0][1].toFixed(1);
    for (var j = 0; j < pts.length - 1; j++) {
      var p0 = pts[Math.max(0, j - 1)],
        p1 = pts[j],
        p2 = pts[j + 1],
        p3 = pts[Math.min(pts.length - 1, j + 2)];
      d +=
        'C' +
        (p1[0] + (p2[0] - p0[0]) / 6).toFixed(1) +
        ' ' +
        (p1[1] + (p2[1] - p0[1]) / 6).toFixed(1) +
        ' ' +
        (p2[0] - (p3[0] - p1[0]) / 6).toFixed(1) +
        ' ' +
        (p2[1] - (p3[1] - p1[1]) / 6).toFixed(1) +
        ' ' +
        p2[0].toFixed(1) +
        ' ' +
        p2[1].toFixed(1);
    }
    return d;
  }
  function treeTendril(T, cx, cy, ex, ey, seed, sign, bow) {
    /* two paths from the centre to (ex, ey): the weave and its mirror, so neighbours cross and interlace */
    var dx = ex - cx,
      dy = ey - cy,
      len = Math.sqrt(dx * dx + dy * dy) || 1,
      nx = -dy / len,
      ny = dx / len,
      lean = T.curve * (Math.abs(dx) + len * 0.2) * bow;
    var side = dx ? (dx > 0 ? 1 : -1) : seed % 2 ? 1 : -1,
      out = [];
    [1, -1].forEach(function (m) {
      var pts = [[cx, cy]];
      for (var t = 0.25; t < 1; t += 0.25) {
        var wv = m * sign * T.weave * Math.sin(T.weaveTurns * 2 * Math.PI * t + seed * 2.4),
          lx = lean * Math.sin(Math.PI * t) * side;
        pts.push([cx + dx * t + nx * wv + lx, cy + dy * t + ny * wv]);
      }
      pts.push([ex, ey]);
      out.push(treeSmooth(pts));
    });
    return out;
  }
  function treeDraw(T, W, H, cx, cy, hit, list, up) {
    var s = '';
    var m = list.length;
    list.forEach(function (e, j) {
      var ex = cx + ((j + 0.5) / m - 0.5) * W * (up ? T.branchSpread : T.rootSpread),
        ey = up ? T.font + 10 : H - T.font - 8,
        seed = j + (up ? 7 : 0),
        bow = 1;
      if (e.small) {
        ex = cx + ((j + 0.5) / m - 0.5) * W * 0.34;
        ey = cy - H * 0.2;
        bow = 1.8;
      } /* grows out and bends back toward the trunk: you hold their stop */
      var w = e.role === 'Trusted payee' || e.faint ? T.thin : T.line,
        p = treeTendril(T, cx, cy, ex, ey, seed, up ? 1 : -1, bow);
      var open = e.guard ? ' data-go="watcher"' : e.stop ? ' data-stop' : e.i != null ? ' data-i="' + e.i + '"' : '';
      s +=
        '<g class="p' +
        (e.faint ? ' faint' : '') +
        '"' +
        open +
        '>' +
        '<path class="glowpath" d="' +
        p[0] +
        '" stroke-width="' +
        (w * T.glow).toFixed(1) +
        '"/>' +
        '<path class="t2" d="' +
        p[1] +
        '" stroke-width="' +
        (T.thin * 0.8).toFixed(2) +
        '"/>' +
        '<path class="t" d="' +
        p[0] +
        '" stroke-width="' +
        w +
        '"/>';
      if (!e.faint) s += treeEnd(T, e, ex, ey, up, cx, hit);
      s += '</g>';
    });
    return s;
  }
  function treeEnd(T, e, ex, ey, up, cx, hit) {
    var s = '';
    var ly = e.small ? ey + 3.5 : up ? ey - 6 : ey + T.font + 3,
      anchor = e.small ? (ex < cx ? 'end' : 'start') : 'middle',
      lx = e.small ? ex + (ex < cx ? -7 : 7) : ex;
    s +=
      '<circle class="end' +
      (e.stop ? ' hollow' : '') +
      '" cx="' +
      ex.toFixed(1) +
      '" cy="' +
      ey.toFixed(1) +
      '" r="' +
      (e.stop ? T.dot * 0.7 : T.dot * 0.45).toFixed(1) +
      '"/>' +
      '<text class="lbl" x="' +
      lx.toFixed(1) +
      '" y="' +
      ly.toFixed(1) +
      '" text-anchor="' +
      anchor +
      '" font-size="' +
      T.font +
      '">' +
      esc(String(e.name || '').slice(0, 10)) +
      '</text>' +
      '<circle class="hit" cx="' +
      ex.toFixed(1) +
      '" cy="' +
      ey.toFixed(1) +
      '" r="' +
      hit +
      '"/>';
    return s;
  }
  /* the People tree: owner at the centre, guardians as roots, heir/guard/payees as branches, sponsored and allowance as small branches bending back */
  function peopleTree(people, state) {
    var T = PEOPLE_TREE,
      W = T.w,
      H = T.h,
      cx = W / 2,
      cy = H * T.trunk,
      s = '',
      hit = T.dot * 3;
    people = people || [];
    state = state || {};
    var guards = [],
      branches = [],
      smalls = [];
    people.forEach(function (q, i) {
      var e = { name: q.name, i: i, role: q.role };
      if (q.role === 'Guardian') guards.push(e);
      else if (q.role === 'Heir' || q.role === 'Trusted payee') branches.push(e);
      else smalls.push(e);
    });
    var named = people.filter(function (q) {
      return q.role === 'Guardian';
    }).length;
    var extra =
      typeof state.stopKeys === 'number'
        ? Math.max(0, state.stopKeys - 1 - named)
        : 0; /* stop keys past your own that no person is named for */
    for (var k = 0; k < extra; k++) guards.push({ name: 'Stop key', stop: true });
    var empty = !people.length && !extra;
    if (!empty && (D.isLive() || D.getSetting('watch_phone') !== 'off'))
      branches.splice(branches.length && branches[0].role === 'Heir' ? 1 : 0, 0, { name: 'Guard', guard: true });
    if (empty) {
      guards.push({ faint: true });
      branches.push({ faint: true });
    }
    smalls.forEach(function (e) {
      e.small = true;
    });
    s += treeDraw(T, W, H, cx, cy, hit, guards, false);
    s += treeDraw(T, W, H, cx, cy, hit, branches, true);
    s += treeDraw(T, W, H, cx, cy, hit, smalls, true);
    s +=
      '<circle class="you" cx="' +
      cx +
      '" cy="' +
      cy +
      '" r="' +
      T.dot +
      '"/><text class="lbl" x="' +
      (cx + T.dot + 5) +
      '" y="' +
      (cy + 3.5) +
      '" font-size="' +
      T.font +
      '">You</text>';
    return (
      '<div class="tree" data-tree><svg viewBox="0 0 ' +
      W +
      ' ' +
      H +
      '" aria-hidden="true"><defs><filter id="kvTreeGlow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="' +
      T.glow +
      '"/></filter></defs>' +
      s +
      '</svg>' +
      (empty ? '<div class="sub center">Nobody in your circle yet</div>' : '') +
      '</div>'
    );
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
    setText(
      el,
      '[data-purse-line]',
      main && !inPurse && s.purse > 0
        ? 'Purse: ' +
            fmt(s.purse) +
            ' KAS outside the account.' +
            (s.unopened || s.stopped || (D.isLocal() && s.rulesNext) ? '' : ' Tap to add it.')
        : ''
    );
    el.querySelector('[data-tour]').hidden = !(D.isLive() && D.getSetting('toured') !== '1' && ph !== 'locked');
    el.querySelector('[data-swap]').classList.toggle('off', !rateOk || bal == null);
  }
  function homeLimitLine(s, main, other, ph) {
    var line;
    if (!main) line = other.limit ? other.limit + ' KAS a day on the spot.' : 'No covenant: payments leave at once. Protect it any time.';
    /* the phase is the main account's; another account always shows its own line */ else if (ph === 'locked') line = '';
    else if (ph === 'oldRules') line = s.oldRulesNotice;
    /* a record this app version cannot read; the engine's plain sentence, nothing alarming */ else if (ph === 'rulesNext')
      line = rulesLine(); /* saved, on their way, or ready to finish */
    else if (ph === 'noKeys') line = 'This phone has no keys yet. Make them, then put money in.';
    else if (ph === 'unopened') line = 'Not open yet. Put KAS in the purse, then open the account.';
    else if (ph === 'stopped') line = 'Stopped. Everything is in your vault.';
    else if (ph === 'stale') line = 'Updating…'; /* the last numbers it saw, until the engine answers */
    else if (s.left == null) line = 'Reaching the network…'; /* connecting or open, before the first numbers */
    else
      line =
        'You can still send ' +
        fmt(s.left) +
        ' KAS today without waiting' +
        (s.refreshSeconds > 0 ? ' (more in ' + (s.refreshSeconds < 60 ? 'under a minute' : dur(s.refreshSeconds)) + ')' : '') +
        '. Payments larger than ' +
        fmt(s.limit) +
        ' KAS wait ' +
        dur(s.noticeSeconds) +
        '.'; /* plain words */
    return line;
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
        '<button class="alarm-ack" data-ack>I did this</button></div>'
      : (hred
          ? '<div class="alarm-text">' +
            (hl > 0 ? 'Your heir can claim in ' + esc(dur(hl)) + '.' : 'Your heir can claim now.') +
            ' Make any payment to restart the clock.</div><div class="alarm-acts"><button class="alarm-ack" data-go="heirclock">See the clock</button></div>'
          : '') + (quiet ? '<div class="alarm-text">' + 'The guard has gone quiet. Check the Guard switch under Watcher.' + '</div>' : '');
    setText(el, '[data-heir]', hl != null && !hred && heirAmber() ? 'Your heir can claim in ' + dur(hl) + '. Tap to see the clock.' : '');
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
  function scanFrame(v, cv, cx) {
    var sc = Math.min(1, 640 / v.videoWidth);
    cv.width = Math.round(v.videoWidth * sc);
    cv.height = Math.round(v.videoHeight * sc);
    cx.drawImage(v, 0, 0, cv.width, cv.height); /* a small frame: enough for a code, light on memory */
    var img = cx.getImageData(0, 0, cv.width, cv.height),
      q = jsQR(img.data, img.width, img.height, { inversionAttempts: 'dontInvert' });
    return q;
  }
  function sendOk(el, d, btn) {
    var n = parseFloat(d.amt) || 0,
      name = d.label || d.to,
      mustWait = onlyTrusted() && !trustedTo(d);
    if (!d.to) {
      setText(el, '[data-e]', 'Who is it for? Enter an address or pick a name.');
      return;
    }
    if (!D.canMove()) {
      setText(el, '[data-e]', 'Not connected to the network yet. Try again in a moment.');
      return;
    }
    if (d.src === 'purse') {
      sendFromPurse(el, d, btn, n, name);
      return;
    }
    if (n <= 0) {
      setText(el, '[data-e]', 'Enter an amount.');
      return;
    }
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
      armSlide(
        self,
        'Send ' + fmt(n) + ' KAS to ' + who,
        waits ? 'More than what is left today: it waits ' + dur(D.state.noticeSeconds) + ' and can be pulled back.' : 'Goes now.',
        false,
        'Slide to send',
        'send',
        function () {
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
    armSlide(
      pself,
      'Send ' + (n > 0 ? fmt(n) + ' KAS' : 'everything in your purse') + ' to ' + pwho,
      'From your purse. Goes now, no notice period.',
      false,
      'Slide to send',
      'send',
      function () {
        pself.disabled = true; /* [data-ok] is gone once the slide replaced it; the captured button stays */
        D.pursePay(pto, n).then(function (r) {
          pself.disabled = false;
          if (!r.ok) {
            resetSlide(el);
            setText(el, '[data-e]', r.error || 'That did not go through. Nothing was sent.');
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
            'One is already waiting',
            'Finish or stop the waiting withdrawal first.',
            null,
          ] /* the vault takes one withdrawal at a time; no queue in Live */
        : waits && qd
          ? [
              'One is already waiting',
              fmt(qd.amount) +
                ' KAS to ' +
                qd.to +
                ' is still in its notice period, ' +
                dur(qd.secondsLeft) +
                ' left. This payment waits in line behind it: its own ' +
                dur(st0.noticeSeconds) +
                ' starts once that one has left, so it arrives in about ' +
                dur(qd.secondsLeft + st0.noticeSeconds) +
                '. Both can still be stopped.',
              'Wait in line',
            ]
          : !waits && og && og.secondsLeft > 0
            ? [
                'Your last payment is still leaving',
                fmt(og.amount) +
                  ' KAS to ' +
                  og.to +
                  ' is in its minute to change your mind. Sending another lets it go now; this one gets the minute instead.',
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
          'You will need your vault seed to use it again.',
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
  function takeOutSlide(el, ok, btn, n, to) {
    var what = n > 0 ? fmt(n) + ' KAS' : 'everything';
    armSlide(
      btn,
      'Take ' + what + ' out of your vault',
      to ? 'To ' + shortAddr(to) + '.' : 'Into your purse.',
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
    armSlide(
      btn,
      'Take ' + what + ' out of your vault',
      p.to ? 'To ' + shortAddr(p.to) + '.' : 'Into your purse.',
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
  var SCR = {
    /* 0. Lock: the door in front of an account that exists */
    lock: {
      money: true,
      scene: function () {
        return { mode: 'safe', snap: true };
      } /* the seal on the unlock screen, formed on arrival */,
      html: function () {
        return (
          '<div class="top"><span class="label">KaspaVault</span>' +
          (D.isLive() ? '' : tag()) +
          '</div><div class="push"></div>' +
          '<button class="sealtap" data-ok aria-label="Open"></button>' /* a glowing centre button, nothing prompts until it is touched */ /* no name, no line; touching the centre of the seal opens */ +
          (D.isLive()
            ? ''
            : '<button class="ghost small" data-pin>Use a PIN</button><label class="field" data-pinbox hidden><span>PIN</span><input data-pinv inputmode="numeric" maxlength="6" autocomplete="off" aria-label="PIN"></label><div class="err" data-e></div>')
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
        if (pinIn)
          pinIn.oninput = function () {
            /* the panic PIN opens a quiet app: Everyday only, savings out of sight */
            var v = pinIn.value.replace(/\D/g, ''),
              panic = D.getSetting('panic_pin');
            if (v.length < 4) return;
            if (panic && v === panic) {
              ui.quiet = true;
              ui.acct = 'everyday';
              pinIn.value = '';
              go('home', {}, { root: true });
              return;
            }
            if (v.length >= (panic ? panic.length : 6)) {
              pinIn.value = '';
              setText(el, '[data-e]', 'Not your PIN. Use your fingerprint, or the PIN in the phone’s own prompt.');
            }
          };
        el.querySelector('[data-ok]').onclick = function () {
          var p =
            D.isLive() && D.phase() === 'locked'
              ? D.retryUnlock()
              : confirmAction('Unlock KaspaVault', ''); /* one fingerprint, then straight to Home, never a second lock */
          p.then(function (yes) {
            if (!yes) return;
            ui.quiet = false;
            go('home', {}, { root: true });
            if (ui.afterLock) {
              go(ui.afterLock, {});
              ui.afterLock = null;
            }
          });
        };
        if (bridge && !document.hidden)
          setTimeout(function () {
            if (ui.cur && ui.cur.name === 'lock' && !ui.bio) el.querySelector('[data-ok]').click();
          }, 350); /* the prompt opens by itself (no bar) */
      },
      live: function () {},
    },
    /* 1. Welcome */
    welcome: {
      money: true,
      html: function () {
        return (
          '<div class="label">KaspaVault</div><div class="push"></div><h1 class="head">Your money, with a stop in your pocket.</h1>' +
          '<button class="pill" data-go="words" data-params=\'{"mode":"wallet"}\'>Create a new wallet</button>' +
          '<button class="ghost" data-go="restore">I have a recovery seed</button><button class="ghost" data-go="invite">I was invited</button>'
        );
      },
    },
    words: {
      locked: false,
      html: function (p) {
        var safe = p.mode === 'safe';
        return (
          topbar(tag()) +
          '<h1 class="head">' +
          (safe ? 'Your vault seed' : 'Your recovery seed') +
          '</h1><p class="line">' +
          (safe
            ? 'Write these on paper. They open your vault, and they stay off this phone.'
            : 'Write these on paper. They bring your wallet back on a new phone.') +
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
              } else setText(el, '.sub', 'Not that one. Check your paper.');
            };
          });
        }
        next();
      },
    },
    restore: {
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Bring your wallet back</h1><label class="field"><span>Your 24-word recovery seed</span><textarea data-w autocapitalize="none" spellcheck="false"></textarea></label><div class="err" data-e></div><div class="push"></div>' +
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
            setText(el, '[data-e]', 'That is not 24 words yet.');
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
          '<div class="alarm" data-alarm hidden></div><button class="notice" data-tour hidden>New here? Show me around.</button><button class="notice quiet" data-notice></button><button class="notice amber" data-heir></button>' +
          '<div class="err" data-e></div><div class="push"></div>' +
          '<div class="big" data-bal></div><button class="denom" data-swap aria-label="Show in the other unit"><span data-usd></span><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 2v10M2 9l3 3 3-3M11 14V4M8 7l3-3 3 3"/></svg></button><div class="line" data-limit></div><button class="notice quiet" data-purse-line></button>' +
          '<button class="sealtap" data-unlock hidden aria-label="Open"></button>' /* the seal itself is the button */ +
          '<div class="guards" data-guards></div>' +
          '<button class="pill" data-rulesfin hidden>Finish updating my rules</button>' +
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
      },
      live: function (el) {
        if (ui.quiet && D.state.everyday) ui.acct = 'everyday'; /* quiet (panic PIN): only Everyday shows */
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
        setText(el, '[data-limit]', line);
        var lock = el.querySelector('[data-unlock]');
        lock.hidden = !(ph === 'locked' && main);
        lock.onclick = function () {
          D.retryUnlock();
        };
        el.classList.toggle('sealed', ph === 'locked' && main); /* while sealed: no account name, no balance, no bars; the seal alone */
        /* the pips come from the engine's status (stop_keys, heir, sheets); until it has answered they stay dim */
        setHtml(el, '[data-guards]', guardsHtml(homeGuardState(s, main, other)));
        var p = s.pending,
          banner = !!(p && !p.dismissed);
        setHtml(el, '[data-log]', logHtml(3));
        setText(el, '[data-notice]', ui.notice || ''); /* the phone's one-time notice; empty hides the row */
        setText(el, '[data-e]', s.error);
        el.querySelector('.bell').classList.toggle('has', banner || !!s.outgoing);
        /* the watcher's word: red for a withdrawal or lineage break not yet owned up to, amber when the guard is silent.
           The stop button opens the usual stop screen (slide, then fingerprint); "I did this" hides this one alert only. */
        homeAlarm(el, s, main);
        var r = ui.quiet ? '' : '<button data-go="safe">Your vault</button>';
        if (main && s.noKeys) r = '<button data-go="makekeys">Make my keys</button>';
        else if (unopened) r = '<button data-go="receive">Put money in</button><button data-go="fundacct">Open the account</button>';
        else if (main && !s.stopped) r += '<button class="s" data-go="stop">Stop</button>';
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
          '<label class="field"><span>Amount</span><div class="amount"><input data-a inputmode="decimal" aria-label="Amount" placeholder="0"><span>KAS</span></div></label><div class="push"></div>' +
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
          done(parseFloat(el.querySelector('[data-a]').value) || 0);
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
          '</div><div class="chart" data-chart></div><div class="push"></div><div class="sub mt8">The price of one KAS in dollars. Your money is counted in KAS; its dollar value moves with this line.</div>'
        );
      },
      after: function (el) {
        chartInto(el.querySelector('[data-chart]'));
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
          (ans ? 'Scan the answer' : 'Scan') +
          '</div><div class="scanbox"><video data-v autoplay playsinline muted></video><i class="scanframe"></i></div><div class="sub center" data-say>' +
          (ans ? 'Point the camera at the code your offline device shows.' : 'Point the camera at a Kaspa address code.') +
          '</div><div class="push"></div>' +
          (ans && !D.isLive() ? '<button class="ghost" data-sample>Use a sample answer</button>' : '')
        );
      },
      after: function (el, p) {
        var sb = el.querySelector('[data-sample]');
        if (sb)
          sb.onclick = function () {
            done('sample-answer');
          };
        var v = el.querySelector('[data-v]'),
          stream = null,
          stop = false,
          cv = document.createElement('canvas'),
          cx = cv.getContext('2d', { willReadFrequently: true });
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
          scanDone(p, text, say, off);
        }
        function tick() {
          if (stop) return;
          if (!ui.cur || ui.cur.name !== 'scan') {
            off();
            return;
          }
          if (v.videoWidth && window.jsQR) {
            var q = scanFrame(v, cv, cx);
            if (q && q.data) {
              done(q.data);
              if (stop) return;
            }
          }
          setTimeout(tick, 150);
        }
        function start() {
          if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
            say('No camera here.');
            return;
          }
          navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment', width: { ideal: 640 }, height: { ideal: 480 } } }).then(
            function (s) {
              if (!ui.cur || ui.cur.name !== 'scan') {
                s.getTracks().forEach(function (t) {
                  t.stop();
                });
                return;
              }
              stream = s;
              v.srcObject = s;
              if (KVScene.sleep) KVScene.sleep(true);
              /* the scene rests only while the camera really runs */ if (!window.jsQR) say('This app cannot read codes from the camera.');
              tick();
            },
            function () {
              say('The camera was refused. Allow it for KaspaVault in the phone\u2019s settings.');
              if (KVScene.sleep) KVScene.sleep(false);
            }
          );
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
          n = parseFloat(ui.draft.amt) || 0,
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
                      return '<button data-c="' + c[0] + '"' + (d.label === c[0] ? ' class="on"' : '') + '>' + c[0] + '</button>';
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
          /* Max = everything in the purse, or what the account can send on the spot today */
          var s = D.state,
            ab = acct(),
            v =
              d.src === 'purse'
                ? s.purse
                : Math.min(
                    ab.balance != null ? ab.balance : Infinity,
                    ui.acct === 'main' ? (s.left != null ? s.left : Infinity) : ab.left != null ? ab.left : Infinity
                  );
          if (typeof v === 'number' && isFinite(v) && v > 0) {
            amt.value = String(Math.floor(v * 100) / 100);
            amt.oninput();
          }
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
        [].forEach.call(el.querySelectorAll('[data-c]'), function (b) {
          b.onclick = function () {
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
        el.querySelector('[data-ok]').onclick = function () {
          sendOk(el, d, this);
        };
      },
      live: function (el) {
        var s = D.state,
          n = parseFloat(ui.draft.amt) || 0,
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
              (lf != null && ab.limit ? ', ' + fmt(lf) + ' on the spot today' : '')
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
          (D.isLive() ? 'This takes about half a minute. You can leave this screen open.' : 'One moment.') +
          '</p>'
        );
      },
      after: function (el, p) {
        if (p.job === 'rules')
          return (p.now ? D.updateNow(rulesStep(el)) : D.finishRules(rulesStep(el))).then(function (r) {
            if (r.ok) {
              haptic();
              return go('home', {}, { root: true });
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
            ui.draft.err = r.error || 'That did not go through. Nothing was sent.';
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
                ? 'It lands in ' + dur(s.pending ? s.pending.secondsLeft : s.noticeSeconds) + ' unless you stop it.'
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
          if (A && navigator.share) navigator.share({ text: A }).catch(function () {});
          else setText(el, '[data-e]', 'Sharing opens on the phone.');
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
          body = '<h1 class="head">' + (me.lockAt && now - me.lockAt < me.lockHold ? 'It has landed' : 'Nothing is waiting') + '</h1>';
          acts = '';
        } else {
          var to = q.to || 'an address the guard saw',
            seen = q.amount == null && !q.to; /* a withdrawal only the guard saw */
          body =
            (q.amount == null ? '' : '<div class="big">' + bigKas(q.amount) + '</div>') +
            '<div class="sub">To ' +
            esc(to.length > 30 ? to.slice(0, 28) + '…' : to) +
            ', asked by ' +
            esc(q.who || 'not known') +
            '</div>' +
            (seen ? '<div class="sub">The guard reported it. The engine on this phone has no details yet.</div>' : '') +
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
            (q.secondsLeft > 0 || seen || !D.isLive() ? '' : '<button class="pill" data-finish>Finish</button>') +
            '<div class="twobtn"><button class="pill" data-decline>Decline it</button><button class="pill" data-let>That was me</button></div>' +
            '<button class="pill danger" data-stop>Stop everything</button>' /* the stop itself, not a way to the stop screen; its own row so the slide has room */ +
            '<div class="sub">Stop everything moves all of this account into your vault, this withdrawal included. Only your vault seed takes it out.</div>' +
            (D.isLive() ? '' : '<button class="ghost" data-skip>Skip ahead</button>');
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
          '<div class="label">Your heir</div><div class="push"></div><div data-body></div><div data-acts></div>'
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
            '<h1 class="head">No heir named</h1><div class="line">An heir can take what is in this account if it hears nothing from you for the silence you set. Never what is in your vault: only your vault seed opens it.</div>';
          acts = D.isLive()
            ? '<button class="pill" data-heir-set>Name an heir</button>'
            : '<button class="pill" data-heir-add>Name an heir</button>'; /* heir.set on this phone */
        } else {
          var warn =
            left == null || sh == null
              ? ''
              : heirRed()
                ? '<div class="line red">' +
                  (left > 0 ? 'Your heir can claim in ' + esc(dur(left)) + '.' : 'Your heir can claim now.') +
                  ' Make any payment to restart the clock.</div>'
                : heirAmber()
                  ? '<div class="line amber">Approaching: ' + esc(dur(left)) + ' left.</div>'
                  : '';
          body =
            '<div class="big">' +
            (left == null ? '\u2014' : heirBig(left)) +
            '</div>' +
            '<div class="sub">' +
            (left == null
              ? 'How long is left is not known yet.'
              : left > 0
                ? 'left before your heir can claim'
                : 'The silence has run out.') +
            '</div>' +
            '<div class="line">Your heir can claim this account after it hears nothing from you for ' +
            esc(typeof total === 'number' && total > 0 ? dur(total) : 'the silence set on it') +
            '. Any payment, top-up or stop restarts the clock.</div>' +
            warn;
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
            'You will need your vault seed to use it again.',
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
            ' into your vault. You will need your vault seed to use it again.'
        );
      },
    },
    stopdone: {
      money: true,
      scene: function () {
        return { mode: 'stopped', k: 1, slow: true };
      },
      html: function () {
        return '<div class="label">Stopped</div><div class="push"></div><div class="big" data-b></div><div class="sub">In your vault</div><div class="line">Nobody can spend this from a phone. Only your vault seed takes it out.</div><button class="pill" data-go="safe">Open your vault</button><button class="ghost" data-home>Home</button>';
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
      scene: function () {
        return { mode: 'locked' };
      } /* the hexagons on the vault */,
      html: function () {
        return (
          topbar() +
          '<div class="label">Your vault</div><div class="push"></div><div class="big" data-b></div><div class="sub" data-usd></div>' +
          '<div class="line mt8">Nothing on this phone can spend it. Only your vault seed takes money out: from your offline device, or typed from paper.</div><div class="list" data-stops></div><div class="push"></div>' +
          '<button class="pill" data-go="takeout">Take money out</button>'
        );
      },
      live: function (el) {
        var s = D.state,
          b = setHtml(el, '[data-b]', bigKas(s.safe));
        fit(b);
        setText(el, '[data-usd]', s.safe == null ? '' : under(s.safe));
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
          var n = parseFloat(amt.value) || Math.floor((have - 1) * 100) / 100;
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
            '<div class="line">Your vault opens with any two of your three sheets, typed from paper. Nothing on this phone can open it alone.</div>' +
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
            '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Next: your sheets</button>'
          );
        }
        return (
          topbar(tag()) +
          '<h1 class="head">Take money out</h1><div class="line">Your vault opens with your vault seed. Your offline device signs for you, or you type the 24 words from paper.</div><div class="push"></div>' +
          '<button class="pill" data-go="airgap">Use my offline device</button><button class="ghost" data-go="takeoutwords">Type the words from paper</button>'
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
          var n = parseFloat(el.querySelector('[data-amt]').value) || 0,
            other = el.querySelector('[data-to] .on').dataset.v === 'other',
            to = other ? el.querySelector('[data-addr]').value.trim().toLowerCase() : '';
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
    /* air gap: the phone shows the request as a code, the offline device signs it, the phone scans the answer. Sample plays it through; Live uses testnet-client airgap (request.hex / response.json). */
    airgap: {
      money: true,
      scene: function () {
        return { mode: 'locked' };
      },
      html: function () {
        var req = 'kvreq:' + (D.state.safe || 0) + ':' + Date.now().toString(36);
        return (
          topbar(tag()) +
          '<div class="label">Step 1 of 2</div><h1 class="head">Show this to your offline device</h1><div class="qrbox"><div class="qr">' +
          KVQR.svg(req) +
          '</div></div>' +
          '<div class="sub center mt8">The request only: ' +
          fmt(D.state.safe) +
          ' KAS back into your account. No words, no keys.</div><div class="push"></div><button class="pill" data-ok>Scan its answer</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-ok]').onclick = function () {
          ui.onScan = function (text) {
            go('airgapdone', { text: text }, { replace: true });
          };
          go('scan', { what: 'answer' });
        };
      },
    },
    airgapdone: {
      money: true,
      scene: function () {
        return { mode: 'locked', heat: ui.drag };
      },
      html: function () {
        return (
          topbar(tag()) +
          '<div class="label">Step 2 of 2</div><h1 class="head">Signed by your offline device</h1><div class="line">' +
          fmt(D.state.safe) +
          ' KAS goes back into your account.</div><div class="push"></div><button class="pill" data-ok>Move it</button>'
        );
      },
      after: function (el) {
        el.querySelector('[data-ok]').onclick = function () {
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
    takeoutwords: {
      money: true,
      scene: function () {
        return { mode: 'locked', heat: ui.drag };
      },
      html: function (p) {
        if (D.isLocal())
          return (
            topbar() +
            '<h1 class="head">Two of your sheets</h1><p class="sub">Type any two of the three sheets you wrote down, 24 words each. They are used once and not kept.</p>' +
            '<label class="field"><span>First sheet</span><textarea data-w1 autocapitalize="none" autocomplete="off" spellcheck="false"></textarea></label>' +
            '<label class="field"><span>Second sheet</span><textarea data-w2 autocapitalize="none" autocomplete="off" spellcheck="false"></textarea></label>' +
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
          var list = String(window.KV_BIP39 || '').split(/\s+/),
            ok = el.querySelector('[data-ok]');
          function sheet(sel) {
            var ws = el.querySelector(sel).value.trim().toLowerCase().split(/\s+/).filter(Boolean);
            if (ws.length !== 24) return 'That is not 24 words yet.';
            for (var i = 0; i < 24; i++)
              if (list.length > 100 && list.indexOf(ws[i]) < 0) return 'Word ' + (i + 1) + ' is not on the list: ' + ws[i];
            return ws.join(' ');
          }
          ok.onclick = function () {
            var a = sheet('[data-w1]'),
              b = sheet('[data-w2]');
            if (a.indexOf(' ') < 0 || a.split(' ').length !== 24) {
              setText(el, '[data-e]', 'First sheet: ' + a);
              return;
            }
            if (b.indexOf(' ') < 0 || b.split(' ').length !== 24) {
              setText(el, '[data-e]', 'Second sheet: ' + b);
              return;
            }
            if (a === b) {
              setText(el, '[data-e]', 'Those are the same sheet. Type two different ones.');
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
            setText(el, '[data-e]', 'That is not 24 words yet.');
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
      html: function () {
        var tree =
          '<div class="treegap"></div>'; /* room for the weave above the list; peopleTree (the drawn SVG) is kept in the file but no longer shown */
        return (
          '<div class="top"><span class="label">People</span>' +
          tag() +
          menuBtn() +
          '</div>' +
          tree +
          '<div class="list" data-l></div><button class="pill" data-go="addperson">Add someone</button>'
        );
      },
      after: function (el) {
        el.addEventListener('click', function (e) {
          /* an end of the tree opens what that person's row opens; the guard end carries data-go, the owner opens nothing */
          var g = e.target.closest('[data-tree] [data-i],[data-tree] [data-stop]');
          if (!g) return;
          if (g.hasAttribute('data-stop')) {
            openDial('stops');
            return;
          }
          var q = ui.people[+g.dataset.i];
          if (!q) return;
          if (q.role === 'Heir') {
            go('heirclock', {});
            return;
          }
          nameSheet('Rename ' + q.name, q.name, function (v) {
            q.note = q.note.split(q.name).join(v);
            q.name = v;
            refresh();
          });
        });
        el.querySelector('[data-l]').addEventListener('click', function (e) {
          var ed = e.target.closest('[data-edit]');
          if (ed) {
            var q0 = ui.people[+ed.dataset.edit];
            if (q0)
              nameSheet('Rename ' + q0.name, q0.name, function (v) {
                q0.note = q0.note.split(q0.name).join(v);
                q0.name = v;
                refresh();
              });
            return;
          }
          var b = e.target.closest('[data-rm]');
          if (!b) return;
          var q = ui.people[+b.dataset.rm];
          if (!q) return;
          function drop() {
            ui.people.splice(+b.dataset.rm, 1);
            if (
              q.role === 'Heir' &&
              !D.isLive() &&
              !ui.people.some(function (x) {
                return x.role === 'Heir';
              })
            )
              D.state.heir = false;
            refresh();
          }
          if (q.role === 'Guardian' || q.role === 'Heir') {
            /* a guardian or an heir is a rule in the coin, so removing one is a change of the rules */
            if (D.isLocal()) {
              if (q.role === 'Heir') openHeirSheet();
              else openKeysFact('Guardians');
              return;
            } /* on this phone the heir is heir.set, guardians are part of the keys */
            openChangeRules(
              'Remove ' + q.name + ' as ' + (q.role === 'Guardian' ? 'a guardian' : 'your heir'),
              q.role === 'Guardian'
                ? 'Their copy of your emergency stop stops working.'
                : 'They can no longer take this account after a silence.',
              drop
            );
            return;
          }
          confirmAction(
            'Remove ' + q.name,
            q.role === 'Trusted payee'
              ? 'Payments to them follow the usual rules again.'
              : 'They lose the account you set up for them; the money in it comes back to you.'
          ).then(function (yes) {
            if (yes) drop();
          });
        });
      },
      live: function (el) {
        var t = el.querySelector('[data-tree]');
        if (t) {
          var th = peopleTree(ui.people, D.state);
          if (t.dataset.h !== th) {
            t.outerHTML = th;
            el.querySelector('[data-tree]').dataset.h = th;
          }
        } /* redrawn on a rename or a removal */
        setHtml(
          el,
          '[data-l]',
          ui.people
            .map(function (p, i) {
              var heir = p.role === 'Heir',
                hl = heir ? heirLeft() : null; /* the heir row opens the heir clock (no rename there) */
              return (
                '<div class="item"><button class="rowmain" ' +
                (heir ? 'data-go="heirclock"' : 'data-edit="' + i + '"') +
                '><span><span class="t">' +
                esc(p.name) +
                '</span><span class="d">' +
                esc(p.role + '. ' + p.note + (hl != null ? ' ' + dur(hl) + ' left on the clock.' : '')) +
                '</span></span></button><button class="r go" data-rm="' +
                i +
                '">Remove</button></div>'
              );
            })
            .join('') || '<div class="sub">Nobody yet.</div>'
        );
      },
    },
    addperson: {
      tab: 'people',
      html: function () {
        function opt(k, t, d) {
          return (
            '<button class="item" data-go="persondetail" data-params=\'{"kind":"' +
            k +
            '"}\'><span><span class="t">' +
            t +
            '</span><span class="d">' +
            d +
            '</span></span></button>'
          );
        }
        return (
          topbar(tag()) +
          '<h1 class="head">Add someone</h1><div class="list">' +
          opt('Sponsored', 'Sponsor a newcomer', 'They spend from it. You hold the emergency stop until they take it over.') +
          opt('Guardian', 'Add a guardian', 'You spend as before. They hold a copy of your emergency stop.') +
          opt('Allowance', 'Give an allowance', 'They spend up to a limit you choose. You hold the stop.') +
          opt(
            'Heir',
            'Name an heir',
            'After a silence you set, they can take what is in this account. Never what is in your vault: only your vault seed opens that.'
          ) +
          opt(
            'Payee',
            'Add a trusted payee',
            'Someone you pay often. With "Only trusted payees on the spot" on, payments to anyone else wait.'
          ) +
          '</div>'
        );
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
            ? '<div class="dial"><div class="h"><span>Silence before they can take it</span><b data-l>6 months</b></div><input type="range" aria-label="Months of silence" data-r min="1" max="24" step="1" value="6"></div><div class="sub">Silence means no payment from this account for that long. Every payment you make restarts it.</div>'
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
            ui.people.push({ name: n, role: 'Trusted payee', note: 'Payments to ' + addr.slice(0, 14) + '… go on the spot.', addr: addr });
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
                  ? 'After ' + r.value + ' months of silence, ' + n + ' can take what is in this account.'
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
          var t = 'Join me on KaspaVault: kaspavault.app/join/sample-' + encodeURIComponent(p.name.toLowerCase());
          if (navigator.share) navigator.share({ text: t }).catch(function () {});
          else setText(el, '[data-e]', 'Sharing opens on the phone.');
        };
        el.querySelector('[data-done]').onclick = function () {
          go('people', {}, { replace: true });
        };
      },
    },

    /* 10. Invite link flow (the newcomer's side) */
    invite: {
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
        setHtml(el, '[data-l]', logHtml(200) || '<div class="sub">Nothing yet.</div>');
      },
    },

    /* coins: money sits in coins; many small ones cost more to spend (Kaspa storage mass), so combine them when it is quiet */
    coins: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Coins</h1><p class="line">Your money sits in coins, the way cash sits in notes. A payment uses some and leaves change. Many small coins cost more to spend, so combine them when things are quiet.</p><div class="list" data-l></div><div class="sub mt8" data-say></div><div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Combine into one</button>'
        );
      },
      live: function (el) {
        var s = D.state,
          c = s.coins || [];
        var rows =
          '<div class="item"><span><span class="t">' +
          esc(s.account || 'Your account') +
          '</span><span class="d">One coin. Your rules keep it that way.</span></span><span class="r">' +
          fmt(s.balance) +
          ' KAS</span></div>';
        if (s.everyday)
          rows +=
            '<div class="item"><span><span class="t">' +
            esc(s.everyday.name) +
            '</span><span class="d">' +
            c.length +
            (c.length === 1 ? ' coin: ' : ' coins: ') +
            c
              .map(function (x) {
                return fmt(x);
              })
              .join(', ') +
            '</span></span><span class="r">' +
            fmt(s.everyday.balance) +
            ' KAS</span></div>';
        setHtml(el, '[data-l]', rows);
        var fee = c.length > 1 ? Math.round(c.length * 0.4) / 100 : 0;
        setText(
          el,
          '[data-say]',
          D.isLive()
            ? 'Combining coins is not part of this app on a live account.'
            : c.length > 1
              ? 'Combining ' + c.length + ' coins costs about ' + fee + ' KAS, under a cent.'
              : 'Nothing to combine.'
        );
        var b = el.querySelector('[data-ok]');
        if (b) b.disabled = c.length < 2;
      },
      after: function (el) {
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
    /* the watcher (all in one): the phone watches while it is on; a second watcher covers the rest */
    watcher: {
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">The guard</h1><p class="line">The guard watches your money on the Kaspa network and raises the alert when a withdrawal is asked for or something is not right. This phone guards while it is on. For the hours it is off, or if it is lost, a second guard is wise: it holds no keys, only your addresses.' +
          (D.isLocal() && (D.state.accounts || []).length > 1 ? ' The guard watches account 1.' : '') +
          '</p><div class="list">' +
          (D.isLocal()
            ? '<div class="item"><span class="t">Guard from this phone</span><span class="seg" data-guard><button data-v="1">On</button><button data-v="0">Off</button></span></div>'
            : toggleRow('watch_phone', 'Guard from this phone')) +
          row('seconddevice', 'Set up a second guard', 'A second phone or tablet') +
          '</div>' +
          (D.isLocal()
            ? '<label class="field"><span>Second node, optional</span><input data-nodes autocapitalize="none" spellcheck="false" placeholder="wss://…" value="' +
              esc(D.getSetting('guard_nodes')) +
              '"></label><div class="sub" data-gsay></div>'
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
              ? 'The guard is on. A small notice stays in the shade while it watches.'
              : 'The guard is off. Nothing watches your money while the app is closed.'
          );
        }
        paint(!!bridge.guardRunning());
        [].forEach.call(seg.children, function (b) {
          b.onclick = function () {
            var on = b.dataset.v === '1';
            D.setSetting('guard_nodes', el.querySelector('[data-nodes]').value.trim());
            bridge.guardOn(on);
            haptic();
            setTimeout(function () {
              paint(!!bridge.guardRunning());
            }, 900);
          };
        });
      },
    },
    /* the panic PIN: typed on the unlock screen, the app opens quiet. Sample keeps it in settings; Live would keep it in the phone's secure store. */
    panicpin: {
      tab: 'settings',
      html: function () {
        var set = !!D.getSetting('panic_pin');
        return (
          topbar(tag()) +
          '<h1 class="head">Panic PIN</h1><p class="line">A PIN you type on the unlock screen when someone makes you open the app. The app opens showing only your Everyday money; your protected account and your vault stay out of sight, and your guardians get a quiet alert.</p>' +
          '<label class="field"><span>' +
          (set ? 'New PIN' : 'Choose a PIN') +
          ', 4 to 6 digits</span><input data-p inputmode="numeric" maxlength="6" autocomplete="off"></label><div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>' +
          (set ? 'Change it' : 'Set it') +
          '</button>' +
          (set ? '<button class="ghost" data-off>Remove it</button>' : '')
        );
      },
      after: function (el) {
        el.querySelector('[data-ok]').onclick = function () {
          var v = el.querySelector('[data-p]').value.replace(/\D/g, '');
          if (v.length < 4) {
            setText(el, '[data-e]', 'Four to six digits.');
            return;
          }
          confirmAction('Set your panic PIN', '').then(function (yes) {
            if (!yes) return;
            D.setSetting('panic_pin', v);
            go('protection', {}, { replace: true });
          });
        };
        var off = el.querySelector('[data-off]');
        if (off)
          off.onclick = function () {
            confirmAction('Remove your panic PIN', '').then(function (yes) {
              if (!yes) return;
              D.setSetting('panic_pin', '');
              go('protection', {}, { replace: true });
            });
          };
      },
    },
    /* a statement: this month's movements and the total out, shareable as text */
    statements: {
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Statement</h1><div class="sub">This month, ' +
          esc(D.state.account || 'your account') +
          '</div><div class="list" data-l></div><div class="sub mt8" data-tot></div><div class="push"></div><button class="pill" data-share>Share it</button>'
        );
      },
      live: function (el) {
        var h = D.state.history,
          out = 0;
        setHtml(
          el,
          '[data-l]',
          h
            .map(function (a) {
              var m = /([\d.,]+) KAS/.exec(a.text),
                v = m ? parseFloat(m[1].replace(/,/g, '')) : 0;
              if (a.kind === 'paid') out += v;
              return (
                '<div class="item"><span><span class="t">' +
                esc(a.text) +
                '</span><span class="d">' +
                esc(a.when || '') +
                '</span></span><span class="r">' +
                (a.kind === 'paid' && v ? '−' + fmt(v) : '') +
                '</span></div>'
              );
            })
            .join('') || '<div class="sub">Nothing yet.</div>'
        );
        setText(
          el,
          '[data-tot]',
          'Out this month: ' + fmt(out) + ' KAS' + (D.isLive() ? '' : ', ' + money(out)) + '. Balance now ' + fmt(D.state.balance) + ' KAS.'
        );
      },
      after: function (el) {
        el.querySelector('[data-share]').onclick = function () {
          var t =
            'KaspaVault statement, ' +
            (D.state.account || '') +
            ': ' +
            D.state.history
              .map(function (a) {
                return (a.when ? a.when + ': ' : '') + a.text;
              })
              .join('; ');
          if (navigator.share) navigator.share({ text: t }).catch(function () {});
          else setText(el, '[data-tot]', 'Sharing opens on the phone.');
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
        function rule(key, title, right) {
          return (
            '<button class="item" data-explain="' +
            key +
            '"><span><span class="t">' +
            title +
            '</span></span><span class="r">' +
            esc(right) +
            '</span></button>'
          );
        }
        var rules = g
          ? rule('limit', 'Daily limit', fmt(s.limit) + ' KAS') +
            rule('notice', 'Notice period', dur(n)) +
            rule('stops', 'Emergency stop', g.stops + (g.stops === 1 ? ' button' : ' buttons')) +
            rule(
              'heir',
              'Heir',
              g.heir ? (ui.acct === 'main' && heirLeft() != null ? 'named, ' + dur(heirLeft()) + ' left' : 'named') : 'none'
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
          toggleRow('alert_sam', 'Sam', 'Guardian') +
          '</div>' +
          (D.isLive()
            ? ''
            : '<div class="label mt20">More protection</div><div class="list mt0">' +
              toggleRow(
                'only_trusted',
                'Only trusted payees on the spot',
                'Payments to anyone else wait for the notice period, whatever the amount. Add payees under People.',
                true
              ) +
              toggleRow('watch_only', 'This phone only watches', 'It can see and stop, not send.', true) +
              toggleRow(
                'clip_check',
                'Check pasted addresses',
                'Shows the start and end of an address you paste, to compare with where it came from.'
              ) +
              row('panicpin', 'Panic PIN', 'Opens the app quiet, Everyday only.', D.getSetting('panic_pin') ? 'Set' : 'Not set') +
              '</div>')
        );
      },
      after: function (el, p) {
        if (p && p.focus)
          setTimeout(function () {
            if (ui.guards) openDial(p.focus);
          }, 350);
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
          var pk = STEP_PEEK[ui.step] && peek(STEP_PEEK[ui.step]);
          if (pk) {
            var why = box.querySelector('.why');
            why.parentNode.insertBefore(pk, why.nextSibling);
            [].forEach.call(pk.querySelectorAll('.big,.clock'), fit);
          }
          setText(el, '[data-count]', ui.step + 1 + ' of ' + STEPS.length);
          [].forEach.call(marks.children, function (m, i) {
            m.classList.toggle('on', i <= ui.step);
          });
          nav.innerHTML =
            (ui.step ? '<button class="pill" data-prev>Back</button>' : '') +
            (ui.step < STEPS.length - 1
              ? '<button class="pill" data-next>Next</button>'
              : '<button class="pill" data-done>' + (p.intro ? 'Open my account' : 'Done') + '</button>');
          var dn = nav.querySelector('[data-done]');
          if (dn)
            dn.onclick = function () {
              if (p.intro) {
                go('home', {}, { root: true });
                setTimeout(explainScreen, 700);
              } else back();
            };
          if (dir) {
            box.classList.add(dir > 0 ? 'fromright' : 'fromleft');
            requestAnimationFrame(function () {
              requestAnimationFrame(function () {
                box.classList.remove('fromright', 'fromleft');
              });
            });
          }
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
          row('protection', 'Your rules', 'Daily limit, notice period, alerts, trusted payees, panic PIN') +
          (live ? '' : row('open', 'Open an account')) +
          row('coins', 'Coins', 'Combine small coins') +
          row('statements', 'Statement') +
          '</div>' +
          '<div class="label mt20">Keys</div><div class="list mt0">' +
          (D.isLocal()
            ? row('makekeys', 'Make my keys', 'From your offline device\u2019s public file') +
              row('keysmade', 'Your rule block code', 'And the public file to carry') +
              row('showsheets', 'Show my sheets', 'Your three vault sheets, after a fingerprint') +
              row('burnsheets', 'Burn the sheets on this phone', 'Only paper opens the vault after this') +
              row('pursepay', 'Pay from the purse', 'To any address, at once') +
              row('resetkeys', 'Start fresh keys', 'Once the purse and the account are empty')
            : '') +
          (D.isLocal()
            ? ''
            : row('words', 'Recovery drill', 'Check you can still find and read your recovery seed').replace(
                'data-go="words"',
                'data-go="words" data-params=\'{"mode":"check"}\''
              ) + row('restore', 'Recover a wallet', 'From your 24-word recovery seed')) +
          row('watcher', 'The guard', D.isLocal() ? 'This phone watches your money' : 'This phone, and a second one') +
          (D.isLocal() ? '' : row('seconddevice', 'Second device')) +
          '</div>' +
          (D.isLocal()
            ? '<div class="sub" data-fpwarn>Adding a fingerprint to this phone clears your everyday keys; your safe sheets stay.</div>'
            : '') +
          '<div class="label mt20">Help</div><div class="list mt0">' +
          row('howitworks', 'How it works') +
          row('glossary', 'Library: crypto and Kaspa') +
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
          '<div class="item"><span class="t">Lock when put away</span><span class="seg" data-lock>' +
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
            'A minute to change your mind',
            'An on-the-spot send waits a minute on this phone with a Stop. Off, it leaves at once and is final within a second.',
            true
          ) +
          toggleRow('alert_push', 'Push alerts') +
          toggleRow('alert_email', 'Email alerts') +
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
        var keys = ['balance_money', 'send_grace', 'alert_push', 'alert_email', 'screenshots'];
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
        stageChanges(el, read, function (v) {
          for (var k in v) D.setSetting(k, v[k]);
          return true;
        });
      },
    },
    /* this phone makes and seals its own three keys. The offline device's public file comes by
       camera or paste; the heir's public key is typed or pasted. One fingerprint seals the keys; the key bytes never reach this page. */
    /* Make my keys: two doors, the simple one first. */
    makekeys: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Make my keys</h1><p class="sub">Your account needs three keys: a card for everyday payments, a stop, and a purse. Pick how they are made.</p>' +
          '<div class="list mt8"><button class="item" data-phone><span><span class="t">Make everything on this phone</span><span class="d">Recommended. One tap and one fingerprint, then three sheets of words to write down.</span></span></button>' +
          '<button class="item" data-go="keysdrawer"><span><span class="t">I have a drawer device</span><span class="d">Advanced. The vault half was made on a device that never goes online.</span></span></button></div>' +
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
            if (!r.ok) {
              setText(el, '[data-e]', r.error);
              return;
            }
            haptic();
            if (r.sheets && r.sheets.length) {
              ui.sheets = r.sheets;
              go('sheet', { n: 0 }, { replace: true });
            } /* the words live only in this page's memory until read back */ else go('keysmade', {}, { replace: true });
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
          '<div class="label">Safe sheet ' +
          (p.n + 1) +
          ' of 3</div><h1 class="head">Write these on paper.</h1><p class="line">Any two of the three sheets open your vault. Keep them in three places, never a photo. Make sure nobody is looking over your shoulder.</p>' +
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
            ? '<button class="pill" data-go="sheetcheck" data-params=\'' + JSON.stringify({ n: p.n }) + "'>I wrote them down</button>"
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
            if (p.n < 2) return go('sheet', { n: p.n + 1 }, { replace: true });
            ui.sheets = null;
            return go('keysmade', {}, { replace: true }); /* all three read back: the words are gone from this phone */
          }
          var n = ask[at],
            right = sh[n],
            opts = [right, sh[(n + 7) % 24], sh[(n + 15) % 24]].sort();
          setText(el, '[data-q]', 'Sheet ' + (p.n + 1) + ', check ' + (at + 1) + ' of 3');
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
              } else setText(el, '.sub', 'Not that one. Check your paper.');
            };
          });
        }
        next();
      },
    },
    /* The drawer-device tutorial: one step per page */
    offlinehow: {
      tab: 'settings',
      html: function (p) {
        var i = p.step || 0,
          s = OFFLINE_STEPS[i];
        return (
          topbar() +
          '<div class="label">Step ' +
          (i + 1) +
          ' of ' +
          OFFLINE_STEPS.length +
          '</div><h1 class="head">' +
          esc(s[0]) +
          '</h1><p class="line">' +
          esc(s[1]) +
          '</p><div class="push"></div>' +
          '<div class="twobtn">' +
          (i > 0 ? '<button class="ghost" data-prev>Back</button>' : '') +
          '<button class="pill" data-next>' +
          (i + 1 < OFFLINE_STEPS.length ? 'Next' : 'Done') +
          '</button></div>'
        );
      },
      after: function (el, p) {
        var i = p.step || 0,
          pv = el.querySelector('[data-prev]');
        if (pv)
          pv.onclick = function () {
            go('offlinehow', { step: i - 1 }, { replace: true });
          };
        el.querySelector('[data-next]').onclick = function () {
          if (i + 1 < OFFLINE_STEPS.length) go('offlinehow', { step: i + 1 }, { replace: true });
          else back();
        };
      },
    },
    keysdrawer: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">I have a drawer device</h1><p class="sub">Your drawer device made the vault half. Bring its public file here by camera or paste, name the heir, and this phone makes and seals its own keys. Nothing secret is typed.</p>' +
          '<button class="ghost small" data-go="offlinehow">How a drawer device is made, step by step</button>' +
          (D.isLive()
            ? '<div class="sub">No drawer device? Go back and make everything on this phone' +
              (D.isMainnet() ? '' : ', or use the test drawer for now') +
              '.</div>'
            : '') +
          '<label class="field"><span>Offline device, public file</span><textarea data-off autocapitalize="none" spellcheck="false" placeholder="{&quot;kind&quot;:&quot;offline-public&quot;, \u2026}"></textarea></label>' +
          '<button class="ghost small" data-scan>Scan it instead</button>' +
          '<label class="field"><span>Heir, public key</span><input data-heir autocapitalize="none" spellcheck="false" maxlength="64" placeholder="64 characters, or empty for no heir"></label>' +
          '<div class="dial"><div class="h"><span>Heir claims without a signature</span></div><span class="seg" data-auto><button data-v="1" class="on">On</button><button data-v="0">Off</button></span></div>' +
          '<div class="dial"><div class="h"><span>Hard limit</span></div><span class="seg" data-hard><button data-v="0" class="on">Off</button><button data-v="1">On</button></span></div>' +
          '<div class="sub">On, nothing above the daily limit can ever leave this account, not even with notice.</div>' +
          '<div class="err" data-e></div><div class="push"></div>' +
          '<button class="pill" data-ok>Make my keys</button>' +
          ''
        );
      },
      after: function (el) {
        var off = el.querySelector('[data-off]'),
          heir = el.querySelector('[data-heir]'),
          ok = el.querySelector('[data-ok]');
        if (ui.keyDraft) {
          off.value = ui.keyDraft.off;
          heir.value = ui.keyDraft.heir;
        }
        if (ui.scanned) {
          off.value = ui.scanned;
          ui.scanned = '';
        }
        wireSeg(el, '.seg');
        function on(sel) {
          return el.querySelector(sel + ' .on').dataset.v === '1';
        }
        el.querySelector('[data-scan]').onclick = function () {
          ui.keyDraft = { off: off.value, heir: heir.value };
          ui.onScan = function (text) {
            ui.scanned = text;
            go('keysdrawer', {}, { replace: true });
          };
          go('scan', { what: 'answer' });
        };
        ok.onclick = function () {
          var text = off.value.trim(),
            hx = heir.value.trim().toLowerCase(),
            j = null;
          try {
            j = JSON.parse(text);
          } catch (e) {}
          if (!j || j.kind !== 'offline-public') {
            setText(el, '[data-e]', 'That is not an offline device\u2019s public file.');
            return;
          }
          if (hx && !/^[0-9a-f]{64}$/.test(hx)) {
            setText(el, '[data-e]', 'The heir\u2019s public key is 64 characters, 0-9 and a-f.');
            return;
          }
          ok.disabled = true;
          ok.textContent = 'Making\u2026';
          setText(el, '[data-e]', '');
          D.makeKeys({ offline_public: text, heir_x: hx, auto_heir: on('[data-auto]'), hard_limit: on('[data-hard]') }).then(function (r) {
            ok.disabled = false;
            ok.textContent = 'Make my keys';
            if (r.ok) {
              haptic();
              ui.keyDraft = null;
              go('keysmade', {}, { replace: true });
            } else {
              resetSlide(el);
              setText(el, '[data-e]', r.error);
            }
          });
        };
      },
    },
    /* The rule block code (the sha of the public block): the offline device shows the same one. The public file can be copied out. */
    keysmade: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Your rule block</h1><p class="sub">This code names the rules your money will live under. Your offline device shows the same code; if they differ, stop and ask.</p>' +
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
          if (have >= 1.05) el.querySelector('[data-amt]').value = String(Math.floor((have - 1) * 100) / 100);
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
              : 'Your purse holds ' + fmt(have) + ' KAS. Put money in first: send test KAS to this address, then come back.'
          );
          addr.hidden = cp.hidden = enough;
          addr.textContent = r.address;
          ok.disabled = !enough;
        });
        cp.onclick = function () {
          copyPurseAddr(el, addr);
        };
        ok.onclick = function () {
          var n = parseFloat(el.querySelector('[data-amt]').value) || 0;
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
          var dials = re
            ? null
            : D.isMainnet()
              ? {
                  cap_kas: ui.liveRules.limit,
                  delay: Math.round(ui.liveRules.notice * 36000),
                  window: Math.round(ui.liveRules.window * 36000),
                }
              : null; /* DAA units at 10 a second */
          D.fund(n, dials, an || undefined).then(function (r) {
            ok.disabled = false;
            ok.textContent = re ? 'Open the account again' : an ? 'Open account ' + an : 'Open the account';
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
            else go('home', {}, { root: true }); /* Home shows the account just opened */
          });
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
          '<h1 class="head">Your sheets</h1><p class="sub">Any two of the three open your vault. Write them on paper and keep them in three places; never a photo.</p><div data-sheets></div><div class="err" data-e></div><div class="push"></div>'
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
                '<div class="label mt20">Sheet ' +
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
          '<h1 class="head">Burn the sheets on this phone</h1><p class="line">This phone keeps a sealed copy of your three vault sheets, so it can show them again and open the vault for you. Burning the copy means only your paper sheets open the vault from then on. Nothing on this phone, and nobody who takes it, can.</p>' +
          '<p class="sub">It cannot be undone. If the paper is lost, the money in the vault is lost with it.</p>' +
          '<div class="dial"><div class="h"><span>I have all three sheets on paper and read them back</span></div><span class="seg" data-sure><button data-v="0" class="on">No</button><button data-v="1">Yes</button></span></div>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok disabled>Burn them</button>'
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
            'Burn the sheets on this phone',
            'Only paper opens the vault after this.',
            true,
            'Slide to burn them',
            'stop',
            function () {
              D.burnSheets().then(function (r) {
                if (r.ok) {
                  haptic();
                  setText(el, '[data-e]', 'Burned. Only your paper opens the vault now.');
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
          '<h1 class="head">Pay from the purse</h1><p class="sub">Money in the purse is outside the account and its rules. It goes where you say, at once.</p>' +
          '<div class="sub" data-purse>Reading the purse\u2026</div>' +
          '<label class="field"><span>To</span><input data-to autocapitalize="none" spellcheck="false" placeholder="kaspa:\u2026"></label>' +
          '<label class="field"><span>Amount, KAS<button class="maxb" data-max type="button">Max</button></span><input data-amt inputmode="decimal" placeholder="All of it"></label>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Pay</button>'
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
            n = parseFloat(el.querySelector('[data-amt]').value) || 0;
          if (!isKaspaAddress(to)) {
            setText(el, '[data-e]', 'That is not a Kaspa address.');
            return;
          }
          if (have != null && n > 0 && n >= Math.floor(have * 100) / 100) n = 0; /* Max or more = everything, the fee comes off the top */
          if (have != null && n > have) {
            setText(el, '[data-e]', 'The purse holds ' + fmt(have) + ' KAS.');
            return;
          }
          armSlide(
            this,
            'Pay ' + (n > 0 ? fmt(n) + ' KAS' : 'everything in the purse'),
            'To ' + shortAddr(to) + '. At once, no notice period.',
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
    /* Fresh keys on this phone (engine keys.reset): only once the purse and the account are empty; the old folder is kept aside. */
    resetkeys: {
      tab: 'settings',
      money: true,
      html: function () {
        return (
          topbar() +
          '<h1 class="head">Start fresh keys</h1><p class="line">New keys mean a new purse, a new account and a new vault. The old ones are set aside, not deleted. This works only once the purse and the account are empty: pay the purse out first.</p>' +
          '<div class="dial"><div class="h"><span>The vault’s sheets are lost: leave the vault behind</span></div><span class="seg" data-leave><button data-v="0" class="on">No</button><button data-v="1">Yes</button></span></div>' +
          '<div class="sub">Yes means the money in the old vault stays where it is, for good. Only choose it when no two sheets exist anywhere.</div>' +
          '<div class="err" data-e></div><div class="push"></div><button class="pill" data-ok>Start fresh</button>'
        );
      },
      after: function (el) {
        var ok = el.querySelector('[data-ok]');
        wireSeg(el, '[data-leave]');
        ok.onclick = function () {
          var leave = el.querySelector('[data-leave] .on').dataset.v === '1';
          confirmAction(
            'Start fresh keys',
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
      tab: 'settings',
      html: function () {
        return (
          topbar(tag()) +
          '<h1 class="head">Second device</h1><p class="line">A second phone or tablet can keep your vault seed, so this one never has to, and can hear about every withdrawal. Linking one is not part of this app.</p><div class="list">' +
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
          '<h1 class="head">KaspaVault</h1><p class="line">Version 1.0</p><p class="sub">An early build. In Sample mode nothing is real and nothing leaves this phone.</p><div class="push"></div>' +
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
  };

  /* ---- start ---- */
  D.subscribe(function () {
    refresh();
    applyScene();
  });
  KVScene.init(document.getElementById('scene'));
  D.boot();
  if (D.isLive()) ui.people = []; /* a live account starts with nobody in it */
  go(D.getSetting('welcomed') === '1' || D.isLive() ? 'lock' : D.demo ? 'home' : 'welcome', {}, { root: true });
  if (document.fonts && document.fonts.ready)
    document.fonts.ready.then(function () {
      [].forEach.call(document.querySelectorAll('.big,.clock'), fit);
    });
  window.KVApp = { go: go, back: back, screens: SCR, ui: ui };
})();
