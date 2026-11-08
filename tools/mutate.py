"""Mutation test for contracts/vault.sil.

Breaks one check at a time and reruns the engine tests. A mutant that still
passes every test means that check is untested or redundant.
Usage: python tools/mutate.py <path to built vault test binary>
"""
import concurrent.futures as cf
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEST_BIN = sys.argv[1]
lines = open(os.path.join(ROOT, "contracts", "vault.sil"), newline="").read().split("\n")

mutants = []
for i, line in enumerate(lines):
    code = line.strip()
    if code.startswith("//"):
        continue
    if code.startswith("require("):
        mutants.append((i, "delete", line, ""))
        inner = code[len("require("):-2]
        for old, new in ((">=", ">"), ("<=", "<"), (" < ", " <= "), (" > ", " >= "), ("==", "!=")):
            if old in inner:
                mutants.append((i, f"{old.strip()} to {new.strip()}", line, line.replace(old, new, 1)))
    if re.match(r"(soloVaultInput|soloContinuation|validateOutputState)\(", code) and code.endswith(";"):
        mutants.append((i, "delete", line, ""))


def run(m):
    i, kind, old, new = m
    mutated = list(lines)
    mutated[i] = new
    with tempfile.NamedTemporaryFile("w", suffix=".sil", delete=False, newline="") as f:
        f.write("\n".join(mutated))
    env = dict(os.environ, VAULT_SRC=f.name)
    r = subprocess.run([TEST_BIN], cwd=os.path.join(ROOT, "engine-tests"), env=env, capture_output=True, text=True, timeout=300)
    os.unlink(f.name)
    out = r.stdout + r.stderr
    if r.returncode == 0:
        result = "SURVIVED"
    elif "vault compiles" in out:
        result = "no-compile"
    else:
        result = "killed"
    return i + 1, kind, old.strip(), result


with cf.ThreadPoolExecutor(4) as pool:
    results = list(pool.map(run, mutants))

counts = {}
for line_no, kind, code, result in results:
    counts[result] = counts.get(result, 0) + 1
    if result != "killed":
        print(f"{result:10} line {line_no} [{kind}] {code[:90]}")
print("TOTAL", len(results), counts)
