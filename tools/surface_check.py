"""The phone app's attack surface, checked for drift. Run from the repo root: python tools/surface_check.py [--table]

Reads four places and fails (exit 1) when they disagree:
  page    app/web/data.js localMethod: every engine method a screen can route to
  Bridge  app/android/.../Bridge.java: ENGINE_METHODS (what the page may call), the @JavascriptInterface doors
  Java    MainActivity SETTINGS (keys the page may write), VaultLib.call(...) made by Java itself (GuardService)
  engine  testnet-client/src/*.rs: the method arms that answer
Rules: page routes == ENGINE_METHODS; every allowed method has an engine arm or is answered in Java by name; every
JS door is used by the page; every SETTINGS key is used by the page and every key the page writes is in SETTINGS.
--table prints the inventory as markdown (for docs/SURFACE.md)."""
import glob, os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..'))
JAVA_DIR = os.path.join(ROOT, 'app', 'android', 'src', 'com', 'kaspavault', 'wallet')
JAVA_ANSWERED = {'keys.burn', 'keys.sheets'}  # Bridge answers these itself (sealed vault seeds), then calls the engine keyless


def read(*parts):
    return open(os.path.join(ROOT, *parts), encoding='utf-8').read().replace('\r\n', '\n')


def expand(rx):
    """All strings a simple alternation regex matches: literals, \\. escapes, (a|b) groups, nested."""
    def seq(s, i):
        outs = ['']
        while i < len(s) and s[i] not in '|)':
            if s[i] == '(':
                alts, i = alt(s, i + 1)
                outs = [o + a for o in outs for a in alts]
            elif s[i] == '\\':
                outs = [o + s[i + 1] for o in outs]; i += 2
            else:
                outs = [o + s[i] for o in outs]; i += 1
        return outs, i

    def alt(s, i):
        res = []
        while True:
            outs, i = seq(s, i)
            res += outs
            if i >= len(s) or s[i] == ')':
                return res, i + 1
            i += 1  # '|'

    return alt(rx, 0)[0]


def local_method():
    js = read('app', 'web', 'data.js')
    body = js[js.index('function localMethod'):]
    return body[:body.index('\n  }\n')]


def path_map():
    """/api/<path> -> engine name, as localMethod resolves it."""
    body, m = local_method(), {}
    for hit in re.finditer(r"p === '/api/([a-z_.]+)'", body):  # each branch returns before the next one starts
        m[hit.group(1)] = re.compile(r"return \{ name: '([a-z_.]+)'").search(body, hit.end()).group(1)
    for rx in re.findall(r"/\^\\/api\\/(.*?)\$/\.test\(p\)", body):
        for x in expand(rx): m[x] = x
    return m


def page_routes():
    return set(path_map().values())


def java():
    return {os.path.basename(f): open(f, encoding='utf-8').read() for f in glob.glob(os.path.join(JAVA_DIR, '*.java'))}


def string_set(src, name):
    m = re.search(name + r' = new HashSet<>\(Arrays\.asList\((.*?)\)\);', src, re.S)
    return set(re.findall(r'"([a-z_.]+)"', m.group(1)))


def engine_arms():
    arms = set()
    for f in glob.glob(os.path.join(ROOT, 'testnet-client', 'src', '*.rs')):
        for m in re.finditer(r'^\s*((?:"[a-z_.]+"\s*\|\s*)*"[a-z_.]+")\s*(?:if [^=]*)?=>', open(f, encoding='utf-8').read(), re.M):
            arms |= set(re.findall(r'"([a-z_.]+)"', m.group(1)))
        # early answers before the match: `if method == "keys.public" {` / `|| method == "link.read"`
        arms |= set(re.findall(r'method == "([a-z_.]+)"', open(f, encoding='utf-8').read()))
    return arms


def main():
    j = java()
    bridge, main_act = j['Bridge.java'], j['MainActivity.java']
    web = read('app', 'web', 'data.js') + read('app', 'web', 'app.js')
    routes, allowed, arms = page_routes(), string_set(bridge, 'ENGINE_METHODS'), engine_arms()
    direct = set(re.findall(r'VaultLib\.call\("([a-z_.]+)"', ''.join(j.values())))
    doors = sorted(set(re.findall(r'@JavascriptInterface public (?:synchronized )?\w+ (\w+)\(', bridge)))
    settings = string_set(main_act, 'SETTINGS')
    written = set(re.findall(r"setSetting\('([a-z_]+)'", web))
    bad = []
    bad += ['page routes %s, which Bridge does not allow' % m for m in sorted(routes - allowed)]
    bad += ['Bridge allows %s, which no screen routes' % m for m in sorted(allowed - routes)]
    bad += ['Bridge allows %s, which nothing answers' % m for m in sorted(allowed - arms - JAVA_ANSWERED)]
    bad += ['JS door Bridge.%s is not used by the page' % d for d in doors if not re.search(r'bridge\.%s\b' % d, web)]
    bad += ['setting %s is allowed but the page never uses it' % k for k in sorted(settings) if not re.search(r'\b%s\b' % k, web)]
    bad += ['the page writes setting %s, which Java drops' % k for k in sorted(written - settings)]
    called = set(re.findall(r"'/api/([a-z_.]+)", web.replace(local_method(), '')))
    bad += ['the page calls /api/%s, which localMethod does not route (a dead button)' % c for c in sorted(called - set(path_map()))]
    if '--table' in sys.argv:
        print('| Engine method | Page route | Answered by |\n|---|---|---|')
        for m in sorted(allowed | direct):
            who = 'Java (by name)' if m in JAVA_ANSWERED else 'engine' if m in arms else '?'
            print('| `%s` | %s | %s%s |' % (m, 'yes' if m in routes else 'no (Java only)', who, ', GuardService' if m in direct else ''))
        print('\nJS doors: ' + ', '.join(doors))
        print('\nSettings keys: ' + ', '.join(sorted(settings)))
        print('\nEngine methods the phone cannot reach (laptop only): ' + ', '.join(sorted(arms - allowed - direct)))
    if bad:
        print('SURFACE DRIFT: %d' % len(bad)); print('\n'.join(bad)); sys.exit(1)
    print('surface ok: %d routes, %d JS doors, %d settings keys, %d engine methods reachable' % (len(routes), len(doors), len(settings), len(allowed | direct)))


if __name__ == '__main__':
    main()
