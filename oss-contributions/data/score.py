"""Combine collected metrics + manual judgements into ../candidates.csv (see REPORT.md rubric)."""
import csv, math, os
D = os.path.dirname(os.path.abspath(__file__))
rd = lambda f: {r["repo"]: r for r in csv.DictReader(open(os.path.join(D, f)))}
cats = {l.split(",")[0]: l.strip().split(",")[1] for l in open(os.path.join(D, "repos.txt"))}
git, srch, resp, man = rd("git_activity.csv"), rd("search_stats.csv"), rd("responsiveness.csv"), rd("manual.csv")
# Beginner-labelled issues that exist but are mostly >1 year old (checked by hand in raw/ listings)
STALE = {"pytorch/ignite", "huggingface/datasets", "jax-ml/jax", "unionai-oss/pandera", "dask/dask",
         "networkx/networkx", "numpy/numpy", "urllib3/urllib3", "pytest-dev/pytest", "python/mypy",
         "Lightning-AI/pytorch-lightning", "huggingface/huggingface_hub", "huggingface/peft",
         "keras-team/keras", "aio-libs/aiohttp", "sympy/sympy", "astral-sh/ruff"}
# Repos where a bot auto-comments on every new issue, so "has a comment" says nothing about humans
BOT_INFLATED = {"langchain-ai/langchain"}
clamp = lambda x: max(0.0, min(1.0, x))
lg = lambda x, cap: clamp(math.log10(1 + x) / math.log10(1 + cap))
num = lambda s: float(s) if s not in ("", "NA", None) else None
rows = []
for repo, cat in cats.items():
    g, s, r, m = git.get(repo, {}), srch[repo], resp.get(repo), man[repo]
    c90, a90, new = (float(g.get(k) or 0) for k in ("commits_90d", "authors_90d", "new_authors_180d"))
    activity = 15 * (0.5 * lg(c90, 300) + 0.5 * lg(a90, 60))
    merged, total = num(s["pr_merged_6m"]), num(s["pr_total_6m"])
    mr = merged / total if total else 0.0
    openness = 12 * clamp((mr - 0.10) / 0.65) + 8 * lg(new, 80)
    if r and repo not in BOT_INFLATED:
        answered = 1 - float(r["issues_nocomment"]) / float(r["issues_total"])
        responsiveness = 20 * clamp((answered - 0.40) / 0.55)
    else:
        answered, responsiveness = None, 10.0  # not measured -> neutral
    gfi = num(s["gfi_unassigned"]) or 0
    issues = 15 * lg(gfi, 40) * (0.6 if repo in STALE else 1.0)
    setup = float(m["setup"]) - (0 if g.get("contributing_file") else 1)
    welcome, fit = float(m["welcome"]), float(m["fit"])
    total_score = activity + openness + responsiveness + issues + setup + welcome + fit
    rows.append(dict(repo=repo, category=cat, score=round(total_score, 1),
        activity=round(activity, 1), openness=round(openness, 1), responsiveness=round(responsiveness, 1),
        beginner_issues=round(issues, 1), setup_docs=setup, welcoming=welcome, fit=fit,
        commits_90d=int(c90), authors_90d=int(a90), new_authors_180d=int(new),
        pr_merge_rate=f"{mr:.0%}", pr_merged_6m=s["pr_merged_6m"], pr_opened_6m=s["pr_total_6m"],
        recent_issues_answered=f"{answered:.0%}" if answered is not None else "n/m",
        open_unassigned_beginner_issues=s["gfi_unassigned"], contributing_file=g.get("contributing_file", ""),
        last_commit=g.get("last_commit", ""), programs=m["programs"], notes=s["note"], what=m["one_liner"]))
rows.sort(key=lambda x: -x["score"])
for i, x in enumerate(rows, 1):
    x["rank"] = i
out = os.path.join(D, "..", "candidates.csv")
with open(out, "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["rank"] + [k for k in rows[0] if k != "rank"])
    w.writeheader(); w.writerows(rows)
for x in rows:
    print(f'{x["rank"]:>2} {x["score"]:>5} {x["repo"]:<32} act{x["activity"]:>5} open{x["openness"]:>5} resp{x["responsiveness"]:>5} iss{x["beginner_issues"]:>5} mr{x["pr_merge_rate"]:>5} ans{x["recent_issues_answered"]:>5}')
