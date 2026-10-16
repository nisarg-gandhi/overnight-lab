"""Collect commit-activity metrics for each candidate repo from a commits-only
(treeless, shallow) git clone. No GitHub API needed.

Metrics (window ending at run date):
  commits_90d, authors_90d, new_authors_180d (authors whose first commit in the
  2-year window falls inside the last 180 days), last_commit, has_contributing.
"""
import csv, datetime as dt, os, subprocess, sys, urllib.request

ROOT = os.path.dirname(os.path.abspath(__file__))
CACHE = os.environ.get("GIT_CACHE", "/home/user/oss-cache")
NOW = dt.datetime.now(dt.timezone.utc)

def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)

def fetch(repo):
    path = os.path.join(CACHE, repo.replace("/", "__") + ".git")
    if not os.path.exists(path):
        r = run(["git", "clone", "--bare", "--filter=tree:0", "--single-branch",
                 "--shallow-since=2024-10-16", f"https://github.com/{repo}", path], timeout=1200)
        if r.returncode:
            raise RuntimeError(r.stderr[-300:])
    return path

def contributing(repo, branch):
    for p in ("CONTRIBUTING.md", ".github/CONTRIBUTING.md", "CONTRIBUTING.rst",
              "docs/CONTRIBUTING.md", ".github/CONTRIBUTING.rst", "CONTRIBUTING.txt"):
        try:
            with urllib.request.urlopen(f"https://raw.githubusercontent.com/{repo}/{branch}/{p}", timeout=20) as f:
                if f.status == 200:
                    return p
        except Exception:
            pass
    return ""

def main():
    out = os.path.join(ROOT, "git_activity.csv")
    done = set()
    if os.path.exists(out):
        done = {r["repo"] for r in csv.DictReader(open(out))}
    fields = ["repo", "branch", "commits_90d", "authors_90d", "new_authors_180d",
              "last_commit", "contributing_file"]
    fh = open(out, "a", newline="")
    w = csv.DictWriter(fh, fieldnames=fields)
    if not done:
        w.writeheader()
    for line in open(os.path.join(ROOT, "repos.txt")):
        repo = line.split(",")[0].strip()
        if not repo or repo in done:
            continue
        try:
            path = fetch(repo)
            branch = run(["git", "-C", path, "symbolic-ref", "--short", "HEAD"]).stdout.strip()
            log = run(["git", "-C", path, "log", "--format=%aE|%aN|%cI", "HEAD"]).stdout.splitlines()
            first_seen, c90, a90 = {}, 0, set()
            last = None
            for ln in log:
                parts = ln.split("|")
                email, name, date = parts[0], "|".join(parts[1:-1]), parts[-1]
                key = name.lower()
                if "[bot]" in key or "bot@" in email:
                    continue
                d = dt.datetime.fromisoformat(date)
                last = max(last, d) if last else d
                first_seen[key] = min(first_seen.get(key, d), d)
                if (NOW - d).days <= 90:
                    c90 += 1
                    a90.add(key)
            new180 = sum(1 for d in first_seen.values() if (NOW - d).days <= 180)
            w.writerow(dict(repo=repo, branch=branch, commits_90d=c90, authors_90d=len(a90),
                            new_authors_180d=new180, last_commit=last.date() if last else "",
                            contributing_file=contributing(repo, branch)))
        except Exception as e:
            print(repo, "FAILED", e, file=sys.stderr)
            w.writerow(dict(repo=repo, branch="ERROR"))
        fh.flush()
        print("done", repo, flush=True)

if __name__ == "__main__":
    main()
