import json, os, random, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
recs = json.load(open("data/recs.json"))
N_CAND, N_KEEP = 70, 30
env = dict(os.environ, GIT_TERMINAL_PROMPT="0")
def sh(*a, cwd=None, timeout=300):
    return subprocess.run(a, cwd=cwd, capture_output=True, text=True, env=env, timeout=timeout)
def fetch(job):
    year, item = job
    repo = item["repo"]; dst = f"code/{year}/{repo.replace('/', '__')}"
    res = {"year": year, **item, "dir": dst}
    try:
        if not os.path.isdir(dst + "/.git"):
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            r = sh("git", "clone", "-q", "--filter=blob:none", "--no-checkout", f"https://github.com/{repo}", dst)
            if r.returncode: res["status"] = "clone-failed"; res["err"] = r.stderr.strip()[-120:]; return res
        c = sh("git", "rev-list", "-1", f"--before={year}-12-31T23:59", "HEAD", cwd=dst).stdout.strip()
        if not c: res["status"] = "no-history-by-year-end"; return res
        res["commit"] = c
        res["commit_date"] = sh("git", "show", "-s", "--format=%cs", c, cwd=dst).stdout.strip()
        tree = sh("git", "ls-tree", "--name-only", c, cwd=dst).stdout.split()
        if "Cargo.toml" not in tree: res["status"] = "no-root-manifest"; return res
        r = sh("git", "checkout", "-q", "--detach", c, cwd=dst)
        if r.returncode: res["status"] = "checkout-failed"; res["err"] = r.stderr.strip()[-120:]; return res
        res["status"] = "ok"
    except Exception as e:
        res["status"] = "error"; res["err"] = str(e)[:120]
    return res
jobs = []
for y in ("2015", "2018", "2021", "2024"):
    items = [i for i in recs[y]["awesome"] if i["top"] in ("Libraries", "Applications", "Applications written in Rust", "Development tools")]
    seen, uniq = set(), []
    for i in items:
        if i["repo"] not in seen: seen.add(i["repo"]); uniq.append(i)
    random.Random(int(y)).shuffle(uniq)
    jobs += [(y, i) for i in uniq[:N_CAND]]
with ThreadPoolExecutor(8) as ex:
    results = list(ex.map(fetch, jobs))
json.dump(results, open("data/fetch.json", "w"), indent=1)
from collections import Counter
for y in ("2015", "2018", "2021", "2024"):
    print(y, Counter(r["status"] for r in results if r["year"] == y))
