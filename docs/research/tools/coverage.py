import json, re, tomllib, glob
recs = json.load(open("data/recs.json"))
R = "/Users/andy/tmp/modernize/cargo-modernize/rules/"
eco = tomllib.load(open(R + "ecosystem.toml", "rb"))["crates"]
rule_crates = {}
for f in glob.glob(R + "*.toml"):
    if f.endswith(("ecosystem.toml", "features.toml")): continue
    for r in tomllib.load(open(f, "rb")).get("rule", []):
        if r.get("crate"): rule_crates.setdefault(r["crate"], []).append(r["id"])
        for p in r.get("paths", []):
            head = re.split(r"::|\{", p)[0]
            if head not in ("std", "core"): rule_crates.setdefault(head, []).append(r["id"])
norm = lambda s: s.replace("-", "_")
eco_n = {norm(k): (k, v) for k, v in eco.items()}
rc_n = {norm(k): (k, v) for k, v in rule_crates.items()}
rows = []
def classify(name):
    n = norm(name)
    e = eco_n.get(n); r = rc_n.get(n)
    return (e[1].get("status") if e else None), (sorted(set(r[1])) if r else [])
out = {}
prev = None
for y in ("2018", "2021", "2024"):
    names = recs[y]["cookbook"]
    st = {}
    for n in names:
        s, r = classify(n); st[n] = {"status": s, "rules": r}
    out[y] = st
    flagged = [n for n, v in st.items() if (v["status"] and v["status"] not in ("outdated",)) or v["rules"]]
    print(y, "cookbook crates:", len(names), "flagged now:", len(flagged))
    print("   ", ", ".join(f"{n}({st[n]['status'] or 'rule'})" for n in flagged))
# churn
cb = {y: set(recs[y]["cookbook"]) for y in ("2018", "2021", "2024")}
print("cookbook 2018->2021 dropped:", sorted(cb["2018"] - cb["2021"]), "added:", sorted(cb["2021"] - cb["2018"]))
print("cookbook 2021->2024 dropped:", sorted(cb["2021"] - cb["2024"]), "added:", sorted(cb["2024"] - cb["2021"]))
# awesome-rust repo-name coverage per year (repo name ~ crate name)
for y in ("2015", "2018", "2021", "2024"):
    names = {i["repo"].split("/")[1].lower() for i in recs[y]["awesome"] if i["top"] == "Libraries"}
    flagged = sorted(n for n in names if norm(n) in eco_n and eco_n[norm(n)][1].get("status") not in (None, "outdated") or norm(n) in rc_n)
    out[f"awesome-{y}"] = {"libraries": len(names), "flagged_now": flagged}
    print(f"awesome-rust {y} libraries: {len(names)}, flagged now: {len(flagged)} -> {flagged[:40]}")
if "blessed" in recs["2024"]:
    b = recs["2024"]["blessed"]; fl = [n for n in b if classify(n)[0] not in (None, "outdated") or classify(n)[1]]
    print("blessed 2024:", len(b), "flagged now:", len(fl), fl)
json.dump(out, open("data/coverage.json", "w"), indent=1)
