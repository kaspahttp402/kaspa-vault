/* The moving art: one physical ribbon, tuned on the "Tune the ribbon" page (its dials are below).
   Swappable: nothing else in the app draws or depends on what is here. A replacement only has to keep this interface.

   KVScene.init(canvas)
   KVScene.setState({left: 0..100, mode, k: 0..1 collapse, dim: bool, slow: bool, screen: name, due: 0..1 | null, charge: 0..1 | null, heat: 0..1 | null, snap: bool})
   charge: while the person holds a slider (0 = not): the glow rises, the main lines brighten and the lights run faster; nothing moves.
   Eased over about 0.25 s. undefined / null = 0. heat: the stop colour while a stop slider is held, 0 green through
        yellow and orange to 1 red (null = red). snap: the mode asked for is put in place already formed, no morph (the unlock screen)
   KVScene.pulse(kind)  one strong pulse of light along every main line and fine tendril, start to end over FIRE.s seconds: 'send' and 'stop'; 'lock' plays the notice shape's lock (LOCK)
        run outward (U 0 to 1), 'receive' inward; 'stop' is red-tinted. Safe at any time, in any mode; changes nothing else
   KVScene.perf()  {ms, work, level}: frame to frame ms, step + draw ms, sharpness level (0..2), each averaged over the last 90 frames;
        the same numbers go to console.log('KVScene frame ms ...') for logcat
   due: in 'notice', the share of the notice period already run (0 just asked, 1 ready): the shape fills that far and holds;
        null (the tune page) runs the demo loop of fill, hold, fade instead
   mode: 'home' | 'send' (mirrored) | 'stop' (the old collapse to a point)
         'locked'  the lines become nested hexagons, one ring after another from the outside in (HEX.spin turns them as they
                   arrive, 0 = no turning); spokes draw in, the glow gives one soft beat, then still
         'stopped' the lines collapse into red nested triangles, glowing, pulsing slowly
         'sending' | 'receiving'  a small still core with RING.count diamonds around it: sending, each one leaves the core and
                   grows until it fades away; receiving, each one comes in from far and lands in the core; the glow beats as each leaves or lands
         one mode per protection, each a slow morph of the same lines (the *.time dials) and back:
         'limit'   daily limit: the lines bundle through a narrow throat (LIMIT.throat = fraction of the width) and spread
                   again; the pulse that runs out swells before the throat and is squeezed through it
         'notice'  notice period: a closed shape, the main lines laid end to end round it, that burns like a fuse as the time runs (NOTICE.unwind s):
                   a spark at the burn point, embers behind it, the line still ahead; when the time is up it locks solid (LOCK), holds, fades and
                   starts again. A faint track shows the whole outline
         'stops'   emergency stops: STOPS.count calm teal nested triangles
         'coin'    a covenant: a coin (two rims), real covenant code stamped round its band
         'beat'    the heir: the lines lie along one heartbeat line that runs across the top
   lift: 0..1 moves any shape up and smaller by that share (a walk step keeps it clear of the text)
         'heir'    heir after silence: the lines thin and slow with HEIR.silence (0..1) until one carries on alone past a
                   small gap; silence back to 0 (a payment) thickens them again
         'safe'    2-of-3 paper sheets: SEAL.sheets squares fan into a star, slowly close into one square (the glow rises
                   as they meet) and open again, around nested inner squares
   A change of mode is a fade out and in (CHANGE.style 1) or a slow morph (the *.time dials, CHANGE.sweep). dim means a reading screen, where the ribbon sits back; slow means
   the long collapse of a real stop; screen picks that screen's ribbon pulse (PAGES). KVScene.dials = {jelly, change, pages, base, drift, hex, stop, flow, ring, limit, due, stops,
   heir, seal, fire, touch} are the state dials, live, for the tune page docs/mock/scene-states.html (fire: the KVScene.pulse comet; touch: the finger). */
(function () {
  var TAU_ = Math.PI * 2;
  var D = { lines: 5, thick: 3, width: 71, twist: 1.2, calm: 0.7, react: 1.4, weight: 3, bounce: 0.45 };
  /* State dials (design px on a 320 wide phone, seconds, multipliers). The base dials above stay as they are. */
  /* every shape also has: sides; light / lightS = its brightness swings by that share every lightS s; lights = how many lights run round it, lightRun = trips per s, lightSize = px */
  var BASE = {
    pulse: 0,
    pulseS: 8,
    rot: 0,
    drift: 0,
  }; /* every block: pulse = size swing (share), pulseS = its seconds, rot = tilt (degrees), drift = turn speed (degrees per s) */
  var PAGES = {
    home: 0.02,
    homeS: 8,
    send: 0.11,
    sendS: 8,
    receive: 0.085,
    receiveS: 6.25,
    people: 0,
    peopleS: 8,
    settings: 0,
    settingsS: 8,
    homeL: 0.05,
    sendL: 0.16,
    receiveL: 0.37,
    peopleL: 0,
    settingsL: 0,
  }; /* the ribbon's pulse on each app screen: size (share), <name>S = seconds, <name>L = light pulse (share of the brightness) */
  var DRIFT = {
    range: 0.11,
    period: 120,
  }; /* base: the whole ribbon wanders over the full screen (under the glass too), range = fraction of the height */
  var HEX = {
    size: 109,
    x: 0.5,
    y: 0.36,
    rings: 3,
    line: 0.8,
    glow: 23,
    breath: 3.5,
    spin: 80,
    kick: 1.3,
    stagger: 0.3,
    spokes: 0,
    drift: 3.5,
    tint: 0,
    rgb: [214, 238, 255],
    time: 5.2,
    pulse: 0,
    pulseS: 8,
    rot: 0,
    sides: 6,
    light: 0.34,
    lightS: 3.5,
    lights: 10,
    lightRun: 0.05,
    lightSize: 1.1,
  }; /* spin: degrees each ring turns as it arrives (0 = no turning); kick: overshoot; drift: slow turn, degrees per s; tint: how much of rgb */
  var STOP = {
    size: 135,
    x: 0.5,
    y: 0.5,
    depth: 3,
    weight: 1,
    glow: 29,
    period: 2.25,
    pulse: 0.055,
    time: 3,
    rgb: [229, 53, 42],
    rot: 0,
    drift: 0,
    sides: 3,
    light: 0.34,
    lightS: 18.25,
    lights: 0,
    lightRun: 0.05,
    lightSize: 0.3,
  };
  var FLOW = { speed: 0.28, gap: 1.2, width: 0.03, pulse: 0.5, spin: 0.14, time: 1.8 };
  var CHANGE = {
    style: 0,
    fade: 1.4,
    sweep: 0,
    mirror: 0,
  }; /* how one shape becomes the next: style 1 = fade out, then in, over fade s; style 0 = the lines move, sweep = how much they go in turn along their length */
  var RING = {
    size: 151,
    tall: 1.4,
    sides: 4,
    count: 3,
    core: 0.08,
    x: 0.5,
    y: 0.25,
    speed: 0.11,
    line: 1.05,
    glow: 29,
    beat: 11,
    curve: 1.65,
    thin: 0.35,
    birth: 10,
    tail: 1.3,
    coreLight: 1,
    drift: 0,
    tint: 0.1,
    rgb: [214, 238, 255],
    time: 5,
    pulse: 0,
    pulseS: 8,
    rot: 0,
    light: 0.15,
    lightS: 7.5,
    lights: 60,
    lightRun: -0.14,
    lightSize: 0.4,
  }; /* core: its size as a share of size; speed: trips per s; beat: extra glow as one leaves or lands */
  var LIMIT = {
    throat: 0.05,
    at: 0.2,
    span: 0.2,
    swell: 0.25,
    time: 3,
    pulse: 0.005,
    pulseS: 2.75,
    rot: 0,
    drift: 0,
    light: 0.18,
    lightS: 8.5,
  }; /* at/span: where along the ribbon, how long; swell: pulse bulge before the throat */
  var NOTICE = {
    unwind: 19,
    hold: 6,
    size: 65,
    x: 0.5,
    y: 0.23,
    gap: 0,
    line: 0.75,
    glow: 29,
    beat: 30,
    track: 0.24,
    time: 5,
    pulse: 0.145,
    pulseS: 15,
    rot: 45,
    drift: 13.5,
    sides: 4,
    light: 0.05,
    lightS: 11.5,
    lights: 4,
    lightRun: -0.07,
    lightSize: 1.7,
    lap: 1.2,
    spark: 2.6,
    sparks: 5,
    ember: [255, 112, 38],
    emberA: 0.8,
    trail: 0.22,
    flick: 0.35,
    flickS: 9,
  }; /* unwind: s to burn; hold: s locked; gap: share of each side left open at its corners (0 = closed); track: brightness of the outline under the burn.
    The fuse: lap = px the last line runs past the start corner so the two flat ends leave no notch; spark = px of the hot core; sparks = flying bits; ember = rgb behind the spark, emberA its brightness there,
    trail = share of the outline it fades over; flick = flicker depth, flickS = its Hz */
  var STOPS = {
    count: 3,
    size: 88,
    x: 0.5,
    y: 0.21,
    glow: 29,
    period: 10,
    pulse: 0.205,
    time: 3,
    rot: 180,
    drift: 11,
    sides: 3,
    light: 0.23,
    lightS: 14.25,
    lights: 3,
    lightRun: 0.14,
    lightSize: 4,
  };
  var HEIR = {
    silence: 0.75,
    at: 0.9,
    gap: 0.2,
    thin: 0.9,
    line: 1,
    time: 3,
    pulse: 0.02,
    pulseS: 8,
    rot: 0,
    drift: 0,
    light: 0.9,
    lightS: 5.5,
  }; /* line: which one carries on */
  var PRICE = {
    y: 0.47,
    height: 0.24,
    x0: 0.06,
    x1: 0.94,
    line: 1.3,
    glow: 25,
    time: 2,
    light: 0.08,
    lightS: 8,
    lights: 16,
    lightRun: 0.06,
    lightSize: 1.2,
    pulse: 0,
    pulseS: 8,
    rot: 0,
    drift: 0,
    tint: 0,
    rgb: [214, 238, 255],
    sides: 4,
  }; /* the KAS price: y/height = where on the screen, x0..x1 = width; st.series = the normalised prices */
  var ACCENT = {
    amount: 0.75,
    time: 1.2,
  }; /* st.accent = [r,g,b] of the shown account (null = the jelly's own colours); amount = how far the colours go toward it */
  var SEAL = {
    size: 100,
    x: 0.5,
    y: 0.44,
    sheets: 3,
    fan: 45,
    period: 12,
    /* the lock seal animates faster */ line: 1.1,
    glow: 22,
    pulse: 0,
    close: 0.45,
    hold: 0.15,
    inner: 2,
    drift: 0,
    tint: 0,
    rgb: [214, 238, 255],
    time: 5,
    pulseS: 8,
    rot: 0,
    sides: 5,
    light: 0.28,
    lightS: 10,
    lights: 0,
    lightRun: 0.05,
    lightSize: 1.4,
  }; /* fan: degrees between sheets when open; period: s for open-close-open; close: 0 = stays a star; hold: rest at each end; inner: nested squares */
  var LOCK = {
    s: 1.3,
    snap: 0.25,
    pinch: 0.35,
    squeeze: 0.07,
    bar: 1.15,
    barW: 1.5,
  }; /* the lock when the notice period is up: s = the one-shot's seconds, snap = s for the outline to go solid, pinch = share of s the shape contracts (by squeeze), bar = the bolt's half length as a share of size, barW = its width against the line */
  /* The base look (a glowing jellyfish): fine tapering tendrils, a smooth halation, lights travelling down
     them, ends always far off screen. The five main lines still make every shape; the fine tendrils dissolve as one forms. */
  var JELLY = {
    fine: 8,
    line: 0.8,
    taper: 0.1,
    wave: 43,
    curl: 1.3,
    speed: 0.5,
    spread: 0.6,
    halation: 0.6,
    reach: 0.3,
    body: 1.6,
    soft: 0.5,
    beads: 200,
    beadSize: 0.45,
    breath: 0.4,
    core: [112, 199, 186],
    halo: [112, 199, 186],
  };
  /* KVScene.pulse: s = seconds from start to end, width = share of the screen's length (the screen pulse is FLOW.width), bright = its alpha,
     tint = how far its colour goes toward white (for 'stop', from red), size = its width against the screen pulse */
  var FIRE = { s: 1.1, width: 0.04, bright: 1, tint: 0.6, size: 1.3 };
  /* sparks: on the plain ribbon (Home and every screen without a shape) one line at a time
     carries a comet,
     at random: about one every `every` s, each taking `run` s to cross */
  var SPARK = { every: 1.4, run: 1.9, size: 1.1, bright: 0.85 };
  var GLOW_ALL = 0.72; /* every glow lower, twice by 15% (the owner's call: the peaks blew out) */
  /* coin: size px, y; period s for one stamp (press, hold, lift); drop = start height (of H) */
  var COIN = { size: 70, x: 0.5, y: 0.2, period: 3.4, drop: 0.12 };
  /* beat: the heartbeat line; amp px, beats across, speed = beats per s, spread px between lines */
  var BEAT = { y: 0.2, width: 0.92, amp: 40, beats: 2.2, speed: 0.55, spread: 1.4 };
  var ECG = [[0, 0], [0.3, 0], [0.36, 0.12], [0.42, 0], [0.5, 0], [0.53, -0.18], [0.57, 1],
    [0.61, -0.32], [0.65, 0], [0.78, 0], [0.86, 0.24], [0.94, 0], [1, 0]];
  /* the code stamped round the coin's band: three lines of the vault's own covenant
     (contracts/vault.sil), word for word (the owner's call: real code, not a symbol) */
  var CODE_RING = 'require(checkSig(s, spendPk)); require(stage == 0); ' +
    'require(checkSig(s, stopKey)); ';
  /* the finger: the lines lean a little AWAY from it (push, within radius), the lines passing near it brighten (light) and a short
     ring of that light runs outward (ring, speed = laps per s); glow = the soft bloom under it. All of it fades `fade` s after lift */
  var FINGER = { push: 0.25, radius: 1.3, glow: 0.6, light: 0.9, ring: 0.7, speed: 1.1, fade: 0.8 };
  var VSPAN = 1.925; /* the ribbon runs about twice the screen: U 0..1 is its whole length, V 0..1 the stretch that crosses the screen */
  function vOf(u) {
    return 0.09 + (u - 0.3) * VSPAN;
  }
  function uOf(v) {
    return 0.3 + (v - 0.09) / VSPAN;
  }
  function clamp(x) {
    return Math.max(0, Math.min(1, x));
  }
  function rng(n) {
    var v = Math.sin(n * 127.1 + 311.7) * 43758.5453;
    return v - Math.floor(v);
  } /* stable, so nothing reshuffles */
  var FINE = [],
    BEADS = [],
    bloom = [],
    mix = null,
    mixG = null,
    grain = null,
    canFilter = false,
    PAD = 0.15,
    fNo = 0;
  (function () {
    for (var i = 0; i < 40; i++)
      FINE.push({
        d: (rng(i) * 2 - 1) * 1.25,
        w: 0.4 + 0.45 * rng(i + 50),
        a: 0.3 + 0.6 * rng(i + 100),
        f: 0.75 + 0.6 * rng(i + 150),
        p: TAU_ * rng(i + 200),
        q: TAU_ * rng(i + 250),
        r: rng(i + 300),
      });
    for (i = 0; i < 200; i++)
      BEADS.push({
        l: rng(i + 400),
        v: Math.pow(rng(i + 500), 0.7) * 1.3 - 0.15,
        s: 0.012 + 0.02 * rng(i + 600),
        z: 0.5 + 0.8 * rng(i + 700),
        p: TAU_ * rng(i + 800),
        t: 5 + 7 * rng(i + 900),
      });
  })();
  var DESIGN_WIDTH = 320,
    N = 10,
    S = 120,
    TAU = Math.PI * 2,
    RT3 = Math.sqrt(3);
  var c,
    g,
    W = 320,
    H = 676,
    nodes = [],
    scale = 1,
    dpr = 1,
    level = 0,
    fN = 0,
    fT = 0,
    fW = 0,
    lastMs = 0,
    lastWork = 0;
  var st = { left: 100, mode: 'home', k: 0, dim: false, slow: false, due: null };
  var left = 100,
    K = 0,
    red = 0,
    dim = 0,
    side = 1,
    T = 0,
    phase = 0.6,
    spin = 0,
    finger = null,
    last = null,
    prev = 0;
  /* People: the five lines themselves take a Celtic tree-of-life shape, nothing else drawn. size: ring radius; roots/branches: how far
     round the ring the roots (bottom) and branches (top) land, in radians per line; weave: sideways knot amplitude in the trunk; turns: crossings; sway: rad/s of the slow weave */
  /* lock screen K (st.mark 'k'): the seal's fanned rings stay as the ring, and its inner
     lines become our own backwards stencil K inside it: the outline and the key slot,
     inner = the K's half-height against the ring; each traces in over draw s, stagger s apart. */
  var KMARK = { inner: 0.48, draw: 0.6, stagger: 0.15 };
  /* after any shape lets go (the lock K after the fingerprint, a stop, the vault seal), the
     fine tendrils are never there already: they peel out of the main lines, starting `after` s
     once the shape is half gone and taking `grow` s */
  var ARRIVE = { after: 0.3, grow: 2.6 };
  /* outline points in half-heights (y -1 top .. 1 bottom), mirrored so the K faces left */
  var K_OUT = [[0.833, -1], [0.444, -1], [0.444, -0.167], [-0.333, -1], [-0.833, -1],
    [0.167, 0], [-0.833, 1], [-0.333, 1], [0.444, 0.167], [0.444, 1], [0.833, 1]];
  var K_SLOT = [[0.639, -0.611], [0.639, 0.611]];
  var PEOPLE = {
    size: 92,
    x: 0.5,
    y: 0.34,
    sides: 6,
    fan: 14,
    step: 0.07,
    line: 1.05,
    glow: 17,
    time: 4,
    pulse: 0.02,
    pulseS: 9,
    rot: 0,
    drift: 1.2,
    bursts: 7,
    burstRun: 0.09,
    burstSize: 4.5,
    burstS: 2.6,
  }; /* People mark: fan = degrees between the rings, step = each ring smaller by this share,
       drift = slow turn in deg/s; bursts = small lights each running back and forth along one
       ring at its own pace (burstRun trips per s),
       swelling and fading on their own every ~burstS s, burstSize px */
  var pageName = 'home',
    pageL = 0,
    pageA = 0,
    pagePh = 0,
    shown = 'home',
    veil = 0,
    lockT = 0,
    lockAge = 0,
    ringT = 0,
    sealAge = 0,
    stopT = 0,
    limitT = 0,
    noticeT = 0,
    stopsT = 0,
    heirT = 0,
    safeT = 0,
    knotAge = 0,
    flowA = 0,
    flowDir = 1,
    priceT = 0,
    peopleT = 0,
    artT = 0,
    artAge = 0,
    artMode = 'coin',
    liftE = 0,
    sparks = [],
    clock = 0,
    arrAge = 0; /* seconds since the last shape let go (or start): fine tendrils grow from it */
  var mixT = 0; /* weight of "some shape is on"; the per-shape weights only share it out, so one shape flows straight into the next */
  var accA = 0,
    accRGB = [112, 199, 186]; /* the eased account colour */
  var asleep = false; /* KVScene.sleep(true) while the camera is open: no frames, no memory churn */
  var charge = 0,
    lclk = 0,
    fire = null,
    lock = null,
    noteSet = 0,
    touchX = 0,
    touchY = 0,
    touchA = 0,
    touchAge = 0,
    heat = 1,
    wantSnap = false; /* charge: eased st.charge; lclk: the lights' clock (faster while charging); fire: the running KVScene.pulse; lock: the running lock one-shot, noteSet: 0..1 the notice outline is solid; touch*: the eased finger and its weight */

  function size() {
    var d = Math.min(
      devicePixelRatio || 1,
      [2, 1.5, 1.5][level]
    ); /* never below 1.5: that was the "low quality"; level 2 halves the blur work instead */
    W = innerWidth;
    H = innerHeight;
    scale = W / DESIGN_WIDTH;
    c.width = W * d;
    c.height = H * d;
    c.style.width = W + 'px';
    c.style.height = H + 'px';
    g.setTransform(d, 0, 0, d, 0, 0);
    dpr = d;
    bloom = [3, 6, 12, 24].map(function (n) {
      var b = document.createElement('canvas');
      b.width = Math.ceil((W * (1 + 2 * PAD)) / n);
      b.height = Math.ceil((H * (1 + 2 * PAD)) / n);
      return { c: b, g: b.getContext('2d'), n: n };
    });
    canFilter = typeof bloom[0].g.filter === 'string';
    fNo = -1; /* fresh tiers: the next frame blurs every one */
    mix = document.createElement('canvas');
    mix.width = bloom[0].c.width;
    mix.height = bloom[0].c.height;
    mixG = mix.getContext('2d'); /* the three tiers added up once, at 1/3 size */
    /* a still, barely-there grain stops the soft glow banding on dark screens; it never moves, so it reads as smoothness */
    var t = document.createElement('canvas');
    t.width = t.height = 128;
    var tg = t.getContext('2d'),
      im = tg.createImageData(128, 128);
    for (var i = 0; i < im.data.length; i += 4) {
      var v = Math.floor(rng(i * 0.37) * 4);
      im.data[i] = v;
      im.data[i + 1] = v;
      im.data[i + 2] = v;
      im.data[i + 3] = 255;
    }
    tg.putImageData(im, 0, 0); /* laid out once at the screen's device size, so each frame is one blit, not a pattern fill */
    grain = document.createElement('canvas');
    grain.width = c.width;
    grain.height = c.height;
    var gg = grain.getContext('2d');
    gg.fillStyle = gg.createPattern(t, 'repeat');
    gg.fillRect(0, 0, grain.width, grain.height);
  }
  /* The glow: the lines are drawn small, blurred at three widths, the tiers added up in one 1/3-size canvas and that laid once over the sharp frame.
     The two wide tiers change slowly, so they are reblurred only every second frame (level 2: everything half as often). */
  function halate(amount) {
    var r = [4, 5, 6] /* wide enough that no tier's coarse grid shows through as blotches */,
      weight = [0.9, 0.8, 0.9],
      slow = level >= 2 ? 2 : 1;
    for (var n = 1; n < bloom.length; n++) {
      if (fNo % ((n === 1 ? 1 : 2) * slow)) continue;
      var b = bloom[n],
        from = bloom[n - 1].c;
      b.g.globalCompositeOperation = 'copy';
      b.g.filter = canFilter ? 'blur(' + r[n - 1] * JELLY.reach + 'px)' : 'none';
      b.g.drawImage(from, 0, 0, b.c.width, b.c.height);
    }
    var M = mixG;
    M.globalCompositeOperation = 'copy';
    M.globalAlpha = 1;
    M.clearRect(0, 0, mix.width, mix.height);
    M.globalCompositeOperation = 'lighter';
    M.imageSmoothingEnabled = true;
    M.imageSmoothingQuality = 'high';
    for (n = 1; n < bloom.length; n++) {
      M.globalAlpha = Math.min(1, amount * weight[n - 1]);
      M.drawImage(bloom[n].c, 0, 0, mix.width, mix.height);
    }
    M.globalAlpha = 1;
    g.globalCompositeOperation = 'lighter';
    g.imageSmoothingEnabled = true;
    g.imageSmoothingQuality = 'medium';
    g.globalAlpha = 1;
    g.drawImage(mix, -W * PAD, -H * PAD, W * (1 + 2 * PAD), H * (1 + 2 * PAD));
  }
  /* Fills a line as a thin shape, so it can taper; xs is null where the line is cut. */
  function tendril(xs, ys, ws) {
    var P = new Path2D(),
      n = xs.length,
      i = 0;
    while (i < n) {
      if (xs[i] === null) {
        i++;
        continue;
      }
      var a = i;
      while (i + 1 < n && xs[i + 1] !== null) i++;
      var b = i++,
        left = [],
        right = [];
      if (b - a < 1) continue;
      for (var k = a; k <= b; k++) {
        var p = Math.max(a, k - 1),
          q = Math.min(b, k + 1),
          tx = xs[q] - xs[p],
          ty = ys[q] - ys[p],
          l = Math.sqrt(tx * tx + ty * ty) || 1,
          h = ws[k] / 2;
        left.push([xs[k] - (ty / l) * h, ys[k] + (tx / l) * h]);
        right.push([xs[k] + (ty / l) * h, ys[k] - (tx / l) * h]);
      }
      P.moveTo(left[0][0], left[0][1]);
      for (k = 1; k < left.length; k++) P.lineTo(left[k][0], left[k][1]);
      for (k = right.length - 1; k >= 0; k--) P.lineTo(right[k][0], right[k][1]);
      P.closePath();
    }
    return P;
  }
  function rest(u) {
    u = vOf(u); /* same path as before, carried on far past both edges of the screen, so no end ever shows */
    var x = W * (1.12 - 1.3 * u),
      w = (TAU * clock) / Math.max(4, DRIFT.period);
    var dx = W * DRIFT.range * 0.5 * Math.sin(w * 0.73 + 1.1),
      dy = H * DRIFT.range * (0.5 - 0.5 * Math.cos(w));
    return [W / 2 + (x - W / 2) * side + dx, H * (-0.02 + 0.56 * u) + H * 0.05 * Math.sin(u * 3.2 + T) + dy];
  }
  function free(t) {
    return !t.closest || !t.closest('button,a,input,textarea,select,label');
  }
  function bind() {
    addEventListener('pointerdown', function (e) {
      if (free(e.target)) {
        finger = [e.clientX, e.clientY];
        last = finger;
        if (touchA <= 0) {
          touchX = finger[0];
          touchY = finger[1];
        }
        touchAge = 0;
      }
    });
    addEventListener('pointermove', function (e) {
      if (!finger) return;
      var p = [e.clientX, e.clientY];
      spin += ((p[0] - last[0] - (p[1] - last[1])) * 0.012 * D.react) / scale;
      last = p;
      finger = p;
    });
    ['pointerup', 'pointercancel'].forEach(function (n) {
      addEventListener(n, function () {
        finger = null;
      });
    });
  }
  function toward(v, t, m) {
    return v + Math.max(-m, Math.min(m, t - v));
  }
  function ease(t) {
    return t * t * (3 - 2 * t);
  }
  function stepEffects(dt) {
    clock += dt;
    arrAge = mixT > 0.5 ? 0 : arrAge + dt;
    charge += (clamp(+st.charge || 0) - charge) * (1 - Math.exp(-dt * 4)); /* about 0.25 s */
    lclk += dt * (1 + 2 * charge);
    heat += ((typeof st.heat === 'number' ? clamp(st.heat) : 1) - heat) * (1 - Math.exp(-dt * 6));
    if (st.accent && st.accent.length === 3) {
      if (accA < 0.001) accRGB = st.accent.slice();
      else for (var ci = 0; ci < 3; ci++) accRGB[ci] += (st.accent[ci] - accRGB[ci]) * (1 - Math.exp(-dt * 2));
      accA = toward(accA, 1, dt / ACCENT.time);
    } else accA = toward(accA, 0, dt / ACCENT.time);
    if (fire) {
      fire.age += dt;
      if (fire.age >= Math.max(0.2, FIRE.s)) fire = null;
    }
    if (lock) {
      lock.age += dt;
      if (lock.age >= Math.max(0.3, LOCK.s)) lock = null;
    }
    if (finger) {
      var fk = 1 - Math.exp(-dt * 14);
      touchX += (finger[0] - touchX) * fk;
      touchY += (finger[1] - touchY) * fk;
      touchA = toward(touchA, 1, dt / 0.12);
    } else touchA = toward(touchA, 0, dt / Math.max(0.1, FINGER.fade));
    touchAge += dt;
  }
  function stepShapes(m, dt, td, flowing) {
    lockT = toward(lockT, m === 'locked' ? 1 : 0, td / HEX.time);
    lockAge = m === 'locked' ? Math.min(lockAge + dt, HEX.time * 3) : Math.max(0, lockAge - dt * 2); /* leaving: the rings turn back out */
    ringT = toward(ringT, m === 'sending' || m === 'receiving' ? 1 : 0, td / RING.time);
    stopT = toward(stopT, m === 'stopped' ? 1 : 0, td / STOP.time);
    limitT = toward(limitT, m === 'limit' ? 1 : 0, td / LIMIT.time);
    noticeT = toward(noticeT, m === 'notice' ? 1 : 0, td / NOTICE.time);
    priceT = toward(priceT, m === 'price' ? 1 : 0, td / PRICE.time);
    peopleT = toward(peopleT, m === 'people' ? 1 : 0, td / PEOPLE.time);
    var art = m === 'coin' || m === 'beat';
    if (art) artMode = m;
    artT = toward(artT, art ? 1 : 0, td / 1.7);
    artAge = art ? artAge + dt : 0;
    liftE = toward(liftE, clamp(+st.lift || 0), td / 1.7);
    stepSparks(dt, m);
    var shapeTime =
      {
        locked: HEX.time,
        stopped: STOP.time,
        notice: NOTICE.time,
        stops: STOPS.time,
        safe: SEAL.time,
        sending: RING.time,
        receiving: RING.time,
        price: PRICE.time,
        people: PEOPLE.time,
      }[m] || 1.7;
    var shapeOn =
      m === 'locked' ||
      m === 'stopped' ||
      m === 'stops' ||
      m === 'safe' ||
      m === 'sending' ||
      m === 'receiving' ||
      m === 'notice' ||
      m === 'price' ||
      m === 'people' ||
      art
        ? 1
        : 0;
    mixT = toward(mixT, shapeOn, td / shapeTime);
    stopsT = toward(stopsT, m === 'stops' ? 1 : 0, td / STOPS.time);
    heirT = toward(heirT, m === 'heir' ? 1 : 0, td / HEIR.time);
    safeT = toward(safeT, m === 'safe' ? 1 : 0, td / SEAL.time);
    sealAge = m === 'safe' ? sealAge + dt : safeT > 0 ? sealAge : 0;
    if (m === 'notice' && typeof st.due !== 'number') {
      var cyc = NOTICE.unwind + NOTICE.hold + 1.5,
        a0 = knotAge % cyc;
      if (a0 < NOTICE.unwind && a0 + dt >= NOTICE.unwind) lock = { age: 0 };
    } /* the demo cycle locks itself when the fuse runs out */
    knotAge = m === 'notice' ? knotAge + dt : 0;
    noteSet = toward(noteSet, noteDone() ? 1 : 0, td / Math.max(0.05, LOCK.snap));
    flowA = toward(flowA, flowing ? 1 : 0, td / FLOW.time);
    if (flowing) flowDir = m === 'receiving' ? -1 : 1;
  }
  function stepNodes(dt) {
    var stiff = 14 / D.weight,
      damp = 2 * Math.sqrt(stiff) * (1 - D.bounce * 0.85),
      cx = 0.16 * W,
      cy = 0.47 * H;
    nodes.forEach(function (n, i) {
      var r = rest(i / (N - 1)),
        tx = r[0] + (cx - r[0]) * K,
        ty = r[1] + (cy - r[1]) * K,
        fx = (tx - n.x) * stiff,
        fy = (ty - n.y) * stiff;
      if (finger && K < 0.5) {
        var dx = finger[0] - n.x,
          dy = finger[1] - n.y,
          f = (Math.exp(-(dx * dx + dy * dy) / (W * W * 0.22 * FINGER.radius * FINGER.radius)) * D.react * 26 * FINGER.push) / D.weight;
        fx -= dx * f;
        fy -= dy * f; /* away from the finger, a little */
      }
      n.vx += (fx - n.vx * damp) * dt;
      n.vy += (fy - n.vy * damp) * dt;
      n.x += n.vx * dt;
      n.y += n.vy * dt;
    });
    for (var a = 1; a < N - 1; a++) {
      var A = nodes[a - 1],
        B = nodes[a],
        C = nodes[a + 1];
      B.vx += ((A.x + C.x) / 2 - B.x) * dt * 30;
      B.vy += ((A.y + C.y) / 2 - B.y) * dt * 30;
    }
  }
  function step(dt) {
    /* style 1: the art fades to the ground, the new shape is put in place unseen, the art fades back. 'stop' and 'send' are the app's own gestures and never wait. */
    var want = st.mode,
      hop = CHANGE.style >= 0.5 && want !== 'stop' && want !== 'send' && shown !== 'stop' && shown !== 'send',
      snap = false,
      fadeR = (dt * 2) / Math.max(0.1, CHANGE.fade);
    var formed = wantSnap;
    wantSnap = false;
    if (formed) {
      shown = want;
      snap = true;
      veil = 1;
    } /* put in place already formed: the unlock screen opens on the seated hexagons */ else if (want !== shown) {
      if (hop) {
        veil = toward(veil, 0, fadeR);
        if (veil <= 0) {
          shown = want;
          snap = true;
        }
      } else shown = want;
    } else veil = toward(veil, 1, fadeR);
    var td = snap ? 1e6 : dt;
    if (snap) lockAge = formed ? HEX.time * 3 : 0;
    var m = shown,
      flowing = m === 'sending' || m === 'receiving' || m === 'limit';
    var toSide = CHANGE.mirror >= 0.5 && (m === 'send' || m === 'sending') ? -1 : 1,
      collapse = st.slow ? 1.1 : 2.2;
    stepEffects(dt);
    stepShapes(m, dt, td, flowing);
    if (PAGES[st.screen] !== undefined) pageName = st.screen; /* inner screens keep the pulse of the tab they came from */
    pageL += (PAGES[pageName + 'L'] - pageL) * (1 - Math.exp(-dt * 2));
    pageA += (PAGES[pageName] - pageA) * (1 - Math.exp(-dt * 2));
    pagePh += (dt * TAU) / Math.max(0.5, PAGES[pageName + 'S']);
    var calm = D.calm * (1 - 0.6 * Math.max(0, Math.min(1, HEIR.silence)) * ease(heirT)); /* heir: the lines slow with the silence */
    T += dt * 0.25 * calm;
    phase += dt * 0.12 * calm + spin * dt - flowDir * FLOW.spin * flowA * dt;
    spin *= Math.exp(-dt * 1.3);
    side += (toSide - side) * (1 - Math.exp(-dt * 2.4));
    left += (st.left - left) * (1 - Math.exp(-dt * 3));
    K += (st.k - K) * (1 - Math.exp(-dt * collapse));
    red += ((st.mode === 'stop' ? 1 : 0) - red) * (1 - Math.exp(-dt * 5));
    dim += ((st.dim ? 1 : 0) - dim) * (1 - Math.exp(-dt * 4));
    stepNodes(dt);
  }
  function curve(u) {
    var f = u * (N - 1),
      i = Math.min(N - 2, Math.floor(f)),
      s = f - i,
      s2 = s * s,
      s3 = s2 * s;
    var P0 = nodes[Math.max(0, i - 1)],
      P1 = nodes[i],
      P2 = nodes[i + 1],
      P3 = nodes[Math.min(N - 1, i + 2)];
    return [
      0.5 * (2 * P1.x + (P2.x - P0.x) * s + (2 * P0.x - 5 * P1.x + 4 * P2.x - P3.x) * s2 + (3 * P1.x - P0.x - 3 * P2.x + P3.x) * s3),
      0.5 * (2 * P1.y + (P2.y - P0.y) * s + (2 * P0.y - 5 * P1.y + 4 * P2.y - P3.y) * s2 + (3 * P1.y - P0.y - 3 * P2.y + P3.y) * s3),
    ];
  }
  /* a shape's pulse and rotation, the same for all of them: swings its size and turns it round its centre */
  function spun(p, O, cx, cy) {
    var a = ((O.rot + clock * O.drift) * Math.PI) / 180,
      z = 1 + O.pulse * Math.sin((TAU * clock) / Math.max(0.5, O.pulseS || O.period)),
      dx = (p[0] - cx) * z,
      dy = (p[1] - cy) * z,
      c = Math.cos(a),
      n = Math.sin(a);
    return [cx + dx * c - dy * n, cy + dx * n + dy * c];
  }
  /* one point of a closed n-sided shape, U 0..1 once round; S lands exactly on the corners for n = 3, 4, 6 */
  function ngon(r, n, rot, U, cx, cy) {
    var v = (U * n + 0.5) % n,
      e = Math.floor(v),
      f = v - e,
      a0 = rot + (TAU * e) / n,
      a1 = a0 + TAU / n;
    return [cx + r * (Math.cos(a0) + (Math.cos(a1) - Math.cos(a0)) * f), cy + r * (Math.sin(a0) + (Math.sin(a1) - Math.sin(a0)) * f)];
  }
  /* locked: line j is the j-th of HEX.rings nested hexagons, arriving one after another from the outside in. */
  function hexPt(j, U) {
    var n = Math.max(2, Math.min(D.lines, Math.round(HEX.rings))),
      i = Math.min(j, n - 1),
      T = HEX.time;
    var t = clamp((lockAge - T * (hexStart() + HEX.stagger * i)) / (T * 0.32)),
      q = t - 1,
      o = HEX.kick,
      seat = t >= 1 ? 1 : o > 0 ? 1 + (o + 1) * q * q * q + o * q * q : ease(t);
    return spun(
      ngon(
        HEX.size * scale * (1 - i / n),
        Math.max(3, Math.min(12, Math.round(HEX.sides))),
        -Math.PI / 2 + ((i & 1 ? 1 : -1) * (1 - seat) * HEX.spin * Math.PI) / 180,
        U,
        W * HEX.x,
        H * HEX.y
      ),
      HEX,
      W * HEX.x,
      H * HEX.y
    );
  }
  function hexStart() {
    return CHANGE.style >= 0.5 ? 0 : 0.4;
  } /* after a fade nothing has to wait for the morph */
  /* the price: the first main line follows st.series across the screen (smoothstep between points, so it curves, not kinks) */
  function pricePt(U) {
    var s = st.series,
      v = 0.5;
    if (s && s.length > 1) {
      var f = clamp(U) * (s.length - 1),
        i0 = Math.min(s.length - 2, Math.floor(f)),
        ff = f - i0;
      ff = ff * ff * (3 - 2 * ff);
      v = s[i0] + (s[i0 + 1] - s[i0]) * ff;
    }
    var cx = W * 0.5,
      cy = H * PRICE.y;
    return spun([W * (PRICE.x0 + (PRICE.x1 - PRICE.x0) * U), H * (PRICE.y - PRICE.height / 2 + PRICE.height * (1 - v))], PRICE, cx, cy);
  }
  /* the stop colour as a stop slider heats: green, yellow, orange, then STOP.rgb (red) */
  function heatRGB(h) {
    var K = [[70, 205, 120], [235, 210, 70], [242, 140, 48], STOP.rgb],
      s = clamp(h) * 3,
      i = Math.min(2, Math.floor(s)),
      f = s - i;
    return [0, 1, 2].map(function (n) {
      return K[i][n] + (K[i + 1][n] - K[i][n]) * f;
    });
  }
  /* stopped / stops: line j becomes the j-th of `depth` nested upright triangles
     (extra lines sit on the innermost).
     O is STOP (red, the collapse) or STOPS (teal, calm, depth = count). */
  function triPt(j, U, O, depth) {
    depth = Math.max(1, Math.min(D.lines, Math.round(depth)));
    var i = Math.min(j, depth - 1);
    var r = O.size * scale * (1 - i / depth),
      cx = W * O.x,
      cy = H * O.y;
    var ns = Math.max(3, Math.min(12, Math.round(O.sides)));
    return spun(ngon(r, ns, -Math.PI / 2, U, cx, cy), O, cx, cy);
  }
  /* notice: the main lines lie end to end round one closed shape (5 lines on 4 sides: the junctions sit mid-side but one, at the start corner).
     noteAge runs a cycle of burn, hold, fade; noteFill says how far along line j the fuse has burnt; noteDone: the time is up */
  function noteAge() {
    return typeof st.due === 'number' ? clamp(st.due) * NOTICE.unwind : knotAge % (NOTICE.unwind + NOTICE.hold + 1.5);
  }
  function noteFill(j) {
    return clamp((noteAge() / Math.max(0.5, NOTICE.unwind)) * D.lines - j);
  }
  function noteLife() {
    return 1 - clamp((noteAge() - NOTICE.unwind - NOTICE.hold) / 1.5);
  }
  function noteDone() {
    return typeof st.due === 'number' ? st.due >= 0.999 : knotAge % (NOTICE.unwind + NOTICE.hold + 1.5) >= NOTICE.unwind;
  }
  function lockZ() {
    return lock ? 1 - LOCK.squeeze * Math.sin(Math.PI * clamp(lock.age / Math.max(0.05, LOCK.s * LOCK.pinch))) : 1;
  } /* the lock: one contraction */
  function notePt(j, U, raw) {
    var R = NOTICE.size * scale * lockZ(),
      cx = W * NOTICE.x,
      cy = H * NOTICE.y,
      n = Math.max(3, Math.min(12, Math.round(NOTICE.sides))),
      f = NOTICE.gap + U * (1 - 2 * NOTICE.gap);
    if (!raw && j === D.lines - 1 && U === 1)
      f +=
        (NOTICE.lap * scale * D.lines) /
        (n *
          2 *
          R *
          Math.sin(
            Math.PI / n
          )); /* the last line runs NOTICE.lap px past the start corner: two flat ends meeting there left a notch */
    return spun(ngon(R, n, -Math.PI / 2, (j + f) / D.lines - 0.5 / n + 1, cx, cy), NOTICE, cx, cy);
  }
  /* safe: the first SEAL.sheets lines are squares fanned around one centre; 0.5 + 0.5 cos with the top and bottom clipped
     holds them open, then closed, with a slow move between. The other lines nest inside, each the last one's inscribed square. */
  function sealOpen() {
    return ease(clamp(0.5 + (0.5 + SEAL.hold) * Math.cos((TAU * sealAge) / SEAL.period)));
  }
  function sealN() {
    return Math.max(2, Math.min(3, Math.round(SEAL.sheets)));
  }
  function sealLife(j) {
    return j < sealN() || j - sealN() < SEAL.inner ? 1 : 0;
  }
  /* the point at share U (0..1) of the way round a closed polyline, by length */
  function polyAt(P, U) {
    var L = 0,
      i,
      n = P.length;
    for (i = 0; i < n; i++) {
      var nx = P[(i + 1) % n];
      L += Math.hypot(nx[0] - P[i][0], nx[1] - P[i][1]);
    }
    var want = clamp(U) * L;
    for (i = 0; i < n; i++) {
      var a = P[i],
        b = P[(i + 1) % n],
        d = Math.hypot(b[0] - a[0], b[1] - a[1]);
      if (want <= d || i === n - 1) {
        var f = d > 0 ? Math.min(1, want / d) : 0;
        return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
      }
      want -= d;
    }
    return P[0];
  }
  /* the K inside the ring: the first inner line is its outline, the rest the key slot; each
     traces in from sealAge 0 and the undrawn rest waits at the pen's tip */
  function kPt(j, U) {
    var k = j - sealN(),
      R = SEAL.size * scale * KMARK.inner,
      cx = W * SEAL.x,
      cy = H * SEAL.y,
      f = ease(clamp((sealAge - KMARK.stagger * k) / Math.max(0.05, KMARK.draw))),
      p = polyAt(k ? K_SLOT : K_OUT, Math.min(U, f));
    return spun([cx + p[0] * R, cy + p[1] * R], SEAL, cx, cy);
  }
  function sealPt(j, U) {
    if (st.mark === 'k' && j >= sealN()) return kPt(j, U);
    var n = sealN(),
      R = SEAL.size * scale,
      cx = W * SEAL.x,
      cy = H * SEAL.y;
    var ns = Math.max(3, Math.min(12, Math.round(SEAL.sides)));
    if (j < n)
      return spun(
        ngon(
          R,
          ns,
          -Math.PI / 2 + Math.PI / ns + (((j - (n - 1) / 2) * SEAL.fan * Math.PI) / 180) * (1 - SEAL.close * (1 - sealOpen())),
          U,
          cx,
          cy
        ),
        SEAL,
        cx,
        cy
      );
    var k = j - n + 1;
    return spun(
      ngon(R * Math.pow(Math.cos(Math.PI / ns), k), ns, -Math.PI / 2 + Math.PI / ns + (k * Math.PI) / ns, U, cx, cy),
      SEAL,
      cx,
      cy
    );
  }
  /* People: one mark in the scene's own style, a cousin of the lock seal: the lines are six-sided
     rings fanned a little apart, each a step smaller, turning slowly together. Nothing else is drawn on that screen. */
  function peoplePt(j, U) {
    var R = PEOPLE.size * scale,
      cx = W * PEOPLE.x,
      cy = H * PEOPLE.y,
      n = D.lines,
      ns = Math.max(3, Math.min(12, Math.round(PEOPLE.sides)));
    return spun(
      ngon(R * (1 - PEOPLE.step * j), ns, -Math.PI / 2 + Math.PI / ns + ((j - (n - 1) / 2) * PEOPLE.fan * Math.PI) / 180, U, cx, cy),
      PEOPLE,
      cx,
      cy
    );
  }
  /* People: each burst rides one ring back and forth (a ping-pong of its own speed and start) and
     swells to full light and away on its own beat, so no two move or flare together */
  function peopleBursts(lines, e) {
    var out = [],
      n = Math.min(24, Math.round(PEOPLE.bursts)),
      rad = PEOPLE.burstSize * scale;
    if (e < 0.02) return out;
    for (var q = 0; q < n; q++) {
      var L = lines[q % D.lines],
        a = (q * 0.618) % 1,
        b = (q * 0.414) % 1,
        f = (lclk * PEOPLE.burstRun * (0.6 + 0.8 * a) + 2 * b) % 2,
        id = Math.floor((f < 1 ? f : 2 - f) * (S - 1)),
        lx = L.xs[id];
      if (L.a < 0.1 || lx === null || lx === undefined) continue;
      var beat = (TAU * lclk) / (PEOPLE.burstS * (0.7 + 0.6 * b)) + q * 1.7,
        flare = Math.pow(0.5 + 0.5 * Math.sin(beat), 3),
        P = new Path2D();
      P.arc(lx, L.ys[id], rad * (0.6 + 0.6 * flare), 0, TAU);
      out.push({ P: P, a: e * flare });
    }
    return out;
  }
  /* lift: a shape point moved up and made smaller around the screen centre; M = its mix weight */
  function liftPt(px, py, M) {
    var z = 1 - liftE;
    return [(W / 2 + (px - W / 2) * z) * M, (H / 2 + (py - H / 2) * z - H * liftE) * M];
  }
  /* sparks: one comet per main line at most; none on a stopped account */
  function stepSparks(dt, m) {
    for (var j = 0; j < D.lines; j++) {
      var s = sparks[j];
      if (s) sparks[j] = (s.age += dt) > SPARK.run ? null : s;
      else if (m !== 'stopped' && Math.random() < dt / Math.max(0.2, SPARK.every * D.lines))
        sparks[j] = { age: 0 };
    }
  }
  function artPt(j, U) {
    return artMode === 'beat' ? beatPt(j, U) : coinPt(j, U);
  }
  /* the heartbeat: x across the screen, y the ECG trace running left; the lines lie a hair apart */
  function beatPt(j, U) {
    var t = U * BEAT.beats + artAge * BEAT.speed,
      f = t - Math.floor(t),
      i = 1;
    while (i < ECG.length - 1 && ECG[i][0] < f) i++;
    var a = ECG[i - 1],
      b = ECG[i],
      v = a[1] + ((b[1] - a[1]) * (f - a[0])) / Math.max(1e-6, b[0] - a[0]);
    var x = W * (0.5 + (U - 0.5) * BEAT.width);
    return [x, H * BEAT.y - v * BEAT.amp * scale + (j - (D.lines - 1) / 2) * BEAT.spread * scale];
  }
  /* the stamp's height through one period: down, held, lifted away (1 = up, 0 = on the coin) */
  function coinUp() {
    var f = (artAge % COIN.period) / COIN.period;
    return f < 0.35 ? 1 - ease(f / 0.35) : f < 0.75 ? 0 : ease((f - 0.75) / 0.25);
  }
  /* the coin: lines 0 and 1 its two rims, the rest the stamp's ring pressing down with the code */
  function coinPt(j, U) {
    var R = COIN.size * scale,
      cx = W * COIN.x,
      cy = H * COIN.y;
    if (j < 2) return ngon(R * (j ? 0.82 : 1), 36, -Math.PI / 2, U, cx, cy);
    var up = coinUp();
    return ngon(R * 0.64 * (1 + 0.4 * up), 36, -Math.PI / 2, U, cx, cy - up * COIN.drop * H);
  }
  /* the code itself, letter by letter round the band between the rims; it presses down with the
     stamp and lands crisp (drawn over the lines, in the ribbon's teal, a soft glow) */
  function coinCode(g) {
    var a = artMode === 'coin' ? ease(artT) : 0;
    if (a < 0.02) return;
    var up = coinUp(),
      z = 1 - liftE,
      c = liftPt(W * COIN.x, H * COIN.y - up * COIN.drop * H, 1),
      r = COIN.size * scale * 0.91 * (1 + 0.4 * up) * z,
      n = CODE_RING.length,
      fs = Math.min(10 * scale, (TAU * r) / n / 0.6);
    g.save();
    g.globalCompositeOperation = 'lighter';
    g.font = '500 ' + fs.toFixed(2) + 'px ui-monospace, Menlo, Consolas, monospace';
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    g.fillStyle = 'rgba(142,232,218,' + (a * (1 - 0.55 * up)).toFixed(3) + ')';
    g.shadowColor = 'rgba(142,232,218,' + (0.8 * GLOW_ALL).toFixed(3) + ')';
    g.shadowBlur = 5 * scale;
    for (var i = 0; i < n; i++) {
      var t = -Math.PI / 2 + (TAU * i) / n;
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.translate(c[0] + Math.cos(t) * r, c[1] + Math.sin(t) * r);
      g.rotate(t + Math.PI / 2);
      g.fillText(CODE_RING.charAt(i), 0, 0);
    }
    g.restore();
  }
  /* sending / receiving: the last line is the still core, the first RING.count lines are the moving shapes, and any left over
     hide in the core. t runs 0 at the core to 1 far out, forward when sending, backward when receiving, so the same
     envelope reads as leaving or as arriving: quick out of the core, slowly gone at the far end, finer the further it is. */
  function ringN() {
    return Math.max(1, Math.min(D.lines - 1, Math.round(RING.count)));
  }
  function ringRun(j) {
    var t = (clock * RING.speed + j / ringN()) % 1;
    return flowDir > 0 ? t : 1 - t;
  }
  function ringLife(j) {
    if (j === D.lines - 1) return RING.coreLight;
    if (j >= ringN()) return 0;
    var t = ringRun(j);
    return Math.min(1, t * RING.birth) * Math.pow(1 - t, RING.tail);
  }
  function ringFine(j) {
    return j >= ringN() ? 1 : 1 - RING.thin * ringRun(j);
  }
  function ringPt(j, U) {
    var c = clamp(RING.core),
      t = j >= ringN() ? 0 : Math.pow(ringRun(j), RING.curve),
      r = RING.size * scale * (c + (1 - c) * t),
      cy = H * RING.y;
    var p = ngon(r, Math.max(3, Math.min(12, Math.round(RING.sides))), -Math.PI / 2, U, W * RING.x, cy);
    return spun([p[0], cy + (p[1] - cy) * RING.tall], RING, W * RING.x, cy);
  }
  /* the travelling pulse: a comet on one line, sampled between the points so it glides, its width and light both tapering
     to nothing at each end. Long soft tail behind, short rounded head. */
  function comet(xs, ys, base, pp, w, tint, dir, head) {
    dir = dir || flowDir;
    head = head || '255,255,255';
    var from = pp - dir * w * 6,
      to = pp + dir * w,
      cx = [],
      cy = [],
      cw = [],
      n = 28,
      i;
    for (i = 0; i <= n; i++) {
      var s = i / n,
        f = clamp(from + (to - from) * s) * S,
        a = Math.min(S - 1, Math.floor(f)),
        r = f - a;
      if (xs[a] === null || xs[a + 1] === null) continue;
      cx.push(xs[a] + (xs[a + 1] - xs[a]) * r);
      cy.push(ys[a] + (ys[a + 1] - ys[a]) * r);
      cw.push(base * (s < 0.82 ? Math.pow(s / 0.82, 1.6) : Math.sqrt(Math.max(0, 1 - Math.pow((s - 0.82) / 0.18, 2)))));
    }
    if (cx.length < 4) return null;
    var G = g.createLinearGradient(cx[0], cy[0], cx[cx.length - 1], cy[cy.length - 1]);
    G.addColorStop(0, 'rgba(' + tint + ',0)');
    G.addColorStop(0.55, 'rgba(' + tint + ',0.3)');
    G.addColorStop(0.85, 'rgba(' + tint + ',1)');
    G.addColorStop(1, 'rgba(' + head + ',1)');
    return { P: tendril(cx, cy, cw), G: G };
  }
  function draw() {
    g.fillStyle = '#06070c';
    g.fillRect(0, 0, W, H);
    var pts = [],
      norms = [],
      i,
      j;
    for (i = 0; i <= S; i++) {
      var u = i / S,
        p = curve(u),
        a = curve(Math.max(0, u - 0.01)),
        b = curve(Math.min(1, u + 0.01)),
        tx = b[0] - a[0],
        ty = b[1] - a[1],
        l = Math.sqrt(tx * tx + ty * ty) || 1;
      pts.push(p);
      norms.push([-ty / l, tx / l]);
    }
    var eL = ease(lockT),
      eS = ease(stopT),
      eN = ease(noticeT),
      eT = ease(stopsT),
      eB = ease(safeT),
      eC = ease(ringT),
      eP = ease(priceT),
      eK = ease(peopleT),
      eA = ease(artT),
      m = eL + eS + eN + eT + eB + eC + eP + eK + eA;
    var mm = ease(mixT);
    if (m > 0) {
      var kk = mm / m;
      eL *= kk;
      eS *= kk;
      eN *= kk;
      eT *= kk;
      eB *= kk;
      eC *= kk;
      eP *= kk;
      eK *= kk;
      eA *= kk;
      m = mm;
    } else {
      m = 0;
    } /* shares of one mix weight: shape to shape is a direct cross-fade */
    var eLim = ease(limitT),
      eH = ease(heirT) * Math.max(0, Math.min(1, HEIR.silence)),
      pinch = 1 - Math.max(0.02, Math.min(1, LIMIT.throat));
    var red2 = Math.max(red, eS),
      open = sealOpen(),
      eHr = ease(heirT),
      stopC = heatRGB(heat);
    function ribMix(f) {
      var a = f(BASE);
      return a + (f(LIMIT) - a) * eLim + (f(HEIR) - a) * eHr;
    }
    var ribA =
        (ribMix(function (O) {
          return O.rot + clock * O.drift;
        }) *
          Math.PI) /
        180,
      ribZ =
        1 +
        pageA * Math.sin(pagePh) +
        ribMix(function (O) {
          return O.pulse * Math.sin((TAU * clock) / Math.max(0.5, O.pulseS));
        });
    var ribC = Math.cos(ribA),
      ribS = Math.sin(ribA),
      ribX = W * 0.5,
      ribY = H * 0.3,
      ribOn = ribA !== 0 || ribZ !== 1;
    var edge = uOf(Math.max(0.02, Math.min(0.98, 1 - left / 100))),
      eb = 0.09 / VSPAN,
      bright = 0.95 - 0.6 * dim,
      faint = 0.14 - 0.06 * dim;
    faint += (bright - faint) * m;
    function dye(base) {
      return base.map(function (v, n) {
        v += (stopC[n] - v) * red2;
        [
          [HEX, eL],
          [SEAL, eB],
          [RING, eC],
        ].forEach(function (o) {
          v += (o[0].rgb[n] - v) * clamp(o[0].tint) * o[1];
        });
        return v;
      });
    }
    function accent(base) {
      return accA > 0.001
        ? base.map(function (v, n) {
            return v + (accRGB[n] - v) * ACCENT.amount * accA;
          })
        : base;
    }
    var rgb = dye(accent(JELLY.core)),
      halo = dye(accent(JELLY.halo));
    var cyc = (clock * FLOW.speed) % (1 + FLOW.gap),
      pp = flowDir > 0 ? cyc : 1 - cyc,
      amp = cyc <= 1 ? FLOW.pulse * flowA * (1 - eC) : 0,
      w = FLOW.width;
    pp = uOf(pp);
    w /= VSPAN;
    function pulseAt(u) {
      var d = (u - pp) / (w * 3);
      return amp * Math.exp(-d * d);
    } /* the swell before the limit's throat: smooth, wider than the comet */
    function col(u, rgb) {
      var a = u < edge - eb ? faint : u > edge + eb ? bright : faint + ((bright - faint) * (u - edge + eb)) / (2 * eb);
      return 'rgba(' + Math.round(rgb[0]) + ',' + Math.round(rgb[1]) + ',' + Math.round(rgb[2]) + ',' + Math.min(1, a) + ')';
    }
    /* a stopped account pulls the ribbon's two ends into one point; a gradient of no length
       paints nothing, so after some seconds the red triangles vanished. The ramp's ends are
       kept at least 1 px apart. */
    var rx0 = pts[0][0],
      ry0 = pts[0][1],
      rx1 = pts[S][0],
      ry1 = pts[S][1];
    if (Math.abs(rx1 - rx0) + Math.abs(ry1 - ry0) < 1) rx1 = rx0 - 1;
    function ramp(base) {
      var G = g.createLinearGradient(rx0, ry0, rx1, ry1);
      [0, edge - eb, edge + eb, 1]
        .map(clamp)
        .sort(function (a, b) {
          return a - b;
        })
        .forEach(function (u) {
          G.addColorStop(u, col(u, base));
        });
      return G;
    }
    var gr = ramp(rgb),
      grH = ramp(halo);
    var sat = lockAge - HEX.time * (hexStart() + HEX.stagger * (Math.max(2, Math.min(D.lines, Math.round(HEX.rings))) - 1) + 0.32),
      grow = eL > 0 ? ease(clamp(sat / (HEX.time * 0.4))) : 0,
      beat = shown === 'locked' && sat > 0 ? Math.exp(-sat * 1.5) : 0;
    var noteBeat = lock ? Math.exp(-lock.age * 2) : 0; /* the lock's flash */
    var bx = (clock * RING.speed * ringN()) % 1,
      ringBeat = RING.beat * (bx < 0.08 ? bx / 0.08 : Math.exp(-(bx - 0.08) * 6)); /* soft rise, slow fall */
    var glow =
      HEX.glow * eL * (0.7 + 0.3 * Math.sin((TAU * clock) / HEX.breath) + beat) +
      SEAL.glow * eB * (1.15 - open) +
      RING.glow * eC +
      ringBeat * eC +
      NOTICE.glow * eN * noteLife() +
      NOTICE.beat * eN * noteBeat +
      STOP.glow * eS * (0.65 + 0.35 * Math.sin((TAU * clock) / STOP.period)) +
      STOPS.glow * eT * (0.75 + 0.25 * Math.sin((TAU * clock) / STOPS.period)) +
      PRICE.glow * eP +
      PEOPLE.glow * eK;
    var thick =
        D.thick *
        (1 +
          (HEX.line - 1) * eL +
          (STOP.weight - 1) * eS +
          (SEAL.line - 1) * eB +
          (RING.line - 1) * eC +
          (NOTICE.line - 1) * eN +
          (PRICE.line - 1) * eP +
          (PEOPLE.line - 1) * eK),
      wide = D.width * scale * (1 - K);
    var tint = [0, 1, 2]
        .map(function (n) {
          return Math.round(rgb[n] + (255 - rgb[n]) * 0.6);
        })
        .join(','),
      lines = [],
      pulses = [],
      count = D.lines + Math.round(JELLY.fine),
      sp = JELLY.speed;
    /* KVScene.pulse: one comet per line, head from U 0 to past 1 (or back for 'receive'), the tail 6 widths behind, so it has left when fire ends */
    var fires = [],
      fireTint = null,
      fireHead = null,
      fw = FIRE.width / VSPAN,
      fpp = 0;
    if (fire) {
      var fb = fire.stop ? stopC : rgb,
        whiten = function (k) {
          return [0, 1, 2]
            .map(function (n) {
              return Math.round(fb[n] + (255 - fb[n]) * k);
            })
            .join(',');
        };
      fireTint = whiten(clamp(FIRE.tint));
      fireHead = fire.stop ? whiten(0.8) : '255,255,255';
      fpp = (fire.age / Math.max(0.2, FIRE.s)) * (1 + 7 * fw);
    }
    var swayR = W * W * 0.05 * FINGER.radius * FINGER.radius,
      swayK = touchA * FINGER.push * 0.5 * (1 - m);
    g.lineCap = 'round';
    g.lineJoin = 'round';
    var arr = ease(
      clamp((arrAge - ARRIVE.after) / Math.max(0.1, ARRIVE.grow))
    ); /* fine tendrils arrive out of the main lines on every arrival, never there first */
    for (j = 0; j < count; j++) {
      var fine = j >= D.lines,
        F = fine ? FINE[j - D.lines] : null,
        d = fine ? F.d : (j / (D.lines - 1)) * 2 - 1,
        A = fine
          ? F.a * (1 - m) * (1 - m) * arr
          : 1 + (ringLife(j) - 1) * eC + (sealLife(j) - 1) * eB + (noteLife() - 1) * eN + ((j === 0 ? 1 : 0) - 1) * eP;
      /* the soft share of the fine tendrils is left out: a glow with no line read as a smear */
      if (fine && (A < 0.01 || F.r < JELLY.soft)) continue;
      /* heir: the other lines dissolve at HEIR.at (closer the more silence); the heir line skips a small gap there and goes on */
      var heir = j === HEIR.line,
        end = heir ? 9 : 1.5 - (1.5 - HEIR.at) * eH,
        gapTo = HEIR.at + HEIR.gap * eH,
        thin = 1 - (heir ? 0.35 : HEIR.thin) * eH;
      var ringW = eC > 0 && !fine ? 1 + (ringFine(j) - 1) * eC : 1,
        xs = [],
        ys = [],
        ws = [],
        ph = fine ? F.p : j * 1.7,
        ph2 = fine ? F.q : j * 2.9,
        fq = JELLY.curl * (fine ? F.f : 1),
        wk = fine ? 1 : 0.45;
      for (i = 0; i <= S; i++) {
        var U = i / S,
          V = vOf(U),
          tail = clamp((1.15 - V) / 1.3),
          turn = V * Math.PI * D.twist + phase,
          env = 0.6 + 0.4 * Math.sin(clamp(V) * Math.PI);
        var off = wide * d * (Math.cos(turn) * env + JELLY.spread * tail),
          arch = wide * 0.2 * Math.sin(turn) * Math.max(0, 1 - d * d);
        var wav =
          JELLY.wave *
          scale *
          wk *
          (1 - K) *
          Math.pow(tail, 1.3) *
          (Math.sin(TAU * V * fq + T * sp + ph) + 0.4 * Math.sin(TAU * V * fq * 2.3 - T * sp * 0.7 + ph2));
        if (fine) {
          off *= arr;
          wav *= arr;
        }
        if (eLim > 0) {
          /* limit: a throat the lines bundle through; the pulse swells before it and is squeezed inside it */
          var sq = Math.exp(-Math.pow((V - LIMIT.at) / LIMIT.span, 2)),
            f = (1 - eLim * pinch * sq) * (1 + eLim * LIMIT.swell * pulseAt(U) * (1 - sq));
          off *= f;
          arch *= f;
          wav *= f;
        }
        var x = pts[i][0] + norms[i][0] * (off + arch + wav),
          y = pts[i][1] + norms[i][1] * (off + arch + wav);
        if (fine && swayK > 0.001) {
          var sx = touchX - x,
            sy = touchY - y,
            sk = swayK * Math.exp(-(sx * sx + sy * sy) / swayR);
          x -= sx * sk;
          y -= sy * sk;
        } /* the fine tendrils lean a little away from the finger */
        if (ribOn) {
          var rx = (x - ribX) * ribZ,
            ry = (y - ribY) * ribZ;
          x = ribX + rx * ribC - ry * ribS;
          y = ribY + rx * ribS + ry * ribC;
        }
        if (m > 0 && !fine) {
          var q = eL > 0 ? hexPt(j, U) : [0, 0],
            t = eS > 0 ? triPt(j, U, STOP, STOP.depth) : [0, 0],
            t2 = eT > 0 ? triPt(j, U, STOPS, STOPS.count) : [0, 0];
          var kn = eN > 0 ? notePt(j, U) : [0, 0],
            br = eB > 0 ? sealPt(j, U) : [0, 0],
            ch = eC > 0 ? ringPt(j, U) : [0, 0],
            pr = eP > 0 ? pricePt(U) : [0, 0],
            pk = eK > 0 ? peoplePt(j, U) : [0, 0],
            ar = eA > 0 ? artPt(j, U) : [0, 0];
          var sw = Math.max(0, CHANGE.sweep),
            M = ease(clamp(m * (1 + sw) - sw * U)),
            Mk = M / m; /* each point crosses over in its own moment, so the line pours into the shape */
          var sx = q[0] * eL + t[0] * eS + t2[0] * eT + kn[0] * eN + br[0] * eB,
            sy = q[1] * eL + t[1] * eS + t2[1] * eT + kn[1] * eN + br[1] * eB;
          sx += ch[0] * eC + pr[0] * eP + pk[0] * eK + ar[0] * eA;
          sy += ch[1] * eC + pr[1] * eP + pk[1] * eK + ar[1] * eA;
          var sh = [sx * Mk, sy * Mk];
          if (liftE > 0.001 && M > 0) sh = liftPt(sh[0] / M, sh[1] / M, M);
          x = x * (1 - M) + sh[0];
          y = y * (1 - M) + sh[1];
        }
        var fade = 1;
        if (heir) {
          if (eH > 0) fade = 1 + (clamp((V <= HEIR.at ? HEIR.at - V : V - gapTo) / 0.1) - 1) * eH;
        } else fade = clamp((end - V) / 0.14);
        if (m > 0) fade += (1 - fade) * m; /* a closed shape keeps its full width right round */
        var noteW =
          eN > 0 && !fine
            ? 1 + (Math.max(clamp((U - noteFill(j) * (1 + 1 / 30)) * 30 + 1), ease(noteSet)) - 1) * eN
            : 1; /* the fuse: the line is what is still ahead of the spark, the burnt part behind it is gone; the whole outline once the time is up */
        var taper = 1 + JELLY.taper * (0.3 - tail),
          lw = thick * thin * fade * ringW * noteW * ((1 - m) * JELLY.line * taper * (fine ? F.w : 1) + m);
        if (lw <= 0.02 || (heir && eH > 0 && V > HEIR.at && V < gapTo)) {
          xs.push(null);
          ys.push(null);
          ws.push(0);
          continue;
        }
        xs.push(x);
        ys.push(y);
        ws.push(lw);
      }
      lines.push({ P: tendril(xs, ys, ws), a: A, far: fine && F.r < JELLY.soft, fine: fine, xs: xs, ys: ys });
      if (!fine && amp > 0.01) {
        var cm = comet(xs, ys, thick * JELLY.line * 3, pp, w, tint);
        if (cm) pulses.push(cm);
      }
      if (fire) {
        var fc = comet(
          xs,
          ys,
          thick * JELLY.line * 3 * FIRE.size * (fine ? F.w : 1),
          fire.dir > 0 ? fpp : 1 - fpp,
          fw,
          fireTint,
          fire.dir,
          fireHead
        );
        if (fc) fires.push({ P: fc.P, G: fc.G, a: Math.min(1, FIRE.bright * A) });
      }
      var sk = !fine && m < 0.6 ? sparks[j] : null;
      if (sk) {
        var sv = uOf(-0.1 + (1.3 * sk.age) / SPARK.run),
          sc = comet(xs, ys, thick * JELLY.line * 3 * SPARK.size, sv, fw, tint, 1);
        if (sc) fires.push({ P: sc.P, G: sc.G, a: Math.min(1, SPARK.bright * A * (1 - m)) });
      }
    }
    var bars = null,
      trackP = null;
    if (eN > 0.01) {
      /* notice: the faint shape still to fill */
      trackP = new Path2D();
      for (j = 0; j < D.lines; j++)
        for (i = 0; i <= 24; i++) {
          var e0 = notePt(j, i / 24, true);
          if (i || j) trackP.lineTo(e0[0], e0[1]);
          else trackP.moveTo(e0[0], e0[1]);
        }
      trackP.closePath(); /* one closed path, so its corners join */
    }
    var fuse =
      null; /* the fuse: the spark at the burn point, the ember trail behind it, the flying bits, and the lock's bolt */
    if (eN > 0.01) {
      var bsh = clamp(noteAge() / Math.max(0.5, NOTICE.unwind)),
        setE = ease(noteSet),
        fz = 1 + NOTICE.flick * Math.sin(TAU * clock * NOTICE.flickS) * Math.sin(TAU * clock * NOTICE.flickS * 0.37 + 1);
      var atP = function (s) {
          s = clamp(s);
          var jj = Math.min(D.lines - 1, Math.floor(s * D.lines));
          return notePt(jj, s * D.lines - jj, true);
        },
        emb = [],
        NE = 40,
        tr = Math.max(0.01, NOTICE.trail);
      if (bsh > 0 && bsh < 1 && setE < 0.99)
        for (i = 1; i <= NE; i++) {
          /* embers: brightest at the spark, fading back to the track over NOTICE.trail of the outline */
          var s1 = bsh - tr * (1 - i / NE),
            k0 = i / NE;
          if (s1 <= 0) continue;
          emb.push({ a: atP(Math.max(0, s1 - tr / NE)), b: atP(s1), al: NOTICE.emberA * k0 * k0 * (1 - setE) * (0.75 + 0.25 * fz) });
        }
      fuse = { on: bsh < 1 && setE < 0.99, p: atP(bsh), r: NOTICE.spark * scale * fz, emb: emb, al: (1 - setE) * eN, bits: [], bolt: null };
      for (i = 0; i < Math.round(NOTICE.sparks); i++) {
        /* flying bits: each circles the spark on its own clock and drifts up */
        var an = i * 2.4 + clock * NOTICE.flickS * (0.6 + 0.1 * i),
          rr = fuse.r * (1.3 + 1.6 * (0.5 + 0.5 * Math.sin(clock * NOTICE.flickS * (1.1 + 0.23 * i) + i * 1.7)));
        fuse.bits.push([
          fuse.p[0] + Math.cos(an) * rr,
          fuse.p[1] + Math.sin(an) * rr - rr * 0.4,
          0.4 + 0.6 * (0.5 + 0.5 * Math.sin(clock * NOTICE.flickS * 1.7 + i * 2.1)),
        ]);
      }
      if (lock) {
        /* the bolt: a bar slides across the shape's middle over LOCK.pinch of the one-shot, then fades */
        var lt = clamp(lock.age / Math.max(0.3, LOCK.s)),
          lp = Math.max(0.05, Math.min(0.95, LOCK.pinch)),
          bx0 = W * NOTICE.x - NOTICE.size * scale * LOCK.bar,
          bl = 2 * NOTICE.size * scale * LOCK.bar;
        fuse.bolt = { x0: bx0, x1: bx0 + bl * ease(clamp(lt / lp)), y: H * NOTICE.y, al: eN * (1 - ease(clamp((lt - lp) / (1 - lp)))) };
      }
    }
    function drawFuse(ctx, big) {
      /* big: the bloom copy, wider and softer */
      if (!fuse) return;
      var em = NOTICE.ember.join(',');
      ctx.strokeStyle = 'rgb(' + em + ')';
      ctx.lineWidth = thick * (big ? 1.4 : 0.7);
      ctx.lineCap = 'round';
      fuse.emb.forEach(function (e) {
        ctx.globalAlpha = e.al * fuse.al;
        ctx.beginPath();
        ctx.moveTo(e.a[0], e.a[1]);
        ctx.lineTo(e.b[0], e.b[1]);
        ctx.stroke();
      });
      if (fuse.on) {
        var r = fuse.r * (big ? 3 : 1.6),
          SG = ctx.createRadialGradient(fuse.p[0], fuse.p[1], 0, fuse.p[0], fuse.p[1], r);
        SG.addColorStop(0, 'rgba(255,250,225,1)');
        SG.addColorStop(0.3, 'rgba(255,205,95,0.85)');
        SG.addColorStop(1, 'rgba(' + em + ',0)');
        ctx.globalAlpha = fuse.al;
        ctx.fillStyle = SG;
        ctx.fillRect(fuse.p[0] - r, fuse.p[1] - r, 2 * r, 2 * r);
        ctx.fillStyle = 'rgb(255,225,150)';
        fuse.bits.forEach(function (b) {
          ctx.globalAlpha = fuse.al * b[2];
          ctx.beginPath();
          ctx.arc(b[0], b[1], (big ? 1.2 : 0.7) * scale, 0, TAU);
          ctx.fill();
        });
      }
      if (fuse.bolt) {
        ctx.globalAlpha = fuse.bolt.al;
        ctx.strokeStyle = 'rgb(' + tint + ')';
        ctx.lineWidth = thick * LOCK.barW * (big ? 1.5 : 1);
        ctx.beginPath();
        ctx.moveTo(fuse.bolt.x0, fuse.bolt.y);
        ctx.lineTo(fuse.bolt.x1, fuse.bolt.y);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    }
    if (grow > 0.01) {
      /* locked: six spokes draw in from the outer corners to the inner ring once every ring has seated */
      bars = new Path2D();
      var n0 = Math.max(2, Math.min(D.lines, Math.round(HEX.rings))),
        r0 = HEX.size * scale,
        r1 = r0 + (r0 * (1 / n0) - r0) * grow * clamp(HEX.spokes),
        hx = W * HEX.x,
        hy = H * HEX.y;
      for (i = 0; i < Math.max(3, Math.min(12, Math.round(HEX.sides))); i++) {
        var sa = -Math.PI / 2 + (TAU * i) / Math.max(3, Math.min(12, Math.round(HEX.sides))),
          s0 = spun([hx + r0 * Math.cos(sa), hy + r0 * Math.sin(sa)], HEX, hx, hy),
          s1 = spun([hx + r1 * Math.cos(sa), hy + r1 * Math.sin(sa)], HEX, hx, hy);
        bars.moveTo(s0[0], s0[1]);
        bars.lineTo(s1[0], s1[1]);
      }
    }
    /* signals: small glows travelling up the tendrils, each with its own smooth brightness and a short tail, like
       packets on the Kaspa network. Position is interpolated between samples, so nothing steps. */
    var glows = [],
      beadA = 1 - m,
      count2 = beadA > 0.02 ? Math.min(BEADS.length, Math.round(JELLY.beads)) : 0;
    for (i = 0; i < count2; i++) {
      var be = BEADS[i],
        L = lines[Math.floor(be.l * lines.length)],
        vv = be.v - ((lclk * be.s) % 1.3);
      if (vv < -0.15) vv += 1.3;
      var fi = uOf(vv) * S,
        i0 = Math.floor(fi),
        ff = fi - i0;
      if (i0 < 0 || i0 >= S) continue;
      var x0 = L.xs[i0],
        x1 = L.xs[i0 + 1];
      if (x0 == null || x1 == null) continue;
      var bx = x0 + (x1 - x0) * ff,
        by = L.ys[i0] + (L.ys[i0 + 1] - L.ys[i0]) * ff,
        i2 = Math.min(S, i0 + 4),
        tx = L.xs[i2],
        ty = L.ys[i2];
      var tw = 0.5 + 0.5 * Math.sin((TAU * clock) / be.t + be.p),
        rad = be.z * JELLY.beadSize * scale * 0.8;
      glows.push({ x: bx, y: by, r: rad, a: 0.3 + 0.7 * tw, tx: tx == null ? bx : tx, ty: tx == null ? by : ty });
    }
    var lightA = [0.25, 0.6, 1],
      shapeL = [];
    [
      [HEX, eL],
      [STOP, eS],
      [STOPS, eT],
      [SEAL, eB],
      [RING, eC],
      [NOTICE, eN],
      [PRICE, eP],
    ].forEach(function (o) {
      var O = o[0],
        n = Math.round(O.lights),
        rad = O.lightSize * scale,
        P = new Path2D();
      if (o[1] < 0.02 || n < 1) return;
      for (var q = 0; q < n; q++) {
        var L2 = lines[q % D.lines],
          id = Math.floor(((((q * 0.618 + lclk * O.lightRun) % 1) + 1) % 1) * S),
          lx = L2.xs[id];
        if (L2.a < 0.1 || lx === null || lx === undefined) continue;
        P.moveTo(lx + rad, L2.ys[id]);
        P.arc(lx, L2.ys[id], rad, 0, TAU);
      }
      shapeL.push({ P: P, a: o[1] });
    });
    peopleBursts(lines, eK).forEach(function (b) {
      shapeL.push(b);
    });
    mark(1);
    /* small copy for the glow: every line drawn on the frame */
    var S0 = bloom[0],
      B = S0.g,
      bs = 1 / S0.n;
    B.setTransform(1, 0, 0, 1, 0, 0);
    B.clearRect(0, 0, S0.c.width, S0.c.height);
    B.setTransform(bs, 0, 0, bs, W * PAD * bs, H * PAD * bs);
    B.globalCompositeOperation = 'lighter';
    B.fillStyle = grH;
    B.strokeStyle = grH;
    B.lineWidth = JELLY.body * scale;
    B.lineJoin = 'round';
    lines.forEach(function (L) {
      B.globalAlpha = Math.min(1, L.a * bright0(L));
      B.fill(L.P);
      B.stroke(L.P);
    });
    function bright0(L) {
      return L.far ? 1 : L.fine ? 0.85 : 0.85 * (1 + 0.5 * charge);
    }
    if (bars) {
      B.globalAlpha = grow * eL;
      B.stroke(bars);
    }
    pulses.forEach(function (p) {
      B.globalAlpha = amp;
      B.fillStyle = p.G;
      B.fill(p.P);
    });
    fires.forEach(function (p) {
      B.globalAlpha = p.a;
      B.fillStyle = p.G;
      B.fill(p.P);
    });
    drawFuse(B, true);
    var haloS = Math.round(halo[0]) + ',' + Math.round(halo[1]) + ',' + Math.round(halo[2]);
    if (touchA > 0.01 && FINGER.glow > 0) {
      /* the soft bloom under the finger */
      var tr = 70 * scale * FINGER.glow,
        TG = B.createRadialGradient(touchX, touchY, 0, touchX, touchY, tr);
      TG.addColorStop(0, 'rgba(' + haloS + ',0.8)');
      TG.addColorStop(1, 'rgba(' + haloS + ',0)');
      B.globalAlpha = touchA;
      B.fillStyle = TG;
      B.fillRect(touchX - tr, touchY - tr, 2 * tr, 2 * tr);
    }
    B.fillStyle = 'rgb(' + haloS + ')';
    B.strokeStyle = B.fillStyle;
    B.lineCap = 'round';
    glows.forEach(function (q) {
      /* the glow and its tail, blurred by the bloom */
      B.globalAlpha = q.a * beadA;
      B.beginPath();
      B.arc(q.x, q.y, q.r * 1.5, 0, TAU);
      B.fill();
      B.globalAlpha = q.a * beadA * 0.45;
      B.lineWidth = q.r * 1.3;
      B.beginPath();
      B.moveTo(q.x, q.y);
      B.lineTo(q.tx, q.ty);
      B.stroke();
    });
    shapeL.forEach(function (p) {
      B.globalAlpha = p.a;
      B.fill(p.P);
    });
    B.globalAlpha = 1;
    var breath = 1 + JELLY.breath * Math.sin((TAU * clock) / 11);
    mark(2);
    halate(GLOW_ALL * JELLY.halation * (1 + 1.2 * charge) * (1 + glow / 16) * breath);
    mark(3);
    if (touchA > 0.01 && FINGER.glow > 0) {
      /* and a smooth one on the frame itself, so the finger reads at once */
      var tr2 = 55 * scale * FINGER.glow,
        TG2 = g.createRadialGradient(touchX, touchY, 0, touchX, touchY, tr2);
      TG2.addColorStop(0, 'rgba(' + tint + ',0.3)');
      TG2.addColorStop(0.5, 'rgba(' + tint + ',0.08)');
      TG2.addColorStop(1, 'rgba(' + tint + ',0)');
      g.globalAlpha = touchA;
      g.fillStyle = TG2;
      g.fillRect(touchX - tr2, touchY - tr2, 2 * tr2, 2 * tr2);
    }
    /* the sharp lines on top; add, so a crossing burns brighter, like light */
    g.globalCompositeOperation = 'lighter';
    g.fillStyle = gr;
    g.strokeStyle = gr;
    lines.forEach(function (L) {
      if (L.far) return;
      g.globalAlpha = L.a;
      g.fill(L.P);
    });
    if (charge > 0.01) {
      /* the slide: a front of light moves along the main lines with the thumb, white at the front */
      var CG = g.createLinearGradient(rx0, ry0, rx1, ry1),
        cf = clamp(charge);
      CG.addColorStop(0, 'rgba(' + tint + ',0.7)');
      CG.addColorStop(Math.max(0.001, cf - 0.05), 'rgba(' + tint + ',0.7)');
      CG.addColorStop(cf, 'rgba(255,255,255,0.95)');
      CG.addColorStop(Math.min(0.999, cf + 0.03), 'rgba(' + tint + ',0.06)');
      CG.addColorStop(1, 'rgba(' + tint + ',0)');
      g.strokeStyle = CG;
      g.globalAlpha = 0.9;
      g.lineWidth = thick * 0.5;
      lines.forEach(function (L) {
        if (!L.fine) g.stroke(L.P);
      });
      g.strokeStyle = gr;
    }
    if (touchA > 0.01 && FINGER.light > 0) {
      /* the finger: the lines passing near it brighten, and a short ring of light runs outward from it */
      var fr = 120 * scale * FINGER.radius,
        ru = (touchAge * FINGER.speed) % 1,
        rw = 0.12,
        FG = g.createRadialGradient(touchX, touchY, 0, touchX, touchY, fr),
        fb0 = 0.55;
      [
        [0, fb0],
        [Math.max(0.001, ru - rw), fb0 * (1 - ru)],
        [ru, (fb0 + FINGER.ring) * (1 - ru)],
        [Math.min(0.999, ru + rw * 0.6), fb0 * 0.4 * (1 - ru)],
        [1, 0],
      ]
        .sort(function (a, b) {
          return a[0] - b[0];
        })
        .forEach(function (s) {
          FG.addColorStop(s[0], 'rgba(' + tint + ',' + Math.max(0, Math.min(1, s[1])).toFixed(3) + ')');
        });
      g.strokeStyle = FG;
      g.globalAlpha = touchA * FINGER.light;
      lines.forEach(function (L) {
        if (L.far) return;
        g.lineWidth = thick * (L.fine ? 0.25 : 0.5);
        g.stroke(L.P);
      });
      g.strokeStyle = gr;
    }
    if (bars) {
      g.globalAlpha = grow * eL;
      g.lineWidth = thick * 0.45;
      g.stroke(bars);
    }
    if (trackP) {
      g.globalAlpha = NOTICE.track * eN * noteLife();
      g.lineWidth = thick * 0.5;
      g.stroke(trackP);
    }
    drawFuse(g, false);
    g.strokeStyle = gr;
    g.globalAlpha = 1;
    pulses.forEach(function (p) {
      g.globalAlpha = amp;
      g.fillStyle = p.G;
      g.fill(p.P);
    });
    fires.forEach(function (p) {
      g.globalAlpha = p.a;
      g.fillStyle = p.G;
      g.fill(p.P);
    });
    g.globalAlpha = 1;
    g.fillStyle = 'rgb(236,247,255)';
    glows.forEach(function (q) {
      g.globalAlpha = q.a * beadA * (bright + 0.05);
      g.beginPath();
      g.arc(q.x, q.y, q.r, 0, TAU);
      g.fill();
    }); /* the bright core */
    shapeL.forEach(function (p) {
      g.globalAlpha = p.a;
      g.fill(p.P);
    });
    g.globalAlpha = 1;
    var dimBy = pageL * (1 - m) * (0.5 - 0.5 * Math.sin(pagePh));
    [
      [HEX, eL],
      [STOP, eS],
      [STOPS, eT],
      [SEAL, eB],
      [RING, eC],
      [NOTICE, eN],
      [PRICE, eP],
      [LIMIT, eLim * (1 - m)],
      [HEIR, eHr * (1 - m)],
    ].forEach(function (o) {
      dimBy += o[1] * o[0].light * (0.5 - 0.5 * Math.sin((TAU * clock) / Math.max(0.5, o[0].lightS)));
    });
    var cover = 1 - ease(veil) * (1 - Math.min(0.95, dimBy)); /* the light pulse: the same cover the fade uses */
    if (cover > 0) {
      g.globalCompositeOperation = 'source-over';
      g.globalAlpha = cover;
      g.fillStyle = '#06070c';
      g.fillRect(0, 0, W, H);
      g.globalAlpha = 1;
      g.globalCompositeOperation = 'lighter';
    }
    if (grain) {
      g.setTransform(1, 0, 0, 1, 0, 0);
      g.drawImage(grain, 0, 0);
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
    coinCode(g);
    g.globalCompositeOperation = 'source-over';
    var dotA = (K - 0.9) * 10 * (1 - ease(mixT)); /* no dot beside a shape (the stop triangles) */
    if (dotA > 0.01) {
      g.beginPath();
      g.arc(0.16 * W, 0.47 * H, 3, 0, TAU);
      g.fillStyle = 'rgba(255,245,238,' + dotA + ')';
      g.fill();
    }
  }
  /* slow phone: one-way steps down in sharpness, never a visible flicker back up */
  function pace(ms, work) {
    if (ms > 250) return; /* a pause (a prompt, another app in front), not a slow frame */
    if (ms > fMax) fMax = ms;
    if (ms > 25) fSlow++;
    fN++;
    fT += ms;
    fW += work;
    if (fN < 90) return;
    var avg = fT / fN;
    lastMs = avg;
    lastWork = fW / fN;
    var parts = pT.map(function (v) {
      return (v / fN).toFixed(1);
    });
    parts = parts.join('/') + ' max ' + fMax.toFixed(0) + ' slow ' + fSlow;
    pT = [0, 0, 0, 0, 0];
    fMax = fSlow = 0;
    fN = 0;
    fT = 0;
    fW = 0;
    console.log(
      'KVScene frame ms ' +
        avg.toFixed(1) +
        ' level ' +
        level +
        ' work ms ' +
        lastWork.toFixed(1) +
        ' step/build/copy/glow/paint ' +
        parts +
        ' shown ' +
        shown +
        ' want ' +
        st.mode +
        ' k ' +
        K.toFixed(2) +
        ' stop ' +
        stopT.toFixed(2) +
        ' mix ' +
        mixT.toFixed(2) +
        ' veil ' +
        veil.toFixed(2) +
        ' dim ' +
        dim.toFixed(2) +
        (asleep ? ' asleep' : '') +
        (document.hidden ? ' hidden' : '')
    );
    if (avg > 26 && level < 2) {
      level++;
      size();
    }
  }
  function frame(now) {
    requestAnimationFrame(frame);
    if (document.hidden || asleep) {
      prev = now;
      return;
    }
    var dt = Math.min(0.033, (now - prev) / 1000 || 0.016),
      t0 = performance.now();
    if (now < restUntil) {
      /* a very slow phone: the last frame took long, so leave the main thread free a while. pace()
         is
         handed the slow frame's own overrun (the first skipped interval) but never the rest after
         it */
      if (lastRaf > prev) prev += now - lastRaf;
      lastRaf = now;
      return;
    }
    for (var s = 0; s < 3; s++) step(dt / 3);
    fNo++;
    markAt = t0;
    mark(0);
    draw();
    mark(4);
    var work = performance.now() - t0;
    if (prev) pace(now - prev, work);
    prev = lastRaf = now;
    /* rest until 5 x work after the frame's start: at most a fifth of the time drawing, so taps and
       the bridge
       never wait seconds */
    restUntil = work > 120 ? now + work * 5 : 0;
  }
  var restUntil = 0,
    lastRaf = 0;
  /* where a frame's time goes, summed per stretch between marks: step, build, glow copy, glow, paint */
  var pT = [0, 0, 0, 0, 0],
    markAt = 0,
    fMax = 0,
    fSlow = 0;
  function mark(i) {
    var n = performance.now();
    pT[i] += n - markAt;
    markAt = n;
  }

  window.KVScene = {
    init: function (el) {
      c = el;
      g = c.getContext('2d', {
        alpha: false,
      }); /* no desynchronized: on the phone it could stop presenting after the window's secure flag changed */
      size();
      addEventListener('resize', size);
      for (var i = 0; i < N; i++) {
        var q = rest(i / (N - 1));
        nodes.push({ x: q[0], y: q[1], vx: 0, vy: 0 });
      }
      bind();
      requestAnimationFrame(frame);
    },
    setState: function (o) {
      for (var n in o)
        if (o[n] !== undefined) {
          if (n === 'snap') {
            if (o[n]) wantSnap = true;
          } else {
            /* the K draws in on each arrival */
            if (n === 'mark' && o[n] === 'k' && st.mark !== 'k') sealAge = 0;
            st[n] = o[n];
          }
        }
    },
    pulse: function (kind) {
      if (kind === 'lock') {
        lock = { age: 0 };
        return;
      }
      fire = { age: 0, dir: kind === 'receive' ? -1 : 1, stop: kind === 'stop' };
    },
    page: function () {
      return pageName;
    },
    shapePt: function (j, U) {
      return peoplePt(j, U);
    } /* headless checks of the People weave */,
    sleep: function (on) {
      asleep = !!on;
    },
    perf: function () {
      return { ms: lastMs, work: lastWork, level: level };
    } /* ms: frame to frame; work: step + draw time, both averaged over the last 90 frames */,
    dials: {
      change: CHANGE,
      pages: PAGES,
      base: BASE,
      drift: DRIFT,
      hex: HEX,
      stop: STOP,
      flow: FLOW,
      ring: RING,
      limit: LIMIT,
      due: NOTICE,
      lock: LOCK,
      stops: STOPS,
      heir: HEIR,
      seal: SEAL,
      kmark: KMARK,
      arrive: ARRIVE,
      people: PEOPLE,
      price: PRICE,
      accent: ACCENT,
      jelly: JELLY,
      fire: FIRE,
      finger: FINGER,
    },
  };
})();
